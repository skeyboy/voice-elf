use std::{collections::VecDeque, future::Future};

use anyhow::{Context, Result};
use axum::extract::ws::Message;
use tokio::sync::mpsc;

use crate::{
    audio::pcm16_bytes,
    backends::SynthesizedAudio,
    protocol::{PipelinePhase, ProcessingStage, ServerEvent},
    storage::UtteranceAudioUpdate,
};

use super::{
    PipelineContext,
    events::{send_event, send_state},
    jobs::{SynthesisJob, TextWorkload},
};

const OUTPUT_AUDIO_CHUNK_SAMPLES: usize = 4_096;

pub(super) async fn run_synthesis_worker(
    context: PipelineContext,
    mut input: mpsc::Receiver<SynthesisJob>,
    workload: TextWorkload,
) {
    let mut pending = VecDeque::new();
    loop {
        if pending.is_empty() {
            let Some(job) = input.recv().await else {
                break;
            };
            pending.push_back(job);
        }
        while let Ok(job) = input.try_recv() {
            pending.push_back(job);
        }

        workload.wait_until_idle().await;
        let Some(job) = pending.pop_front() else {
            continue;
        };
        let utterance_id = job.utterance.id.to_string();
        let _ = send_state(
            &context.output,
            PipelinePhase::Synthesizing,
            Some(&utterance_id),
        )
        .await;

        let synthesis = context.services.synthesizer.synthesize(
            &job.translated,
            &job.utterance.config.target_language,
            &job.utterance.config.voice,
        );
        let outcome = finish_or_defer(&workload, synthesis).await;

        match outcome {
            None => {
                tracing::info!(%utterance_id, "deferring TTS for pending text work");
                pending.push_front(job);
            }
            Some(Ok(audio)) => {
                if let Err(error) = publish_audio(&context, job, audio).await {
                    report_synthesis_error(&context, &utterance_id, &error).await;
                }
            }
            Some(Err(error)) => {
                let error = error.context("speech synthesis failed");
                report_synthesis_error(&context, &utterance_id, &error).await;
            }
        }
    }
}

async fn finish_or_defer<F, T>(workload: &TextWorkload, future: F) -> Option<T>
where
    F: Future<Output = T>,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = workload.wait_for_work() => None,
        result = &mut future => Some(result),
    }
}

async fn publish_audio(
    context: &PipelineContext,
    mut job: SynthesisJob,
    audio: SynthesizedAudio,
) -> Result<()> {
    let utterance_id = job.utterance.id.to_string();
    job.utterance.latency.mark_tts_complete();
    let media = context
        .media
        .save_translated(
            context.session_id,
            job.utterance.id,
            &audio.samples,
            audio.sample_rate,
        )
        .await
        .context("failed to save translated audio")?;
    let latency = job
        .utterance
        .latency
        .complete_report(job.utterance.audio.len());
    let media_persisted = if let Some(database) = &context.database {
        match database
            .complete_utterance_audio(UtteranceAudioUpdate {
                id: job.utterance.id,
                translated_audio_path: &media.path,
                translated_audio_url: &media.url,
                latency: &latency,
            })
            .await
        {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, %utterance_id, "failed to persist translated audio");
                false
            }
        }
    } else {
        true
    };
    if media_persisted {
        send_event(
            &context.output,
            ServerEvent::Media {
                utterance_id: utterance_id.clone(),
                source_audio_url: None,
                translated_audio_url: Some(media.url),
            },
        )
        .await?;
    }

    send_state(&context.output, PipelinePhase::Playing, Some(&utterance_id)).await?;
    send_event(
        &context.output,
        ServerEvent::AudioStart {
            utterance_id: utterance_id.clone(),
            sample_rate: audio.sample_rate,
            sample_count: audio.samples.len(),
        },
    )
    .await?;
    for chunk in audio.samples.chunks(OUTPUT_AUDIO_CHUNK_SAMPLES) {
        context
            .output
            .send(Message::Binary(pcm16_bytes(chunk).into()))
            .await
            .context("WebSocket writer closed during audio delivery")?;
    }
    send_event(
        &context.output,
        ServerEvent::AudioEnd {
            utterance_id: utterance_id.clone(),
        },
    )
    .await?;
    send_event(
        &context.output,
        ServerEvent::Latency {
            utterance_id,
            latency,
        },
    )
    .await
}

async fn report_synthesis_error(
    context: &PipelineContext,
    utterance_id: &str,
    error: &anyhow::Error,
) {
    tracing::warn!(%error, %utterance_id, "translated speech generation failed");
    if let Some(database) = &context.database
        && let Ok(id) = uuid::Uuid::parse_str(utterance_id)
        && let Err(storage_error) = database
            .mark_utterance_failed(id, "tts_failed", &error.to_string())
            .await
    {
        tracing::warn!(%storage_error, %utterance_id, "failed to persist TTS failure");
    }
    let _ = send_event(
        &context.output,
        ServerEvent::ProcessingFailed {
            utterance_id: utterance_id.to_owned(),
            stage: ProcessingStage::Tts,
            message: "译声生成未完成，原文和译文已保留".to_owned(),
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use super::*;

    #[tokio::test]
    async fn pending_text_preempts_synthesis() {
        let workload = TextWorkload::default();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_by_future = completed.clone();
        let workload_for_task = workload.clone();
        let task = tokio::spawn(async move {
            finish_or_defer(&workload_for_task, async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                completed_by_future.store(true, Ordering::Release);
            })
            .await
        });

        tokio::task::yield_now().await;
        workload.add();
        assert!(task.await.unwrap().is_none());
        assert!(!completed.load(Ordering::Acquire));
        workload.finish();
    }
}
