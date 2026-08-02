use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use axum::extract::ws::Message;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    backends::AppServices,
    media::MediaStore,
    protocol::{ClientEvent, INPUT_SAMPLE_RATE, PipelinePhase, ServerEvent, SessionConfig},
    storage::Database,
};

use super::{
    PipelineContext, PipelineIdentity, PipelineInput,
    config::normalize_config,
    events::{send_event, send_state},
    jobs::{LiveUtterance, PipelineWorkers},
    latency::SpeechStart,
};

const CLIENT_FRAME_SAMPLES: usize = 512;
const CLIENT_PRE_ROLL_SAMPLES: usize = CLIENT_FRAME_SAMPLES * 7;
const MIN_CLIENT_UTTERANCE_SAMPLES: usize = INPUT_SAMPLE_RATE as usize / 5;

pub async fn run_pipeline(
    services: Arc<AppServices>,
    database: Option<Database>,
    media: MediaStore,
    identity: PipelineIdentity,
    mut input: mpsc::Receiver<PipelineInput>,
    output: mpsc::Sender<Message>,
) -> Result<()> {
    let session_id = Uuid::new_v4();
    let mut config = SessionConfig::default();
    if let Some(database) = &database {
        if let Some(room) = database
            .get_room(identity.room_id)
            .await
            .context("failed to load room pipeline configuration")?
        {
            config.source_language = room.source_language;
            config.target_language = room.target_language;
            config.max_utterance_seconds = room.max_utterance_seconds as u32;
        }
        database
            .create_session(
                session_id,
                identity.user_id,
                identity.room_id,
                services.backend_name,
                &config,
            )
            .await
            .context("failed to persist voice session")?;
    }
    send_event(
        &output,
        ServerEvent::Ready {
            session_id: session_id.to_string(),
            room_id: identity.room_id.to_string(),
            backend: services.backend_name.to_owned(),
            input_sample_rate: INPUT_SAMPLE_RATE,
        },
    )
    .await?;
    send_state(&output, PipelinePhase::Listening, None).await?;

    let workers = PipelineWorkers::start(PipelineContext {
        services,
        database: database.clone(),
        media,
        session_id,
        user_id: identity.user_id,
        room_id: identity.room_id,
        output: output.clone(),
    });
    let mut live_utterance: Option<LiveUtterance> = None;
    let mut capturing = false;
    let mut segment_active = false;
    let mut segment_started: Option<Instant> = None;
    let mut segment_audio = Vec::new();
    let mut queued_since_start = 0_usize;

    while let Some(message) = input.recv().await {
        match message {
            PipelineInput::Event(ClientEvent::Configure(next)) => match normalize_config(next) {
                Ok(next) => {
                    config = next;
                    if let Some(database) = &database {
                        database
                            .update_session_config(session_id, &config)
                            .await
                            .context("failed to update persisted session config")?;
                    }
                    send_event(
                        &output,
                        ServerEvent::Configured {
                            source_language: config.source_language.clone(),
                            target_language: config.target_language.clone(),
                            voice: config.voice.clone(),
                            max_utterance_seconds: config.max_utterance_seconds,
                        },
                    )
                    .await?;
                }
                Err(message) => {
                    send_event(&output, ServerEvent::Warning { message }).await?;
                }
            },
            PipelineInput::Event(ClientEvent::Start) => {
                if let Some(live) = live_utterance.take() {
                    workers.cancel_live(live, &output).await;
                }
                capturing = true;
                segment_active = false;
                segment_started = None;
                segment_audio.clear();
                queued_since_start = 0;
                send_state(&output, PipelinePhase::Listening, None).await?;
            }
            PipelineInput::Event(ClientEvent::SpeechStart) => {
                if !capturing {
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: "忽略了录音会话之外的语音开始信号。".to_owned(),
                        },
                    )
                    .await?;
                    continue;
                }
                if segment_active {
                    continue;
                }

                segment_active = true;
                segment_started = Some(Instant::now());
                segment_audio.clear();
                let started = SpeechStart::now();
                send_event(
                    &output,
                    ServerEvent::Vad {
                        active: true,
                        level: 1.0,
                    },
                )
                .await?;
                send_state(&output, PipelinePhase::Speech, None).await?;
                live_utterance = Some(workers.begin_live(started, &config, &output).await?);
            }
            PipelineInput::Event(ClientEvent::SpeechEnd) => {
                if !capturing || !segment_active {
                    continue;
                }

                if segment_audio.len() >= MIN_CLIENT_UTTERANCE_SAMPLES {
                    if let Some(live) = live_utterance.take() {
                        workers
                            .finish_live(std::mem::take(&mut segment_audio), live)
                            .await?;
                        queued_since_start += 1;
                    }
                } else if let Some(live) = live_utterance.take() {
                    workers.cancel_live(live, &output).await;
                }
                segment_audio.clear();
                segment_active = false;
                segment_started = None;
                send_event(
                    &output,
                    ServerEvent::Vad {
                        active: false,
                        level: 0.0,
                    },
                )
                .await?;
                send_state(&output, PipelinePhase::Listening, None).await?;
            }
            PipelineInput::Event(ClientEvent::Flush) => {
                if segment_active && segment_audio.len() >= MIN_CLIENT_UTTERANCE_SAMPLES {
                    if let Some(live) = live_utterance.take() {
                        workers
                            .finish_live(std::mem::take(&mut segment_audio), live)
                            .await?;
                        queued_since_start += 1;
                    }
                } else if let Some(live) = live_utterance.take() {
                    workers.cancel_live(live, &output).await;
                }
                if queued_since_start == 0 {
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: "Web 端未检测到有效语音，请靠近麦克风后重试。".to_owned(),
                        },
                    )
                    .await?;
                }
                capturing = false;
                segment_active = false;
                segment_started = None;
                segment_audio.clear();
                send_event(
                    &output,
                    ServerEvent::Vad {
                        active: false,
                        level: 0.0,
                    },
                )
                .await?;
                send_state(&output, PipelinePhase::Listening, None).await?;
            }
            PipelineInput::Audio(bytes) => {
                if !capturing || !segment_active {
                    continue;
                }
                let Some(frame) = decode_web_vad_frame(&bytes) else {
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: "Web 端音频帧格式无效，已丢弃。".to_owned(),
                        },
                    )
                    .await?;
                    continue;
                };

                let elapsed = segment_started
                    .map(|started| started.elapsed().as_secs_f64())
                    .unwrap_or_default();
                let realtime_limit = CLIENT_PRE_ROLL_SAMPLES
                    + (elapsed * INPUT_SAMPLE_RATE as f64 * 1.5) as usize
                    + CLIENT_FRAME_SAMPLES;
                if segment_audio.len() + frame.len() > realtime_limit {
                    tracing::warn!(
                        session_id = %session_id,
                        received_samples = segment_audio.len() + frame.len(),
                        realtime_limit,
                        "dropping Web VAD audio received faster than real time"
                    );
                    continue;
                }

                let hard_limit = config.max_utterance_seconds as usize * INPUT_SAMPLE_RATE as usize
                    + CLIENT_PRE_ROLL_SAMPLES;
                let remaining = hard_limit.saturating_sub(segment_audio.len());
                let accepted = &frame[..frame.len().min(remaining)];
                segment_audio.extend_from_slice(accepted);
                if let Some(live) = &live_utterance
                    && let Err(error) = live.push(accepted)
                {
                    tracing::warn!(%error, "live ASR stopped while receiving Web VAD speech");
                }

                if segment_audio.len() >= hard_limit {
                    if let Some(live) = live_utterance.take() {
                        workers
                            .finish_live(std::mem::take(&mut segment_audio), live)
                            .await?;
                        queued_since_start += 1;
                    }
                    segment_active = false;
                    segment_started = None;
                    segment_audio.clear();
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: format!(
                                "已达到最长断句 {} 秒，服务器已强制结束本段。",
                                config.max_utterance_seconds
                            ),
                        },
                    )
                    .await?;
                    send_event(
                        &output,
                        ServerEvent::Vad {
                            active: false,
                            level: 0.0,
                        },
                    )
                    .await?;
                    send_state(&output, PipelinePhase::Listening, None).await?;
                }
            }
            PipelineInput::Invalid(message) => {
                send_event(&output, ServerEvent::Warning { message }).await?;
            }
            PipelineInput::Ping(payload) => {
                output
                    .send(Message::Pong(payload.into()))
                    .await
                    .context("WebSocket writer closed")?;
            }
        }
    }

    workers.abort().await;
    if let Some(database) = &database {
        if let Err(error) = database
            .interrupt_session_utterances(
                session_id,
                "WebSocket connection closed before processing completed",
            )
            .await
        {
            tracing::warn!(%error, %session_id, "failed to mark interrupted utterances");
        }
        if let Err(error) = database.complete_session(session_id).await {
            tracing::warn!(%error, %session_id, "failed to mark voice session complete");
        }
    }
    Ok(())
}

fn decode_web_vad_frame(bytes: &[u8]) -> Option<Vec<i16>> {
    if bytes.len() != CLIENT_FRAME_SAMPLES * size_of::<i16>() {
        return None;
    }
    Some(
        bytes
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_vad_frames_must_be_exactly_thirty_two_milliseconds() {
        assert_eq!(decode_web_vad_frame(&vec![0; 1_024]).unwrap().len(), 512);
        assert!(decode_web_vad_frame(&vec![0; 1_022]).is_none());
        assert!(decode_web_vad_frame(&vec![0; 1_026]).is_none());
    }
}
