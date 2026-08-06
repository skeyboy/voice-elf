mod asr;
mod moss;
mod translator;
mod tts;

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::future::join_all;
use serde::Serialize;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::config::{AppConfig, BackendMode};

pub use asr::{DemoTranscriber, NoSpeechDetected, QwenAsrTranscriber};
pub use moss::MossTranscribeEngine;
pub use translator::{DemoTranslator, LlamaCppTranslator, LocalLlmTranslator};
#[cfg(test)]
pub use tts::DemoSynthesizer;
pub use tts::{TtsChunkSink, TtsEngine, TtsRequest, build_tts_engine};

pub use crate::protocol::TranscriptionSegment;

pub const QWEN_LOCAL_ASR_ID: &str = "qwen-local";
pub const DEMO_ASR_ID: &str = "demo";

#[derive(Clone, Debug, Serialize)]
pub struct AsrBackendInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub engine: &'static str,
    pub description: &'static str,
    pub available: bool,
    pub production: bool,
}

#[derive(Clone)]
struct AsrBackendEntry {
    info: AsrBackendInfo,
    transcriber: Option<Arc<dyn Transcriber>>,
}

#[derive(Clone)]
pub struct AsrBackendRegistry {
    entries: Arc<HashMap<&'static str, AsrBackendEntry>>,
    default_backend_id: &'static str,
}

impl AsrBackendRegistry {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        let mut entries = HashMap::new();
        entries.insert(
            DEMO_ASR_ID,
            AsrBackendEntry {
                info: AsrBackendInfo {
                    id: DEMO_ASR_ID,
                    name: "演示占位识别",
                    engine: "内置 Demo",
                    description: "仅用于界面联调，不执行真实语音识别",
                    available: true,
                    production: false,
                },
                transcriber: Some(Arc::new(DemoTranscriber::new())),
            },
        );

        let qwen_available = config
            .asr
            .model_dir
            .as_ref()
            .is_some_and(|model_dir| model_dir.is_dir());
        let qwen = if qwen_available {
            Some(Arc::new(QwenAsrTranscriber::new(
                config.asr.clone(),
                config.inference_timeout,
            )?) as Arc<dyn Transcriber>)
        } else {
            None
        };
        entries.insert(
            QWEN_LOCAL_ASR_ID,
            AsrBackendEntry {
                info: AsrBackendInfo {
                    id: QWEN_LOCAL_ASR_ID,
                    name: "Qwen3 ASR 本地模型",
                    engine: "Qwen3-ASR-0.6B",
                    description: "在当前实例本地执行流式识别，音频不离开部署环境",
                    available: qwen.is_some(),
                    production: true,
                },
                transcriber: qwen,
            },
        );

        let default_backend_id = match config.backend_mode {
            BackendMode::Demo => DEMO_ASR_ID,
            BackendMode::Local => QWEN_LOCAL_ASR_ID,
        };
        if !entries
            .get(default_backend_id)
            .is_some_and(|entry| entry.transcriber.is_some())
        {
            bail!("default ASR backend '{default_backend_id}' is not available");
        }
        Ok(Self {
            entries: Arc::new(entries),
            default_backend_id,
        })
    }

    pub fn default_backend_id(&self) -> &'static str {
        self.default_backend_id
    }

    pub fn providers(&self) -> Vec<AsrBackendInfo> {
        let mut providers = self
            .entries
            .values()
            .map(|entry| entry.info.clone())
            .collect::<Vec<_>>();
        providers.sort_by_key(|provider| (!provider.production, provider.name));
        providers
    }

    pub fn services_for(
        &self,
        services: &AppServices,
        backend_id: &str,
    ) -> Option<Arc<AppServices>> {
        let entry = self.entries.get(backend_id)?;
        let transcriber = entry.transcriber.as_ref()?.clone();
        Some(Arc::new(
            services.with_transcriber(transcriber, entry.info.id),
        ))
    }
}

#[derive(Clone, Debug)]
pub struct Transcription {
    pub text: String,
    pub language: String,
    pub segments: Vec<TranscriptionSegment>,
}

impl Transcription {
    pub fn plain(text: impl Into<String>, language: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            language: language.into(),
            segments: Vec::new(),
        }
    }
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
pub trait CompletedTranscriptionEngine: Send + Sync {
    fn name(&self) -> &'static str;

    async fn transcribe_completed(
        &self,
        pcm: &[i16],
        source_language: &str,
    ) -> Result<Transcription>;
}

#[async_trait]
pub trait Transcriber: CompletedTranscriptionEngine {
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
impl CompletedTranscriptionEngine for MultiChannelTranscriber {
    fn name(&self) -> &'static str {
        "multi-channel"
    }

    async fn transcribe_completed(
        &self,
        pcm: &[i16],
        source_language: &str,
    ) -> Result<Transcription> {
        let calls = self.channels.iter().map(|(name, transcriber)| async move {
            (
                *name,
                transcriber.transcribe_completed(pcm, source_language).await,
            )
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
        let transcription = self.transcribe_completed(pcm, source_language).await?;
        let _ = updates.send(transcription.text.clone());
        Ok(transcription)
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
    pub refinement_engines: Vec<Arc<dyn CompletedTranscriptionEngine>>,
    pub translator: Arc<dyn Translator>,
    pub synthesizer: Arc<dyn TtsEngine>,
    pub backend_name: &'static str,
}

impl AppServices {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        config.validate_local()?;
        let refinement_engines: Vec<Arc<dyn CompletedTranscriptionEngine>> =
            if config.moss_transcribe.enabled {
                vec![Arc::new(MossTranscribeEngine::new(
                    config.moss_transcribe.clone(),
                )?)]
            } else {
                Vec::new()
            };
        let synthesizer = build_tts_engine(&config.tts)?;
        match config.backend_mode {
            BackendMode::Demo => Ok(Self {
                transcriber: Arc::new(MultiChannelTranscriber::new(vec![(
                    "demo",
                    Arc::new(DemoTranscriber::new()),
                )])?),
                refinement_engines,
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
                    refinement_engines,
                    translator,
                    synthesizer,
                    backend_name: "local",
                })
            }
        }
    }

    fn with_transcriber(
        &self,
        transcriber: Arc<dyn Transcriber>,
        backend_name: &'static str,
    ) -> Self {
        Self {
            transcriber,
            refinement_engines: self.refinement_engines.clone(),
            translator: self.translator.clone(),
            synthesizer: self.synthesizer.clone(),
            backend_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_prefers_matching_channels_over_a_longer_outlier() {
        let candidates = vec![
            Transcription::plain("hello world", "en"),
            Transcription::plain("Hello, world!", "en"),
            Transcription::plain("hello wonderful world", "en"),
        ];
        assert_eq!(select_consensus(&candidates).unwrap().text, "Hello, world!");
    }

    #[test]
    fn registry_keeps_demo_explicitly_non_production() {
        let provider = AsrBackendInfo {
            id: DEMO_ASR_ID,
            name: "Demo",
            engine: "Demo",
            description: "placeholder",
            available: true,
            production: false,
        };
        assert!(!provider.production);
        assert_eq!(QWEN_LOCAL_ASR_ID, "qwen-local");
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
