use anyhow::{Context, Result};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::{
    protocol::{PipelinePhase, ProcessingStage, ServerEvent},
    storage::TranslationUpdate,
};

use super::{
    PipelineContext,
    events::{send_event, send_state},
    jobs::{SynthesisJob, TextWorkload, TranslationJob},
};

pub(super) async fn run_translation_worker(
    context: PipelineContext,
    mut input: mpsc::Receiver<TranslationJob>,
    output: mpsc::Sender<SynthesisJob>,
    workload: TextWorkload,
) {
    while let Some(job) = input.recv().await {
        let utterance_id = job.utterance.id;
        match translate(&context, job).await {
            Ok(job) => {
                if let Err(error) = output.try_send(job) {
                    let reason = match error {
                        mpsc::error::TrySendError::Full(_) => {
                            "语音生成队列已满，本条原文和译文已保留，但暂不生成译声。"
                        }
                        mpsc::error::TrySendError::Closed(_) => {
                            "语音生成服务已停止，本条原文和译文已保留。"
                        }
                    };
                    let _ = send_event(
                        &context.output,
                        ServerEvent::Warning {
                            message: reason.to_owned(),
                        },
                    )
                    .await;
                }
                workload.finish();
            }
            Err(error) => {
                if let Some(database) = &context.database
                    && let Err(storage_error) = database
                        .mark_utterance_failed(
                            utterance_id,
                            "translation_failed",
                            &error.to_string(),
                        )
                        .await
                {
                    tracing::warn!(%storage_error, %utterance_id, "failed to persist translation failure");
                }
                tracing::warn!(%error, %utterance_id, "utterance translation failed");
                let _ = send_event(
                    &context.output,
                    ServerEvent::ProcessingFailed {
                        utterance_id: utterance_id.to_string(),
                        stage: ProcessingStage::Translation,
                        message: "翻译未完成，原文和原声已保留".to_owned(),
                    },
                )
                .await;
                workload.finish();
            }
        }
    }
}

async fn translate(context: &PipelineContext, mut job: TranslationJob) -> Result<SynthesisJob> {
    let utterance_id = job.utterance.id.to_string();
    send_state(
        &context.output,
        PipelinePhase::Translating,
        Some(&utterance_id),
    )
    .await?;
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
    let mut live_text = String::new();
    let started_at = Instant::now();
    let mut first_delta = true;
    let translated = {
        let translation = context.services.translator.translate_streaming(
            &job.transcription.text,
            &job.transcription.language,
            &job.utterance.config.target_language,
            updates_tx,
        );
        tokio::pin!(translation);
        loop {
            tokio::select! {
                result = &mut translation => break result.context("translation failed")?,
                Some(delta) = updates_rx.recv() => {
                    if first_delta && !delta.trim().is_empty() {
                        first_delta = false;
                        tracing::info!(
                            %utterance_id,
                            first_translation_ms = started_at.elapsed().as_millis(),
                            "translator emitted its first text"
                        );
                    }
                    live_text.push_str(&delta);
                    send_event(
                        &context.output,
                        ServerEvent::TranslationDelta {
                            utterance_id: utterance_id.clone(),
                            delta,
                            text: live_text.clone(),
                            target_language: job.utterance.config.target_language.clone(),
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
            ServerEvent::TranslationDelta {
                utterance_id: utterance_id.clone(),
                delta,
                text: live_text.clone(),
                target_language: job.utterance.config.target_language.clone(),
                done: false,
            },
        )
        .await?;
    }
    job.utterance.latency.mark_translation_complete();
    send_event(
        &context.output,
        ServerEvent::TranslationDelta {
            utterance_id: utterance_id.clone(),
            delta: String::new(),
            text: translated.clone(),
            target_language: job.utterance.config.target_language.clone(),
            done: true,
        },
    )
    .await?;
    send_event(
        &context.output,
        ServerEvent::Translation {
            utterance_id: utterance_id.clone(),
            source_text: job.transcription.text.clone(),
            translated_text: translated.clone(),
            source_language: job.transcription.language.clone(),
            target_language: job.utterance.config.target_language.clone(),
        },
    )
    .await?;

    let text_latency = job.utterance.latency.text_report(job.utterance.audio.len());
    if let Some(database) = &context.database {
        database
            .save_utterance_translation(TranslationUpdate {
                id: job.utterance.id,
                translated_text: &translated,
                target_language: &job.utterance.config.target_language,
                latency: &text_latency,
            })
            .await
            .context("failed to persist translation")?;
    }

    Ok(SynthesisJob {
        utterance: job.utterance,
        translated,
    })
}
