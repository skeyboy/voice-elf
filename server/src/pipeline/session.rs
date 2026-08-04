use std::{collections::HashSet, sync::Arc, time::Instant};

use anyhow::{Context, Result};
use axum::extract::ws::Message;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    backends::AppServices,
    media::MediaStore,
    protocol::{
        ClientEvent, ClientVadStart, INPUT_SAMPLE_RATE, PipelinePhase, ServerEvent, SessionConfig,
        SpeakerIdentity, VadEndReason,
    },
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
const CLIENT_PRE_ROLL_SAMPLES: usize = CLIENT_FRAME_SAMPLES * 32;
const MIN_CLIENT_UTTERANCE_SAMPLES: usize = INPUT_SAMPLE_RATE as usize / 5;
const MIN_CLIENT_SPEECH_FRAMES: usize = 6;

struct ActiveSegment {
    tc_id: Uuid,
    config: SessionConfig,
    live: LiveUtterance,
    wall_started: Instant,
    audio: Vec<i16>,
    speakers: Vec<SpeakerIdentity>,
}

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
    let mut active_segment: Option<ActiveSegment> = None;
    let mut seen_tc_ids = HashSet::new();

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
            PipelineInput::Event(ClientEvent::Start {
                tc_id,
                vad,
                config: segment_config,
            }) => {
                if let Some(vad) = &vad
                    && let Err(message) = validate_vad_start(vad)
                {
                    send_event(&output, ServerEvent::Warning { message }).await?;
                    continue;
                }
                let segment_config = match normalize_config(segment_config) {
                    Ok(config) => config,
                    Err(message) => {
                        send_event(&output, ServerEvent::Warning { message }).await?;
                        continue;
                    }
                };
                if !seen_tc_ids.insert(tc_id) {
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: format!("重复的 tc_id 已被拒绝：{tc_id}"),
                        },
                    )
                    .await?;
                    continue;
                }
                if let Some(previous) = active_segment.take() {
                    discard_segment(
                        &output,
                        previous.tc_id,
                        "新的分段已开始，上一静音占位已丢弃",
                    )
                    .await?;
                }
                config = segment_config.clone();
                if let Some(database) = &database {
                    database
                        .update_session_config(session_id, &config)
                        .await
                        .context("failed to update persisted segment config")?;
                }
                let started = SpeechStart::now();
                let live = workers
                    .begin_live(tc_id, started, &segment_config, &output)
                    .await?;
                active_segment = Some(ActiveSegment {
                    tc_id,
                    config: segment_config,
                    live,
                    wall_started: Instant::now(),
                    audio: Vec::new(),
                    speakers: Vec::new(),
                });
                send_event(
                    &output,
                    ServerEvent::Vad {
                        active: true,
                        level: 1.0,
                        utterance_id: Some(tc_id.to_string()),
                        reason: None,
                        sample_count: 0,
                    },
                )
                .await?;
                send_state(&output, PipelinePhase::Speech, Some(&tc_id.to_string())).await?;
            }
            PipelineInput::Event(ClientEvent::End {
                tc_id,
                is_silent_vad,
                vad,
            }) => {
                let Some(segment) = active_segment.take() else {
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: format!("未找到 tc_id 对应的活动分段：{tc_id}"),
                        },
                    )
                    .await?;
                    continue;
                };
                if segment.tc_id != tc_id {
                    let active_tc_id = segment.tc_id;
                    active_segment = Some(segment);
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: format!(
                                "分段结束顺序无效：收到 {tc_id}，当前活动分段为 {active_tc_id}"
                            ),
                        },
                    )
                    .await?;
                    continue;
                }
                let sample_count = segment.audio.len();
                let reason =
                    vad.as_ref()
                        .map(|metadata| metadata.reason)
                        .unwrap_or(if is_silent_vad {
                            VadEndReason::Silent
                        } else {
                            VadEndReason::Unknown
                        });
                if let Some(metadata) = &vad
                    && metadata.sample_count != sample_count
                {
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: format!(
                                "VAD 样本计数与服务端接收量不一致：Web={}，Server={}",
                                metadata.sample_count, sample_count
                            ),
                        },
                    )
                    .await?;
                }
                let insufficient_speech = has_insufficient_client_speech(vad.as_ref());
                finish_segment(
                    &workers,
                    &output,
                    segment,
                    is_silent_vad || insufficient_speech,
                )
                .await?;
                send_event(
                    &output,
                    ServerEvent::Vad {
                        active: false,
                        level: 0.0,
                        utterance_id: Some(tc_id.to_string()),
                        reason: Some(reason),
                        sample_count,
                    },
                )
                .await?;
                send_state(&output, PipelinePhase::Listening, None).await?;
            }
            PipelineInput::Event(ClientEvent::Flush) => {
                if let Some(segment) = active_segment.take() {
                    let tc_id = segment.tc_id;
                    let sample_count = segment.audio.len();
                    finish_segment(&workers, &output, segment, false).await?;
                    send_event(
                        &output,
                        ServerEvent::Vad {
                            active: false,
                            level: 0.0,
                            utterance_id: Some(tc_id.to_string()),
                            reason: Some(VadEndReason::Manual),
                            sample_count,
                        },
                    )
                    .await?;
                }
                send_state(&output, PipelinePhase::Listening, None).await?;
            }
            PipelineInput::Audio(bytes) => {
                let Some(segment) = active_segment.as_mut() else {
                    continue;
                };
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

                let elapsed = segment.wall_started.elapsed().as_secs_f64();
                let realtime_limit = CLIENT_PRE_ROLL_SAMPLES
                    + (elapsed * INPUT_SAMPLE_RATE as f64 * 1.5) as usize
                    + CLIENT_FRAME_SAMPLES;
                if segment.audio.len() + frame.len() > realtime_limit {
                    tracing::warn!(
                        session_id = %session_id,
                        tc_id = %segment.tc_id,
                        received_samples = segment.audio.len() + frame.len(),
                        realtime_limit,
                        "dropping Web VAD audio received faster than real time"
                    );
                    continue;
                }

                let hard_limit = segment.config.max_utterance_seconds as usize
                    * INPUT_SAMPLE_RATE as usize
                    + CLIENT_PRE_ROLL_SAMPLES
                    + CLIENT_FRAME_SAMPLES;
                let remaining = hard_limit.saturating_sub(segment.audio.len());
                let accepted = &frame[..frame.len().min(remaining)];
                segment.audio.extend_from_slice(accepted);
                if let Err(error) = segment.live.push(accepted) {
                    tracing::warn!(%error, tc_id = %segment.tc_id, "live ASR stopped while receiving audio");
                }

                if segment.audio.len() >= hard_limit {
                    let segment = active_segment
                        .take()
                        .expect("active segment must exist at its hard limit");
                    let max_seconds = segment.config.max_utterance_seconds;
                    let tc_id = segment.tc_id;
                    let sample_count = segment.audio.len();
                    finish_segment(&workers, &output, segment, false).await?;
                    send_event(
                        &output,
                        ServerEvent::Warning {
                            message: format!(
                                "已达到最长断句 {} 秒，服务器已强制结束本段。",
                                max_seconds
                            ),
                        },
                    )
                    .await?;
                    send_event(
                        &output,
                        ServerEvent::Vad {
                            active: false,
                            level: 0.0,
                            utterance_id: Some(tc_id.to_string()),
                            reason: Some(VadEndReason::ServerLimit),
                            sample_count,
                        },
                    )
                    .await?;
                    send_state(&output, PipelinePhase::Listening, None).await?;
                }
            }
            PipelineInput::Speakers {
                utterance_id,
                speakers,
            } => {
                let Some(segment) = active_segment.as_mut() else {
                    continue;
                };
                if segment.tc_id != utterance_id {
                    continue;
                }
                segment.speakers = speakers.clone();
                send_event(
                    &output,
                    ServerEvent::UtteranceSpeakers {
                        utterance_id: utterance_id.to_string(),
                        speakers,
                    },
                )
                .await?;
            }
        }
    }

    workers.finish().await;
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

