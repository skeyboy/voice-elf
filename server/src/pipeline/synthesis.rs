use anyhow::{Context, Result};
use axum::extract::ws::Message;
use tokio::sync::mpsc;

use crate::{
    audio::pcm16_bytes,
    backends::{TtsChunkSink, TtsRequest},
    protocol::{AudioCodec, PipelinePhase, ProcessingStage, ServerEvent},
    storage::UtteranceAudioUpdate,
};

use super::{
    PipelineContext,
    events::{send_event, send_state},
    jobs::SynthesisJob,
};

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
    let reference_audio_path = if let Some(id) = job
        .utterance
        .config
        .voice
        .strip_prefix("custom:")
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
    {
        let database = context
            .database
            .as_ref()
            .context("custom voice references require PostgreSQL")?;
        let reference = database
            .get_voice_reference(id, context.user_id)
            .await?
            .context("custom voice reference does not exist or is not owned by this user")?;
        Some(reference.audio_path.into())
    } else {
        None
    };
    let request = TtsRequest {
        text: job.translated_text.clone(),
        language: job.utterance.config.target_language.clone(),
        voice: job.utterance.config.voice.clone(),
        reference_audio_path,
    };
    let synthesizer = context.services.synthesizer.clone();
    let (audio_tx, mut audio_rx) = mpsc::channel(8);
    let synthesis = tokio::spawn(async move {
        synthesizer
            .synthesize(&request, TtsChunkSink::new(audio_tx))
            .await
    });
    let mut samples = Vec::new();
    let mut sample_rate = None;
    let mut channels = None;
    let mut engine = None;
    let mut stream_started = false;
    while let Some(chunk) = audio_rx.recv().await {
        if let Some(expected) = sample_rate
            && expected != chunk.sample_rate
        {
            anyhow::bail!("TTS engine changed sample rate during one utterance");
        }
        if let Some(expected) = channels
            && expected != chunk.channels
        {
            anyhow::bail!("TTS engine changed channel count during one utterance");
        }
        if let Some(expected) = engine
            && expected != chunk.engine
        {
            anyhow::bail!("TTS fallback changed engines after streaming started");
        }
        sample_rate = Some(chunk.sample_rate);
        channels = Some(chunk.channels);
        engine = Some(chunk.engine);
        if !stream_started {
            send_event(
                &context.output,
                ServerEvent::AudioStart {
                    utterance_id: utterance_id.clone(),
                    engine: chunk.engine.to_owned(),
                    codec: AudioCodec::PcmS16le,
                    sample_rate: chunk.sample_rate,
                    channels: chunk.channels,
                    sample_count: None,
                },
            )
            .await?;
            send_state(&context.output, PipelinePhase::Playing, Some(&utterance_id)).await?;
            stream_started = true;
        }
        if context
            .output
            .send(Message::Binary(pcm16_bytes(&chunk.samples).into()))
            .await
            .is_err()
        {
            tracing::debug!(%utterance_id, "audio subscriber disconnected during playback stream");
        }
        samples.extend_from_slice(&chunk.samples);
    }
    let synthesis_result = synthesis
        .await
        .context("TTS engine task failed")?
        .context("TTS generation failed");
    if stream_started {
        let channel_count = usize::from(channels.unwrap_or(1));
        let _ = send_event(
            &context.output,
            ServerEvent::AudioEnd {
                utterance_id: utterance_id.clone(),
                sample_count: samples.len() / channel_count,
            },
        )
        .await;
    }
    synthesis_result?;
    if samples.is_empty() {
        anyhow::bail!("TTS returned empty audio");
    }
    let sample_rate = sample_rate.context("TTS returned no sample rate")?;
    let channels = channels.context("TTS returned no channel count")?;
    let media = context
        .media
        .save_translated(
            context.session_id,
            job.utterance.id,
            &samples,
            sample_rate,
            channels,
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
        ServerEvent::Latency {
            utterance_id,
            latency,
        },
    )
    .await?;
    Ok(())
}
