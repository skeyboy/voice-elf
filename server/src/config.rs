use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub bind: SocketAddr,
    pub web_dist: PathBuf,
    pub media_dir: PathBuf,
    pub backend_mode: BackendMode,
    pub asr: AsrConfig,
    pub translator: TranslatorConfig,
    pub tts: TtsConfig,
    pub inference_timeout: Duration,
    pub database_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendMode {
    Demo,
    Local,
}

#[derive(Clone, Debug)]
pub struct AsrConfig {
    pub binary: PathBuf,
    pub model_dir: Option<PathBuf>,
    pub stream_unfixed_chunks: usize,
    pub stream_max_new_tokens: usize,
    pub encoder_window_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct TranslatorConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub binary: PathBuf,
    pub model_path: Option<PathBuf>,
    pub threads: usize,
}

#[derive(Clone, Debug)]
pub struct TtsConfig {
    pub kokoro_model_dir: PathBuf,
    pub supertonic_model_dir: PathBuf,
    pub threads: usize,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let bind = env::var("VOICE_ELF_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3000".to_owned())
            .parse()
            .context("VOICE_ELF_BIND must be a socket address")?;

        let backend_mode = match env::var("VOICE_ELF_BACKEND")
            .unwrap_or_else(|_| "demo".to_owned())
            .to_ascii_lowercase()
            .as_str()
        {
            "demo" => BackendMode::Demo,
            "local" => BackendMode::Local,
            value => anyhow::bail!("VOICE_ELF_BACKEND must be 'demo' or 'local', got '{value}'"),
        };

        let timeout_seconds = env::var("VOICE_ELF_INFERENCE_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(120);

        Ok(Self {
            bind,
            web_dist: env::var("VOICE_ELF_WEB_DIST")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("web/dist")),
            media_dir: env::var("VOICE_ELF_MEDIA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("media")),
            backend_mode,
            asr: AsrConfig {
                binary: env::var("QWEN_ASR_BINARY")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("qwen_asr")),
                model_dir: env::var("QWEN_ASR_MODEL_DIR").ok().map(PathBuf::from),
                stream_unfixed_chunks: env::var("QWEN_ASR_STREAM_UNFIXED_CHUNKS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
                stream_max_new_tokens: env::var("QWEN_ASR_STREAM_MAX_NEW_TOKENS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(32),
                encoder_window_seconds: env::var("QWEN_ASR_ENCODER_WINDOW_SECONDS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(4),
            },
            translator: TranslatorConfig {
                base_url: env::var("LOCAL_LLM_BASE_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".to_owned()),
                model: env::var("LOCAL_LLM_MODEL").unwrap_or_else(|_| "qwen3:4b".to_owned()),
                api_key: env::var("LOCAL_LLM_API_KEY").ok(),
                binary: env::var("LOCAL_LLM_BINARY")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("llama-cli")),
                model_path: env::var("LOCAL_LLM_MODEL_PATH").ok().map(PathBuf::from),
                threads: env::var("LOCAL_LLM_THREADS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(8),
            },
            tts: TtsConfig {
                kokoro_model_dir: env::var("TTS_KOKORO_MODEL_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        PathBuf::from(".local/models/tts/kokoro-int8-multi-lang-v1_1")
                    }),
                supertonic_model_dir: env::var("TTS_SUPERTONIC_MODEL_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        PathBuf::from(
                            ".local/models/tts/sherpa-onnx-supertonic-3-tts-int8-2026-05-11",
                        )
                    }),
                threads: env::var("TTS_THREADS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(2),
            },
            inference_timeout: Duration::from_secs(timeout_seconds),
            database_url: env::var("DATABASE_URL").ok().filter(|url| !url.is_empty()),
        })
    }

    pub fn validate_local(&self) -> Result<()> {
        if self.backend_mode != BackendMode::Local {
            return Ok(());
        }
        let asr_model = self
            .asr
            .model_dir
            .as_ref()
            .context("QWEN_ASR_MODEL_DIR is required when VOICE_ELF_BACKEND=local")?;
        if !asr_model.is_dir() {
            anyhow::bail!(
                "QWEN_ASR_MODEL_DIR is not a directory: {}",
                asr_model.display()
            );
        }
        if let Some(model) = &self.translator.model_path
            && !model.is_file()
        {
            anyhow::bail!("LOCAL_LLM_MODEL_PATH is not a file: {}", model.display());
        }
        Ok(())
    }
}
