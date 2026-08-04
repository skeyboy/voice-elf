use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

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
            .unwrap_or_else(|_| "127.0.0.1:3001".to_owned())
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
                .map(resolve_workspace_path)
                .unwrap_or_else(|_| resolve_workspace_path("web/dist")),
            media_dir: env::var("VOICE_ELF_MEDIA_DIR")
                .map(resolve_workspace_path)
                .unwrap_or_else(|_| resolve_workspace_path("media")),
            backend_mode,
            asr: AsrConfig {
                binary: env::var("QWEN_ASR_BINARY")
                    .map(resolve_workspace_executable)
                    .unwrap_or_else(|_| PathBuf::from("qwen_asr")),
                model_dir: env::var("QWEN_ASR_MODEL_DIR")
                    .ok()
                    .map(resolve_workspace_path),
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
                    .map(resolve_workspace_executable)
                    .unwrap_or_else(|_| PathBuf::from("llama-cli")),
                model_path: env::var("LOCAL_LLM_MODEL_PATH")
                    .ok()
                    .map(resolve_workspace_path),
                threads: env::var("LOCAL_LLM_THREADS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(8),
            },
            tts: TtsConfig {
                kokoro_model_dir: env::var("TTS_KOKORO_MODEL_DIR")
                    .map(resolve_workspace_path)
                    .unwrap_or_else(|_| {
                        resolve_workspace_path(".local/models/tts/kokoro-int8-multi-lang-v1_1")
                    }),
                supertonic_model_dir: env::var("TTS_SUPERTONIC_MODEL_DIR")
                    .map(resolve_workspace_path)
                    .unwrap_or_else(|_| {
                        resolve_workspace_path(
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

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("server crate must be inside the workspace")
}

fn resolve_workspace_path(path: impl Into<PathBuf>) -> PathBuf {
    let path = path.into();
    if path.is_absolute() {
        path
    } else {
        workspace_root().join(path)
    }
}

fn resolve_workspace_executable(path: impl Into<PathBuf>) -> PathBuf {
    let path = path.into();
    if path.is_absolute() || path.components().count() == 1 {
        path
    } else {
        workspace_root().join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_resource_paths_from_the_workspace_root() {
        assert_eq!(
            resolve_workspace_path(".local/models/qwen3-asr-0.6b"),
            workspace_root().join(".local/models/qwen3-asr-0.6b")
        );
        assert_eq!(
            resolve_workspace_executable(".local/bin/qwen_asr"),
            workspace_root().join(".local/bin/qwen_asr")
        );
    }

    #[test]
    fn preserves_absolute_paths_and_path_commands() {
        let absolute = workspace_root().join("models/asr");
        assert_eq!(resolve_workspace_path(&absolute), absolute);
        assert_eq!(
            resolve_workspace_executable("qwen_asr"),
            PathBuf::from("qwen_asr")
        );
    }
}
