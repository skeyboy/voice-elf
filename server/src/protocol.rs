use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const INPUT_SAMPLE_RATE: u32 = 16_000;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct SpeakerIdentity {
    pub user_id: Option<Uuid>,
    pub username: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscriptionSegment {
    pub start: f64,
    pub end: f64,
    pub speaker: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoomMemberState {
    pub user_id: Uuid,
    pub username: String,
    pub is_owner: bool,
    pub is_muted: bool,
    pub is_online: bool,
    pub is_speaking: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Configure(SessionConfig),
    Start {
        tc_id: Uuid,
        #[serde(default)]
        vad: Option<ClientVadStart>,
        #[serde(flatten)]
        config: SessionConfig,
    },
    End {
        tc_id: Uuid,
        #[serde(default)]
        is_silent_vad: bool,
        #[serde(default)]
        vad: Option<ClientVadEnd>,
    },
    Flush,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClientVadStart {
    pub engine: String,
    pub sample_rate: u32,
    pub frame_samples: usize,
    pub pre_roll_samples: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClientVadEnd {
    pub reason: VadEndReason,
    pub sample_count: usize,
    #[serde(default)]
    pub speech_frames: Option<usize>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VadEndReason {
    Silence,
    MaxDuration,
    Manual,
    Superseded,
    Silent,
    ServerLimit,
    Unknown,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_source_language")]
    pub source_language: String,
    #[serde(default = "default_target_language")]
    pub target_language: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_max_utterance_seconds")]
    pub max_utterance_seconds: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            source_language: default_source_language(),
            target_language: default_target_language(),
            voice: default_voice(),
            max_utterance_seconds: default_max_utterance_seconds(),
        }
    }
}

fn default_source_language() -> String {
    "auto".to_owned()
}

fn default_target_language() -> String {
    "zh".to_owned()
}

fn default_voice() -> String {
    "F1".to_owned()
}

fn default_max_utterance_seconds() -> u32 {
    20
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    RoomSubscribed {
        room_id: String,
        can_publish: bool,
        user_id: Uuid,
        backend: String,
    },
    RoomMembers {
        members: Vec<RoomMemberState>,
    },
    Ready {
        session_id: String,
        room_id: String,
        backend: String,
        input_sample_rate: u32,
    },
    Configured {
        source_language: String,
        target_language: String,
        voice: String,
        max_utterance_seconds: u32,
    },
    State {
        phase: PipelinePhase,
        utterance_id: Option<String>,
    },
    Vad {
        active: bool,
        level: f32,
        utterance_id: Option<String>,
        reason: Option<VadEndReason>,
        sample_count: usize,
    },
    UtteranceQueued {
        utterance_id: String,
        tc_id: String,
    },
    UtteranceSpeakers {
        utterance_id: String,
        speakers: Vec<SpeakerIdentity>,
    },
    UtteranceDiscarded {
        utterance_id: String,
        tc_id: String,
        reason: String,
    },
    RecognitionFailed {
        utterance_id: String,
        message: String,
    },
    ProcessingFailed {
        utterance_id: String,
        stage: ProcessingStage,
        message: String,
    },
    Transcript {
        utterance_id: String,
        text: String,
        language: String,
    },
    TranscriptDelta {
        utterance_id: String,
        delta: String,
        text: String,
        language: String,
        done: bool,
    },
    TranscriptRefinement {
        utterance_id: String,
        engine: String,
        status: RefinementStatus,
        text: Option<String>,
        language: Option<String>,
        segments: Vec<TranscriptionSegment>,
        message: Option<String>,
    },
    Translation {
        utterance_id: String,
        source_text: String,
        translated_text: String,
        source_language: String,
        target_language: String,
    },
    TranslationDelta {
        utterance_id: String,
        delta: String,
        text: String,
        target_language: String,
        done: bool,
    },
    Media {
        utterance_id: String,
        source_audio_url: Option<String>,
        translated_audio_url: Option<String>,
    },
    AudioStart {
        utterance_id: String,
        engine: String,
        codec: AudioCodec,
        sample_rate: u32,
        channels: u16,
        sample_count: Option<usize>,
    },
    AudioEnd {
        utterance_id: String,
        sample_count: usize,
    },
    Latency {
        utterance_id: String,
        latency: LatencyReport,
    },
    Warning {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    PcmS16le,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelinePhase {
    Listening,
    Speech,
    Transcribing,
    Translating,
    Synthesizing,
    Playing,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStage {
    Translation,
    Tts,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefinementStatus {
    Processing,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct LatencyReport {
    pub vad_ms: u64,
    pub stt_ms: u64,
    pub translation_ms: u64,
    pub tts_ms: u64,
    pub total_ms: u64,
    pub audio_ms: u64,
    pub t0_unix_ms: u64,
    pub t1_unix_ms: u64,
    pub t2_unix_ms: u64,
    pub t3_unix_ms: u64,
    pub t4_unix_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_primary_voice_profile() {
        assert_eq!(SessionConfig::default().voice, "F1");
    }

    #[test]
    fn parses_traceable_segment_boundaries() {
        let tc_id = Uuid::new_v4();
        let start = serde_json::from_value::<ClientEvent>(serde_json::json!({
            "type": "start",
            "tc_id": tc_id,
            "vad": {
                "engine": "silero-v6.2-lele",
                "sample_rate": 16000,
                "frame_samples": 512,
                "pre_roll_samples": 3584
            },
            "source_language": "en",
            "target_language": "zh",
            "voice": "ryan",
            "max_utterance_seconds": 20
        }))
        .unwrap();
        assert!(matches!(start, ClientEvent::Start { tc_id: id, .. } if id == tc_id));

        let end = serde_json::from_value::<ClientEvent>(serde_json::json!({
            "type": "end",
            "tc_id": tc_id,
            "is_silent_vad": true,
            "vad": { "reason": "silent", "sample_count": 0 }
        }))
        .unwrap();
        assert!(matches!(
            end,
            ClientEvent::End {
                tc_id: id,
                is_silent_vad: true,
                vad: Some(ClientVadEnd {
                    reason: VadEndReason::Silent,
                    sample_count: 0,
                    speech_frames: None,
                })
            } if id == tc_id
        ));
    }

    #[test]
    fn serializes_room_subscription_role() {
        let value = serde_json::to_value(ServerEvent::RoomSubscribed {
            room_id: "room-1".to_owned(),
            can_publish: false,
            user_id: Uuid::nil(),
            backend: "local".to_owned(),
        })
        .unwrap();
        assert_eq!(value["type"], "room_subscribed");
        assert_eq!(value["room_id"], "room-1");
        assert_eq!(value["can_publish"], false);
        assert_eq!(value["backend"], "local");
    }
}
