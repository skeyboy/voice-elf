use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::{
    backends::NoSpeechDetected,
    protocol::{INPUT_SAMPLE_RATE, PipelinePhase, ServerEvent},
    storage::{NewUtteranceAttempt, TranscriptUpdate},
};

use super::{
    PipelineContext,
    events::{send_event, send_state},
    jobs::{TranslationJob, UtteranceJob},
};

pub(super) async fn run_transcription_worker(
    context: PipelineContext,
    mut input: mpsc::Receiver<UtteranceJob>,
    output: mpsc::Sender<TranslationJob>,
) {
    while let Some(job) = input.recv().await {
        let utterance_id = job.id;
        match transcribe(&context, job).await {
            Ok(job) => {
                if output.send(job).await.is_err() {
                    break;
                }
            }
            Err(error) => {
                let message = if error.downcast_ref::<NoSpeechDetected>().is_some() {
                    "未识别到清晰语音，已保留原声".to_owned()
                } else {
                    tracing::warn!(%error, %utterance_id, "speech recognition failed");
                    "语音识别处理失败，已保留原声".to_owned()
                };
                if let Some(database) = &context.database
                    && let Err(storage_error) = database
                        .mark_utterance_failed(
                            utterance_id,
                            "recognition_failed",
                            &error.to_string(),
                        )
                        .await
                {
                    tracing::warn!(%storage_error, %utterance_id, "failed to persist recognition failure");
                }
                let _ = send_event(
                    &context.output,
                    ServerEvent::RecognitionFailed {
                        utterance_id: utterance_id.to_string(),
                        message,
                    },
                )
                .await;
            }
        }
    }
}

async fn transcribe(context: &PipelineContext, mut job: UtteranceJob) -> Result<TranslationJob> {
    let utterance_id = job.id.to_string();
    send_state(
        &context.output,
        PipelinePhase::Transcribing,
        Some(&utterance_id),
    )
    .await?;
    prepare_utterance(context, &mut job).await?;
    let transcription = if let Some(live) = job.live.take() {
        match live.finish().await {
            Ok(primary) => context
                .services
                .transcriber
                .refine_transcription(&job.audio, &job.config.source_language, primary)
                .await
                .context("parallel ASR refinement failed")?,
            Err(error) => {
                tracing::warn!(%error, %utterance_id, "live ASR failed; retrying completed utterance");
                transcribe_completed_audio(context, &job, &utterance_id).await?
            }
        }
    } else {
        transcribe_completed_audio(context, &job, &utterance_id).await?
    };
    send_event(
        &context.output,
        ServerEvent::TranscriptDelta {
            utterance_id: utterance_id.clone(),
            delta: String::new(),
            text: transcription.text.clone(),
            language: transcription.language.clone(),
            done: true,
        },
    )
    .await?;
    job.latency.mark_stt_complete();
    if let Some(database) = &context.database {
        let latency = job.latency.transcription_report(job.audio.len());
        database
            .save_utterance_transcript(TranscriptUpdate {
                id: job.id,
                source_text: &transcription.text,
                source_language: &transcription.language,
                latency: &latency,
            })
            .await
            .context("failed to persist transcript")?;
    }
    send_event(
        &context.output,
        ServerEvent::Transcript {
            utterance_id,
            text: transcription.text.clone(),
            language: transcription.language.clone(),
        },
    )
    .await?;

    Ok(TranslationJob {
        utterance: job,
        transcription,
    })
}

async fn transcribe_completed_audio(
    context: &PipelineContext,
    job: &UtteranceJob,
    utterance_id: &str,
) -> Result<crate::backends::Transcription> {
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
    let mut live_text = String::new();
    let transcription = {
        let transcription_future = context.services.transcriber.transcribe_streaming(
            &job.audio,
            &job.config.source_language,
            updates_tx,
        );
        tokio::pin!(transcription_future);
        loop {
            tokio::select! {
                result = &mut transcription_future => break result.context("speech recognition failed")?,
                Some(delta) = updates_rx.recv() => {
                    live_text.push_str(&delta);
                    send_event(
                        &context.output,
                        ServerEvent::TranscriptDelta {
                            utterance_id: utterance_id.to_owned(),
                            delta,
                            text: live_text.clone(),
                            language: job.config.source_language.clone(),
                            done: false,
                        },
                    )
                    .await?;
                }
            }
        }
    };
    while let Ok(delta) = updates_rx.try_recv() {
        live_text.push_str(&delta);
        send_event(
            &context.output,
            ServerEvent::TranscriptDelta {
                utterance_id: utterance_id.to_owned(),
                delta,
                text: live_text.clone(),
                language: transcription.language.clone(),
                done: false,
            },
        )
        .await?;
    }
    Ok(transcription)
}

async fn prepare_utterance(context: &PipelineContext, job: &mut UtteranceJob) -> Result<()> {
    let utterance_id = job.id.to_string();
    let source_media = match context
        .media
        .save_source(context.session_id, job.id, &job.audio, INPUT_SAMPLE_RATE)
        .await
    {
        Ok(media) => Some(media),
        Err(error) => {
            tracing::warn!(%error, %utterance_id, "failed to save source audio before ASR");
            None
        }
    };
    let latency = job.latency.queued_report(job.audio.len());
    if let Some(database) = &context.database {
        database
            .create_utterance_attempt(NewUtteranceAttempt {
                id: job.id,
                session_id: context.session_id,
                user_id: context.user_id,
                room_id: context.room_id,
                source_language: &job.config.source_language,
                target_language: &job.config.target_language,
                source_audio_path: source_media.as_ref().map(|media| media.path.as_str()),
                source_audio_url: source_media.as_ref().map(|media| media.url.as_str()),
                latency: &latency,
            })
            .await
            .context("failed to create utterance record")?;
    }
    if let Some(media) = source_media {
        send_event(
            &context.output,
            ServerEvent::Media {
                utterance_id,
                source_audio_url: Some(media.url),
                translated_audio_url: None,
            },
        )
        .await?;
    }
    Ok(())
}
