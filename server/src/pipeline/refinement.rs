use tokio::sync::mpsc;

use crate::{
    protocol::{RefinementStatus, ServerEvent},
    storage::RefinementUpdate,
};

use super::{PipelineContext, events::send_event, jobs::RefinementJob};

pub(super) async fn run_refinement_worker(
    context: PipelineContext,
    mut input: mpsc::Receiver<RefinementJob>,
) {
    while let Some(job) = input.recv().await {
        for engine in &context.services.refinement_engines {
            let engine_name = engine.name();
            if let Some(database) = &context.database
                && let Err(error) = database
                    .start_utterance_refinement(job.utterance_id, engine_name)
                    .await
            {
                tracing::warn!(%error, utterance_id = %job.utterance_id, engine = engine_name, "failed to persist refinement start");
            }
            let _ = send_event(
                &context.output,
                ServerEvent::TranscriptRefinement {
                    utterance_id: job.utterance_id.to_string(),
                    engine: engine_name.to_owned(),
                    status: RefinementStatus::Processing,
                    text: None,
                    language: None,
                    segments: Vec::new(),
                    message: None,
                },
            )
            .await;

            match engine
                .transcribe_completed(&job.audio, &job.source_language)
                .await
            {
                Ok(mut transcription) => {
                    transcription.text = context
                        .language_policy
                        .normalize_transcript(&transcription.text);
                    for segment in &mut transcription.segments {
                        segment.text = context.language_policy.normalize_transcript(&segment.text);
                    }
                    if let Some(database) = &context.database
                        && let Err(error) = database
                            .save_utterance_refinement(RefinementUpdate {
                                utterance_id: job.utterance_id,
                                engine: engine_name,
                                text: &transcription.text,
                                language: &transcription.language,
                                segments: &transcription.segments,
                            })
                            .await
                    {
                        tracing::warn!(%error, utterance_id = %job.utterance_id, engine = engine_name, "failed to persist refinement result");
                    }
                    let _ = send_event(
                        &context.output,
                        ServerEvent::TranscriptRefinement {
                            utterance_id: job.utterance_id.to_string(),
                            engine: engine_name.to_owned(),
                            status: RefinementStatus::Completed,
                            text: Some(transcription.text),
                            language: Some(transcription.language),
                            segments: transcription.segments,
                            message: None,
                        },
                    )
                    .await;
                }
                Err(error) => {
                    tracing::warn!(%error, utterance_id = %job.utterance_id, engine = engine_name, "accurate transcription failed; primary transcript remains active");
                    if let Some(database) = &context.database
                        && let Err(storage_error) = database
                            .fail_utterance_refinement(
                                job.utterance_id,
                                engine_name,
                                &error.to_string(),
                            )
                            .await
                    {
                        tracing::warn!(%storage_error, utterance_id = %job.utterance_id, engine = engine_name, "failed to persist refinement failure");
                    }
                    let _ = send_event(
                        &context.output,
                        ServerEvent::TranscriptRefinement {
                            utterance_id: job.utterance_id.to_string(),
                            engine: engine_name.to_owned(),
                            status: RefinementStatus::Failed,
                            text: None,
                            language: None,
                            segments: Vec::new(),
                            message: Some("会后精识别失败，已保留实时识别结果".to_owned()),
                        },
                    )
                    .await;
                }
            }
        }
    }
}
