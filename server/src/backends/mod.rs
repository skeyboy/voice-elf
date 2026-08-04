mod asr;
mod translator;
mod tts;

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::future::join_all;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::config::{AppConfig, BackendMode};

pub use asr::{DemoTranscriber, NoSpeechDetected, QwenAsrTranscriber};
pub use translator::{DemoTranslator, LlamaCppTranslator, LocalLlmTranslator};
#[cfg(test)]
pub use tts::DemoSynthesizer;
pub use tts::{SherpaOnnxSynthesizer, Synthesizer};

#[derive(Clone, Debug)]
pub struct Transcription {
    pub text: String,
    pub language: String,
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
            .map_err(|_| anyhow!("live ASR process stopped"))
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

    async fn refine_transcription(
        &self,
        _pcm: &[i16],
        _source_language: &str,
        primary: Transcription,
    ) -> Result<Transcription> {
        Ok(primary)
    }
}

pub struct MultiChannelTranscriber {
    channels: Vec<(&'static str, Arc<dyn Transcriber>)>,
}

impl MultiChannelTranscriber {
    pub fn new(channels: Vec<(&'static str, Arc<dyn Transcriber>)>) -> Result<Self> {
        if channels.is_empty() {
            bail!("at least one ASR channel is required");
        }
        Ok(Self { channels })
    }
}

#[async_trait]
impl Transcriber for MultiChannelTranscriber {
    async fn start_live(
        &self,
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Option<LiveTranscription>> {
        self.channels[0]
            .1
            .start_live(source_language, updates)
            .await
    }

    async fn transcribe_streaming(
        &self,
        pcm: &[i16],
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Transcription> {
        let calls = self
            .channels
            .iter()
            .enumerate()
            .map(|(index, (name, transcriber))| {
                let channel_updates = if index == 0 {
                    updates.clone()
                } else {
                    mpsc::unbounded_channel().0
                };
                async move {
                    (
                        *name,
                        transcriber
                            .transcribe_streaming(pcm, source_language, channel_updates)
                            .await,
                    )
                }
            });
        let mut candidates = Vec::new();
        let mut failures = Vec::new();
        for (name, result) in join_all(calls).await {
            match result {
                Ok(transcription) if !transcription.text.trim().is_empty() => {
                    candidates.push(transcription);
                }
                Ok(_) => failures.push(format!("{name}: empty transcript")),
                Err(error) => {
                    tracing::warn!(channel = name, %error, "ASR channel failed");
                    failures.push(format!("{name}: {error}"));
                }
            }
        }
        select_consensus(&candidates)
            .cloned()
            .ok_or_else(|| anyhow!("all ASR channels failed: {}", failures.join("; ")))
    }

    async fn refine_transcription(
        &self,
        pcm: &[i16],
        source_language: &str,
        primary: Transcription,
    ) -> Result<Transcription> {
        if self.channels.len() == 1 {
            return Ok(primary);
        }
        let calls = self.channels.iter().skip(1).map(|(name, transcriber)| {
            let updates = mpsc::unbounded_channel().0;
            async move {
                (
                    *name,
                    transcriber
                        .transcribe_streaming(pcm, source_language, updates)
                        .await,
                )
            }
        });
        let mut candidates = vec![primary];
        for (name, result) in join_all(calls).await {
            match result {
                Ok(transcription) if !transcription.text.trim().is_empty() => {
                    candidates.push(transcription);
                }
                Ok(_) => tracing::warn!(channel = name, "ASR refinement returned empty text"),
                Err(error) => tracing::warn!(channel = name, %error, "ASR refinement failed"),
            }
        }
        Ok(select_consensus(&candidates)
            .cloned()
            .expect("the primary ASR result is always present"))
    }
}

fn select_consensus(candidates: &[Transcription]) -> Option<&Transcription> {
    candidates.iter().max_by_key(|candidate| {
        let key = normalize_transcript(&candidate.text);
        let votes = candidates
            .iter()
            .filter(|other| normalize_transcript(&other.text) == key)
            .count();
        (votes, candidate.text.chars().count())
    })
}

fn normalize_transcript(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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
        let synthesizer: Arc<dyn Synthesizer> =
            Arc::new(SherpaOnnxSynthesizer::new(config.tts.clone())?);
        match config.backend_mode {
            BackendMode::Demo => Ok(Self {
                transcriber: Arc::new(MultiChannelTranscriber::new(vec![(
                    "demo",
                    Arc::new(DemoTranscriber::new()),
                )])?),
                translator: Arc::new(DemoTranslator::new()),
                synthesizer,
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
                    transcriber: Arc::new(MultiChannelTranscriber::new(vec![(
                        "qwen",
                        Arc::new(QwenAsrTranscriber::new(
                            config.asr.clone(),
                            config.inference_timeout,
                        )?),
                    )])?),
                    translator,
                    synthesizer,
                    backend_name: "local",
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_prefers_matching_channels_over_a_longer_outlier() {
        let candidates = vec![
            Transcription {
                text: "hello world".to_owned(),
                language: "en".to_owned(),
            },
            Transcription {
                text: "Hello, world!".to_owned(),
                language: "en".to_owned(),
            },
            Transcription {
                text: "hello wonderful world".to_owned(),
                language: "en".to_owned(),
            },
        ];
        assert_eq!(select_consensus(&candidates).unwrap().text, "Hello, world!");
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
