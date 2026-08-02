use serde::{Deserialize, Serialize};

pub const INPUT_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Configure(SessionConfig),
    Start,
    SpeechStart,
    SpeechEnd,
    Flush,
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
    "ryan".to_owned()
}

fn default_max_utterance_seconds() -> u32 {
    20
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
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
    },
    UtteranceQueued {
        utterance_id: String,
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
        sample_rate: u32,
        sample_count: usize,
    },
    AudioEnd {
        utterance_id: String,
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