async fn finish_segment(
    workers: &PipelineWorkers,
    output: &mpsc::Sender<Message>,
    segment: ActiveSegment,
    is_silent_vad: bool,
) -> Result<bool> {
    if is_silent_vad || segment.audio.len() < MIN_CLIENT_UTTERANCE_SAMPLES {
        let reason = if is_silent_vad {
            "VAD 判定为静音，分段未进入识别"
        } else {
            "有效语音不足 200ms，分段未进入识别"
        };
        discard_segment(output, segment.tc_id, reason).await?;
        return Ok(false);
    }
    workers
        .finish_live(segment.audio, segment.live, segment.speakers)
        .await?;
    Ok(true)
}

async fn discard_segment(output: &mpsc::Sender<Message>, tc_id: Uuid, reason: &str) -> Result<()> {
    let id = tc_id.to_string();
    send_event(
        output,
        ServerEvent::UtteranceDiscarded {
            utterance_id: id.clone(),
            tc_id: id,
            reason: reason.to_owned(),
        },
    )
    .await
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

fn has_insufficient_client_speech(vad: Option<&crate::protocol::ClientVadEnd>) -> bool {
    vad.and_then(|metadata| metadata.speech_frames)
        .is_some_and(|frames| frames < MIN_CLIENT_SPEECH_FRAMES)
}

fn validate_vad_start(vad: &ClientVadStart) -> Result<(), String> {
    if vad.engine.trim().is_empty() {
        return Err("VAD 引擎标识不能为空".to_owned());
    }
    if vad.sample_rate != INPUT_SAMPLE_RATE {
        return Err(format!(
            "VAD 输出采样率必须为 {INPUT_SAMPLE_RATE} Hz，收到 {} Hz",
            vad.sample_rate
        ));
    }
    if vad.frame_samples != CLIENT_FRAME_SAMPLES {
        return Err(format!(
            "VAD 帧长度必须为 {CLIENT_FRAME_SAMPLES} 样本，收到 {}",
            vad.frame_samples
        ));
    }
    if vad.pre_roll_samples > CLIENT_PRE_ROLL_SAMPLES {
        return Err(format!(
            "VAD pre-roll 不能超过 {CLIENT_PRE_ROLL_SAMPLES} 样本"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use serde_json::Value;

    use crate::backends::{AppServices, DemoSynthesizer, DemoTranscriber, DemoTranslator};
    use crate::protocol::ClientVadEnd;

    use super::*;

    #[test]
    fn web_vad_frames_must_be_exactly_thirty_two_milliseconds() {
        assert_eq!(decode_web_vad_frame(&vec![0; 1_024]).unwrap().len(), 512);
        assert!(decode_web_vad_frame(&vec![0; 1_022]).is_none());
        assert!(decode_web_vad_frame(&vec![0; 1_026]).is_none());
    }

    #[test]
    fn rejects_segments_without_enough_silero_confirmed_speech() {
        let insufficient = ClientVadEnd {
            reason: VadEndReason::Silence,
            sample_count: 48_000,
            speech_frames: Some(MIN_CLIENT_SPEECH_FRAMES - 1),
        };
        let valid = ClientVadEnd {
            speech_frames: Some(MIN_CLIENT_SPEECH_FRAMES),
            ..insufficient.clone()
        };

        assert!(has_insufficient_client_speech(Some(&insufficient)));
        assert!(!has_insufficient_client_speech(Some(&valid)));
        assert!(!has_insufficient_client_speech(None));
    }

    #[tokio::test]
    async fn streams_primary_asr_before_finalizing_the_segment() {
        let directory = tempfile::tempdir().unwrap();
        let media = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let services = Arc::new(AppServices {
            transcriber: Arc::new(DemoTranscriber::new()),
            translator: Arc::new(DemoTranslator::new()),
            synthesizer: Arc::new(DemoSynthesizer::new()),
            backend_name: "demo",
        });
        let (input_tx, input_rx) = mpsc::channel(64);
        let (output_tx, mut output_rx) = mpsc::channel(256);
        let tc_id = Uuid::new_v4();
        let pipeline = tokio::spawn(run_pipeline(
            services,
            None,
            media,
            PipelineIdentity {
                user_id: Uuid::new_v4(),
                room_id: Uuid::new_v4(),
            },
            input_rx,
            output_tx,
        ));

        input_tx
            .send(PipelineInput::Event(ClientEvent::Start {
                tc_id,
                vad: Some(ClientVadStart {
                    engine: "silero-v6.2-lele".to_owned(),
                    sample_rate: INPUT_SAMPLE_RATE,
                    frame_samples: CLIENT_FRAME_SAMPLES,
                    pre_roll_samples: CLIENT_PRE_ROLL_SAMPLES,
                }),
                config: SessionConfig::default(),
            }))
            .await
            .unwrap();
        for _ in 0..20 {
            input_tx
                .send(PipelineInput::Audio(
                    vec![0xe8, 0x03].repeat(CLIENT_FRAME_SAMPLES),
                ))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tokio::time::sleep(Duration::from_millis(450)).await;
        input_tx
            .send(PipelineInput::Event(ClientEvent::End {
                tc_id,
                is_silent_vad: false,
                vad: Some(ClientVadEnd {
                    reason: VadEndReason::Silence,
                    sample_count: 20 * CLIENT_FRAME_SAMPLES,
                    speech_frames: Some(20),
                }),
            }))
            .await
            .unwrap();
        drop(input_tx);
        pipeline.await.unwrap().unwrap();

        let mut event_types = Vec::new();
        let mut queued_tc_id = None;
        let mut source_media_index = None;
        let mut translated_media_index = None;
        let mut vad_end = None;
        while let Some(message) = output_rx.recv().await {
            let Message::Text(text) = message else {
                continue;
            };
            let event: Value = serde_json::from_str(&text).unwrap();
            let Some(event_type) = event["type"].as_str() else {
                continue;
            };
            if event_type == "utterance_queued" {
                queued_tc_id = event["tc_id"].as_str().map(str::to_owned);
            }
            if event_type == "media" && event["source_audio_url"].is_string() {
                source_media_index = Some(event_types.len());
            }
            if event_type == "media" && event["translated_audio_url"].is_string() {
                translated_media_index = Some(event_types.len());
            }
            if event_type == "vad" && event["active"] == false {
                vad_end = Some((
                    event["reason"].as_str().map(str::to_owned),
                    event["sample_count"].as_u64(),
                ));
            }
            event_types.push(event_type.to_owned());
        }

        let live_delta = event_types
            .iter()
            .position(|event| event == "transcript_delta")
            .unwrap();
        let source_media = source_media_index.unwrap();
        let translated_media = translated_media_index.unwrap();
        let live_translation = event_types
            .iter()
            .position(|event| event == "translation_delta")
            .unwrap();
        let transcript = event_types
            .iter()
            .position(|event| event == "transcript")
            .unwrap();
        let translation = event_types
            .iter()
            .position(|event| event == "translation")
            .unwrap();
        assert_eq!(queued_tc_id.as_deref(), Some(tc_id.to_string().as_str()));
        assert!(live_delta < source_media);
        assert!(live_translation < source_media);
        assert!(source_media < transcript);
        assert!(translation < translated_media);
        assert_eq!(
            vad_end,
            Some((
                Some("silence".to_owned()),
                Some((20 * CLIENT_FRAME_SAMPLES) as u64)
            ))
        );
        assert!(event_types.iter().any(|event| event == "audio_start"));
        assert!(event_types.iter().any(|event| event == "audio_end"));
        assert!(
            !event_types
                .iter()
                .any(|event| event == "utterance_discarded")
        );
    }

    #[tokio::test]
    async fn consecutive_vad_segments_each_complete_translation_and_tts() {
        let directory = tempfile::tempdir().unwrap();
        let media = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let services = Arc::new(AppServices {
            transcriber: Arc::new(DemoTranscriber::new()),
            translator: Arc::new(DemoTranslator::new()),
            synthesizer: Arc::new(DemoSynthesizer::new()),
            backend_name: "demo",
        });
        let (input_tx, input_rx) = mpsc::channel(96);
        let (output_tx, mut output_rx) = mpsc::channel(512);
        let ids = [Uuid::new_v4(), Uuid::new_v4()];
        let pipeline = tokio::spawn(run_pipeline(
            services,
            None,
            media,
            PipelineIdentity {
                user_id: Uuid::new_v4(),
                room_id: Uuid::new_v4(),
            },
            input_rx,
            output_tx,
        ));

        for tc_id in ids {
            input_tx
                .send(PipelineInput::Event(ClientEvent::Start {
                    tc_id,
                    vad: Some(ClientVadStart {
                        engine: "silero-v6.2-lele".to_owned(),
                        sample_rate: INPUT_SAMPLE_RATE,
                        frame_samples: CLIENT_FRAME_SAMPLES,
                        pre_roll_samples: CLIENT_PRE_ROLL_SAMPLES,
                    }),
                    config: SessionConfig::default(),
                }))
                .await
                .unwrap();
            for _ in 0..20 {
                input_tx
                    .send(PipelineInput::Audio(
                        vec![0xe8, 0x03].repeat(CLIENT_FRAME_SAMPLES),
                    ))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            input_tx
                .send(PipelineInput::Event(ClientEvent::End {
                    tc_id,
                    is_silent_vad: false,
                    vad: Some(ClientVadEnd {
                        reason: VadEndReason::Silence,
                        sample_count: 20 * CLIENT_FRAME_SAMPLES,
                        speech_frames: Some(20),
                    }),
                }))
                .await
                .unwrap();
        }
        drop(input_tx);
        pipeline.await.unwrap().unwrap();

        let mut completed = std::collections::HashMap::<String, HashSet<String>>::new();
        while let Some(message) = output_rx.recv().await {
            let Message::Text(text) = message else {
                continue;
            };
            let event: Value = serde_json::from_str(&text).unwrap();
            let Some(utterance_id) = event["utterance_id"].as_str() else {
                continue;
            };
            let Some(event_type) = event["type"].as_str() else {
                continue;
            };
            if event_type == "media" {
                if event["source_audio_url"].is_string() {
                    completed
                        .entry(utterance_id.to_owned())
                        .or_default()
                        .insert("source_media".to_owned());
                }
                if event["translated_audio_url"].is_string() {
                    completed
                        .entry(utterance_id.to_owned())
                        .or_default()
                        .insert("translated_media".to_owned());
                }
            } else {
                completed
                    .entry(utterance_id.to_owned())
                    .or_default()
                    .insert(event_type.to_owned());
            }
        }

        for id in ids.map(|id| id.to_string()) {
            let events = completed.get(&id).unwrap();
            for expected in [
                "transcript_delta",
                "transcript",
                "translation_delta",
                "translation",
                "source_media",
                "translated_media",
                "audio_end",
            ] {
                assert!(
                    events.contains(expected),
                    "{id} missed {expected}: {events:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn discards_vad_silence_before_persistence_and_asr() {
        let directory = tempfile::tempdir().unwrap();
        let media = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let services = Arc::new(AppServices {
            transcriber: Arc::new(DemoTranscriber::new()),
            translator: Arc::new(DemoTranslator::new()),
            synthesizer: Arc::new(DemoSynthesizer::new()),
            backend_name: "demo",
        });
        let (input_tx, input_rx) = mpsc::channel(8);
        let (output_tx, mut output_rx) = mpsc::channel(64);
        let tc_id = Uuid::new_v4();
        let pipeline = tokio::spawn(run_pipeline(
            services,
            None,
            media,
            PipelineIdentity {
                user_id: Uuid::new_v4(),
                room_id: Uuid::new_v4(),
            },
            input_rx,
            output_tx,
        ));

        input_tx
            .send(PipelineInput::Event(ClientEvent::Start {
                tc_id,
                vad: None,
                config: SessionConfig::default(),
            }))
            .await
            .unwrap();
        input_tx
            .send(PipelineInput::Event(ClientEvent::End {
                tc_id,
                is_silent_vad: true,
                vad: None,
            }))
            .await
            .unwrap();
        drop(input_tx);
        pipeline.await.unwrap().unwrap();

        let mut event_types = Vec::new();
        while let Some(Message::Text(text)) = output_rx.recv().await {
            let event: Value = serde_json::from_str(&text).unwrap();
            if let Some(event_type) = event["type"].as_str() {
                event_types.push(event_type.to_owned());
            }
        }
        assert!(
            event_types
                .iter()
                .any(|event| event == "utterance_discarded")
        );
        assert!(!event_types.iter().any(|event| event == "media"));
        assert!(!event_types.iter().any(|event| event == "transcript"));
    }
}
