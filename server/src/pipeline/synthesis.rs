use anyhow::{Context, Result};
use axum::extract::ws::Message;
use tokio::sync::mpsc;

use crate::{
    audio::pcm16_bytes,
    protocol::{PipelinePhase, ProcessingStage, ServerEvent},
    storage::UtteranceAudioUpdate,
};

use super::{
    PipelineContext,
    events::{send_event, send_state},
    jobs::SynthesisJob,
};

const AUDIO_CHUNK_SAMPLES: usize = 4_096;

pub(super) async fn run_synthesis_worker(
    context: PipelineContext,
    mut input: mpsc::Receiver<SynthesisJob>,
) {
    while let Some(job) = input.recv().await {
        let utterance_id = job.utterance.id;
        if let Err(error) = synthesize(&context, job).await {
            if let Some(database) = &context.database
                && let Err(storage_error) = database
                    .mark_utterance_failed(utterance_id, "tts_failed", &error.to_string())
                    .await
            {
                tracing::warn!(%storage_error, %utterance_id, "failed to persist TTS failure");
            }
            tracing::warn!(%error, %utterance_id, "utterance synthesis failed");
            let _ = send_event(
                &context.output,
                ServerEvent::ProcessingFailed {
                    utterance_id: utterance_id.to_string(),
                    stage: ProcessingStage::Tts,
                    message: "译声生成未完成，原文、译文和原声已保留".to_owned(),
                },
            )
            .await;
        }
    }
}

async fn synthesize(context: &PipelineContext, mut job: SynthesisJob) -> Result<()> {
    let utterance_id = job.utterance.id.to_string();
    send_state(
        &context.output,
        PipelinePhase::Synthesizing,
        Some(&utterance_id),
    )
    .await?;
    let audio = context
        .services
        .synthesizer
        .synthesize(
            &job.translated_text,
            &job.utterance.config.target_language,
            &job.utterance.config.voice,
        )
        .await
        .context("TTS generation failed")?;
    if audio.samples.is_empty() {
        anyhow::bail!("TTS returned empty audio");
    }
    let media = context
        .media
        .save_translated(
            context.session_id,
            job.utterance.id,
            &audio.samples,
            audio.sample_rate,
        )
        .await
        .context("failed to persist translated audio")?;
    job.utterance.latency.mark_tts_complete();
    let latency = job
        .utterance
        .latency
        .final_report(job.utterance.audio.len());
    if let Some(database) = &context.database {
        database
            .complete_utterance_audio(UtteranceAudioUpdate {
                id: job.utterance.id,
                translated_audio_path: &media.path,
                translated_audio_url: &media.url,
                latency: &latency,
            })
            .await
            .context("failed to persist translated audio metadata")?;
    }

    send_event(
        &context.output,
        ServerEvent::Media {
            utterance_id: utterance_id.clone(),
            source_audio_url: None,
            translated_audio_url: Some(media.url),
        },
    )
    .await?;
    send_event(
        &context.output,
        ServerEvent::AudioStart {
            utterance_id: utterance_id.clone(),
            sample_rate: audio.sample_rate,
            sample_count: audio.samples.len(),
        },
    )
    .await?;
    send_state(&context.output, PipelinePhase::Playing, Some(&utterance_id)).await?;
    for samples in audio.samples.chunks(AUDIO_CHUNK_SAMPLES) {
        if context
            .output
            .send(Message::Binary(pcm16_bytes(samples).into()))
            .await
            .is_err()
        {
            tracing::debug!(%utterance_id, "audio subscriber disconnected during playback stream");
            break;
        }
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
    .await?;
    Ok(())
}
