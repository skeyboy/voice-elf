mod asr;
mod translator;
mod tts;

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::config::{AppConfig, BackendMode};

pub use asr::{DemoTranscriber, NoSpeechDetected, QwenAsrTranscriber};
pub use translator::{DemoTranslator, LlamaCppTranslator, LocalLlmTranslator};
pub use tts::{DemoSynthesizer, QwenTtsSynthesizer};

#[derive(Clone, Debug)]
pub struct Transcription {
    pub text: String,
    pub language: String,
}

#[derive(Clone, Debug)]
pub struct SynthesizedAudio {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
}

pub struct LiveTranscription {
    audio: Option<mpsc::UnboundedSender<Vec<i16>>>,
    task: Option<JoinHandle<Result<Transcription>>>,
}

impl LiveTranscription {
    pub(super) fn new(
        audio: mpsc::UnboundedSender<Vec<i16>>,
        task: JoinHandle<Result<Transcription>>,
    ) -> Self {
        Self {
            audio: Some(audio),
            task: Some(task),
        }
    }

    pub fn push(&self, pcm: &[i16]) -> Result<()> {
        self.audio
            .as_ref()
            .context("live ASR input is already closed")?
            .send(pcm.to_vec())
            .map_err(|_| anyhow::anyhow!("live ASR process stopped"))
    }

    pub async fn finish(mut self) -> Result<Transcription> {
        self.audio.take();
        self.task
            .take()
            .context("live ASR result task is missing")?
            .await
            .context("live ASR result task failed")?
    }
}

impl Drop for LiveTranscription {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn start_live(
        &self,
        _source_language: &str,
        _updates: mpsc::UnboundedSender<String>,
    ) -> Result<Option<LiveTranscription>> {
        Ok(None)
    }

    async fn transcribe_streaming(
        &self,
        pcm: &[i16],
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Transcription>;
}

#[async_trait]
pub trait Translator: Send + Sync {
    async fn translate_streaming(
        &self,
        text: &str,
        source_language: &str,
        target_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<String>;
}

#[async_trait]
pub trait Synthesizer: Send + Sync {
    async fn synthesize(&self, text: &str, language: &str, voice: &str)
    -> Result<SynthesizedAudio>;
}

#[derive(Clone)]
pub struct AppServices {
    pub transcriber: Arc<dyn Transcriber>,
    pub translator: Arc<dyn Translator>,
    pub synthesizer: Arc<dyn Synthesizer>,
    pub backend_name: &'static str,
}

impl AppServices {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        config.validate_local()?;
        match config.backend_mode {
            BackendMode::Demo => Ok(Self {
                transcriber: Arc::new(DemoTranscriber::new()),
                translator: Arc::new(DemoTranslator::new()),
                synthesizer: Arc::new(DemoSynthesizer::new()),
                backend_name: "demo",
            }),
            BackendMode::Local => {
                let translator: Arc<dyn Translator> = if config.translator.model_path.is_some() {
                    Arc::new(LlamaCppTranslator::new(
                        config.translator.clone(),
                        config.inference_timeout,
                    )?)
                } else {
                    Arc::new(LocalLlmTranslator::new(
                        config.translator.clone(),
                        config.inference_timeout,
                    )?)
                };
                Ok(Self {
                    transcriber: Arc::new(QwenAsrTranscriber::new(
                        config.asr.clone(),
                        config.inference_timeout,
                    )?),
                    translator,
                    synthesizer: Arc::new(QwenTtsSynthesizer::new(
                        config.tts.clone(),
                        config.inference_timeout,
                    )?),
                    backend_name: "local",
                })
            }
        }
    }
}

pub(crate) async fn short_demo_delay(duration: Duration) {
    if !cfg!(test) {
        tokio::time::sleep(duration).await;
    }
}

pub(crate) fn language_name(code: &str) -> &'static str {
    match code.to_ascii_lowercase().as_str() {
        "auto" => "Auto",
        "zh" | "chinese" => "Chinese",
        "en" | "english" => "English",
        "ja" | "japanese" => "Japanese",
        "ko" | "korean" => "Korean",
        "fr" | "french" => "French",
        "de" | "german" => "German",
        "es" | "spanish" => "Spanish",
        "it" | "italian" => "Italian",
        "pt" | "portuguese" => "Portuguese",
        "ru" | "russian" => "Russian",
        _ => "English",
    }
}
