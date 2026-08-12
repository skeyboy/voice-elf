mod asr;
mod fun_asr;
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
pub use fun_asr::{FunAsrRuntimeStatus, FunAsrTranscriber};
pub use moss::MossTranscribeEngine;
pub use translator::{DemoTranslator, LlamaCppTranslator, LocalLlmTranslator};
#[cfg(test)]
pub use tts::DemoSynthesizer;
pub use tts::{
    IndexTtsEngine, QwenTtsEngine, TtsChunkSink, TtsEngine, TtsRequest, build_tts_engine,
};

pub use crate::protocol::TranscriptionSegment;

pub const QWEN_LOCAL_ASR_ID: &str = "qwen-local";
pub const FUN_ASR_ID: &str = "funasr-streaming";
pub const DEMO_ASR_ID: &str = "demo";
pub const LOCAL_TTS_ID: &str = "local-fallback";
pub const INDEX_TTS_ID: &str = "index-tts2";
pub const QWEN_TTS_ID: &str = "qwen3-tts";

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
    fun_asr: Option<Arc<FunAsrTranscriber>>,
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

        let fun_asr = if config.fun_asr.enabled {
            Some(Arc::new(FunAsrTranscriber::new(config.fun_asr.clone())?))
        } else {
            None
        };
        entries.insert(
            FUN_ASR_ID,
            AsrBackendEntry {
                info: AsrBackendInfo {
                    id: FUN_ASR_ID,
                    name: "FunASR Paraformer 流式服务",
                    engine: "FunASR 2-pass WebSocket",
                    description: "面向中文实时识别，在线增量输出并在句末用离线模型纠错",
                    available: fun_asr.is_some(),
                    production: true,
                },
                transcriber: fun_asr
                    .as_ref()
                    .map(|engine| engine.clone() as Arc<dyn Transcriber>),
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
            BackendMode::Local if qwen_available => QWEN_LOCAL_ASR_ID,
            BackendMode::Local => FUN_ASR_ID,
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
            fun_asr,
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

    pub fn is_available(&self, backend_id: &str) -> bool {
        self.entries
            .get(backend_id)
            .is_some_and(|entry| entry.transcriber.is_some())
    }

    pub async fn fun_asr_runtime_status(&self) -> FunAsrRuntimeStatus {
        match &self.fun_asr {
            Some(engine) => engine.runtime_status().await,
            None => FunAsrRuntimeStatus::disabled(),
        }
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

#[derive(Clone, Debug, Serialize)]
pub struct TtsBackendInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub engine: &'static str,
    pub description: &'static str,
    pub available: bool,
    pub production: bool,
    pub voice_clone: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TtsVoiceInfo {
    pub id: String,
    pub name: String,
    pub group: String,
    pub description: String,
    pub languages: Vec<String>,
}

#[derive(Clone)]
struct TtsBackendEntry {
    info: TtsBackendInfo,
    synthesizer: Option<Arc<dyn TtsEngine>>,
    voices: Vec<TtsVoiceInfo>,
}

#[derive(Clone)]
pub struct TtsBackendRegistry {
    entries: Arc<HashMap<&'static str, TtsBackendEntry>>,
    default_backend_id: &'static str,
}

impl TtsBackendRegistry {
    pub fn from_config(config: &AppConfig, local: Arc<dyn TtsEngine>) -> Result<Self> {
        let mut entries = HashMap::new();
        let local_voice_clone = local.supports_voice_clone();
        entries.insert(
            LOCAL_TTS_ID,
            TtsBackendEntry {
                info: TtsBackendInfo {
                    id: LOCAL_TTS_ID,
                    name: "本地自动回退",
                    engine: "MOSS Nano / Kokoro / Supertonic",
                    description: "按语言和音色能力自动选择当前本机已配置的 TTS 引擎",
                    available: true,
                    production: true,
                    voice_clone: local_voice_clone,
                },
                synthesizer: Some(local),
                voices: local_tts_voices(config),
            },
        );
        // Keep the HTTP adapter registered so an administrator can install and
        // start the local sidecar without restarting the main service.
        let index = Some(
            Arc::new(IndexTtsEngine::new(config.tts.index_tts.clone())?) as Arc<dyn TtsEngine>
        );
        entries.insert(
            INDEX_TTS_ID,
            TtsBackendEntry {
                info: TtsBackendInfo {
                    id: INDEX_TTS_ID,
                    name: "IndexTTS2",
                    engine: "Bilibili IndexTTS2",
                    description: "通过参考音频执行中英文零样本音色克隆与高表现力语音合成",
                    available: false,
                    production: true,
                    voice_clone: true,
                },
                synthesizer: index,
                voices: index_tts_voices(config),
            },
        );
        let qwen = if config.tts.qwen_tts.enabled {
            Some(Arc::new(QwenTtsEngine::new(config.tts.qwen_tts.clone())?) as Arc<dyn TtsEngine>)
        } else {
            None
        };
        entries.insert(
            QWEN_TTS_ID,
            TtsBackendEntry {
                info: TtsBackendInfo {
                    id: QWEN_TTS_ID,
                    name: "Qwen3-TTS",
                    engine: "Qwen3-TTS CustomVoice / vLLM-Omni",
                    description: "通过 OpenAI 兼容语音接口提供十语种低延迟语音合成",
                    available: qwen.is_some(),
                    production: true,
                    voice_clone: false,
                },
                synthesizer: qwen,
                voices: qwen_tts_voices(),
            },
        );
        Ok(Self {
            entries: Arc::new(entries),
            default_backend_id: LOCAL_TTS_ID,
        })
    }

    pub fn default_backend_id(&self) -> &'static str {
        self.default_backend_id
    }

    pub fn providers(&self) -> Vec<TtsBackendInfo> {
        let mut providers = self
            .entries
            .values()
            .map(|entry| entry.info.clone())
            .collect::<Vec<_>>();
        providers.sort_by_key(|provider| provider.name);
        providers
    }

    pub fn services_for(
        &self,
        services: &AppServices,
        backend_id: &str,
    ) -> Option<Arc<AppServices>> {
        let entry = self.entries.get(backend_id)?;
        let synthesizer = entry.synthesizer.as_ref()?.clone();
        Some(Arc::new(services.with_synthesizer(synthesizer)))
    }

    pub fn voices_for(&self, backend_id: &str) -> Option<Vec<TtsVoiceInfo>> {
        self.entries
            .get(backend_id)
            .map(|entry| entry.voices.clone())
    }
}

fn local_tts_voices(config: &AppConfig) -> Vec<TtsVoiceInfo> {
    const VOICES: &[(&str, &str, &str, &str, &[&str])] = &[
        (
            "F1",
            "模思中文",
            "中文参考声",
            "清晰自然的中文参考声",
            &["zh"],
        ),
        (
            "ZH_GENTLE",
            "温柔晚安",
            "中文参考声",
            "温柔舒缓的中文女声",
            &["zh"],
        ),
        (
            "ZH_TAIWAN",
            "台湾腔",
            "中文参考声",
            "轻松自然的台湾口语",
            &["zh"],
        ),
        (
            "M1",
            "京味胡同",
            "中文参考声",
            "具有京味特征的中文男声",
            &["zh"],
        ),
        (
            "ZH_LECTURE",
            "文化讲述",
            "中文参考声",
            "适合正式内容的讲述声",
            &["zh"],
        ),
        (
            "ZH_MONOLOGUE",
            "沉稳独白",
            "中文参考声",
            "沉稳自然的独白声",
            &["zh"],
        ),
        (
            "EN_MOSS",
            "OpenMOSS English",
            "English voices",
            "Clear English presentation voice",
            &["en"],
        ),
        (
            "EN_LECTURE",
            "English Lecture",
            "English voices",
            "Measured English lecture voice",
            &["en"],
        ),
        (
            "EN_NEWS",
            "English News",
            "English voices",
            "English broadcast news voice",
            &["en"],
        ),
        (
            "EN_GENTLE",
            "Gentle English",
            "English voices",
            "Gentle English reminder voice",
            &["en"],
        ),
        (
            "EN_EXPRESSIVE",
            "Expressive English",
            "English voices",
            "Expressive English speech voice",
            &["en"],
        ),
        (
            "EN_NARRATION",
            "English Narration",
            "English voices",
            "Calm English narration voice",
            &["en"],
        ),
        (
            "JA_NEWS",
            "ニュース",
            "日本語音声",
            "ニュース読みの日本語参考音声",
            &["ja"],
        ),
    ];
    let configured = &config.tts.moss_nano.voice_map;
    let mut voices = VOICES
        .iter()
        .filter(|(id, ..)| !config.tts.moss_nano.enabled || configured.contains_key(*id))
        .map(|(id, name, group, description, languages)| TtsVoiceInfo {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            group: (*group).to_owned(),
            description: (*description).to_owned(),
            languages: languages.iter().map(|value| (*value).to_owned()).collect(),
        })
        .collect::<Vec<_>>();
    if config.tts.moss_nano.enabled {
        for id in configured.keys() {
            if voices.iter().any(|voice| voice.id.eq_ignore_ascii_case(id)) {
                continue;
            }
            voices.push(TtsVoiceInfo {
                id: id.clone(),
                name: humanize_voice_id(id),
                group: "MOSS Nano 自定义映射".to_owned(),
                description: "通过 TTS_MOSS_NANO_VOICE_MAP 配置的参考音色".to_owned(),
                languages: ["zh", "en", "ja", "ko", "fr", "de", "es", "it", "pt", "ru"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            });
        }
    }
    voices.sort_by(|left, right| {
        left.group
            .cmp(&right.group)
            .then(left.name.cmp(&right.name))
    });
    voices
}

fn index_tts_voices(config: &AppConfig) -> Vec<TtsVoiceInfo> {
    let mut ids = config
        .tts
        .index_tts
        .voice_map
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    if !ids
        .iter()
        .any(|id| id.eq_ignore_ascii_case(&config.tts.index_tts.default_voice_id))
    {
        ids.push(config.tts.index_tts.default_voice_id.clone());
    }
    ids.sort();
    ids.into_iter()
        .map(|id| TtsVoiceInfo {
            name: humanize_voice_id(&id),
            id,
            group: "IndexTTS2 参考声".to_owned(),
            description: "IndexTTS2 零样本参考音色，支持中文与 English".to_owned(),
            languages: vec!["zh".to_owned(), "en".to_owned()],
        })
        .collect()
}

fn qwen_tts_voices() -> Vec<TtsVoiceInfo> {
    const LANGUAGES: &[&str] = &["zh", "en", "ja", "ko", "de", "fr", "ru", "pt", "es", "it"];
    const VOICES: &[(&str, &str, &str, &str)] = &[
        ("vivian", "Vivian", "中文音色", "明亮、略带棱角的年轻女声"),
        ("serena", "Serena", "中文音色", "温暖柔和的年轻女声"),
        ("uncle_fu", "Uncle Fu", "中文音色", "低沉醇厚的成熟男声"),
        ("dylan", "Dylan", "中文方言", "清晰自然的年轻北京男声"),
        ("eric", "Eric", "中文方言", "活泼、略带沙哑明亮感的成都男声"),
        (
            "ryan",
            "Ryan",
            "English voices",
            "节奏感强、富有动感的英文男声",
        ),
        ("aiden", "Aiden", "English voices", "阳光清晰的美式英文男声"),
        ("ono_anna", "Ono Anna", "日本語音声", "轻快灵动的日语女声"),
        ("sohee", "Sohee", "한국어 음성", "温暖且情感丰富的韩语女声"),
    ];
    VOICES
        .iter()
        .map(|(id, name, group, description)| TtsVoiceInfo {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            group: (*group).to_owned(),
            description: (*description).to_owned(),
            languages: LANGUAGES.iter().map(|value| (*value).to_owned()).collect(),
        })
        .collect()
}

fn humanize_voice_id(id: &str) -> String {
    id.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(characters.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
        terminology: &[TranslationTerm],
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<String>;
}

#[derive(Clone, Debug)]
pub struct TranslationTerm {
    pub source: String,
    pub target: String,
    pub aliases: Vec<String>,
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

    fn with_synthesizer(&self, synthesizer: Arc<dyn TtsEngine>) -> Self {
        Self {
            transcriber: self.transcriber.clone(),
            refinement_engines: self.refinement_engines.clone(),
            translator: self.translator.clone(),
            synthesizer,
            backend_name: self.backend_name,
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
