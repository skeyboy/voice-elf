use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig, OfflineTtsSupertonicModelConfig,
};
use tokio::sync::Mutex;

use crate::config::TtsConfig;

#[derive(Debug)]
pub struct SynthesizedAudio {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
}

#[async_trait]
pub trait Synthesizer: Send + Sync {
    async fn synthesize(&self, text: &str, language: &str, voice: &str)
    -> Result<SynthesizedAudio>;
}

pub struct SherpaOnnxSynthesizer {
    config: TtsConfig,
    kokoro: Arc<Mutex<Option<OfflineTts>>>,
    supertonic: Arc<Mutex<Option<OfflineTts>>>,
}

impl SherpaOnnxSynthesizer {
    pub fn new(config: TtsConfig) -> Result<Self> {
        validate_files(
            &config.kokoro_model_dir,
            &[
                "model.int8.onnx",
                "voices.bin",
                "tokens.txt",
                "espeak-ng-data",
                "dict",
            ],
            "Kokoro",
        )?;
        validate_files(
            &config.supertonic_model_dir,
            &[
                "duration_predictor.int8.onnx",
                "text_encoder.int8.onnx",
                "vector_estimator.int8.onnx",
                "vocoder.int8.onnx",
                "tts.json",
                "unicode_indexer.bin",
                "voice.bin",
            ],
            "Supertonic",
        )?;
        if config.threads == 0 {
            bail!("TTS_THREADS must be greater than zero");
        }
        Ok(Self {
            config,
            kokoro: Arc::new(Mutex::new(None)),
            supertonic: Arc::new(Mutex::new(None)),
        })
    }
}

#[async_trait]
impl Synthesizer for SherpaOnnxSynthesizer {
    async fn synthesize(
        &self,
        text: &str,
        language: &str,
        voice: &str,
    ) -> Result<SynthesizedAudio> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            bail!("cannot synthesize empty translated text");
        }
        let language = canonical_language(language);
        let config = self.config.clone();
        let engine = if language == "zh" {
            self.kokoro.clone()
        } else {
            self.supertonic.clone()
        };
        let voice = voice.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut engine = engine.blocking_lock();
            let tts = match engine.as_ref() {
                Some(tts) => tts,
                None => {
                    let created = if language == "zh" {
                        create_kokoro(&config)?
                    } else {
                        create_supertonic(&config)?
                    };
                    engine.insert(created)
                }
            };
            let generation = if language == "zh" {
                GenerationConfig {
                    sid: kokoro_speaker(&voice),
                    speed: 1.0,
                    ..Default::default()
                }
            } else {
                let mut extra = HashMap::new();
                extra.insert("lang".to_owned(), serde_json::json!(language));
                GenerationConfig {
                    sid: supertonic_speaker(&voice),
                    num_steps: 8,
                    speed: 1.05,
                    extra: Some(extra),
                    ..Default::default()
                }
            };
            let audio = tts
                .generate_with_config(&text, &generation, None::<fn(&[f32], f32) -> bool>)
                .context("sherpa-onnx TTS generation failed")?;
            let sample_rate = u32::try_from(audio.sample_rate())
                .context("TTS returned an invalid sample rate")?;
            let samples = audio
                .samples()
                .iter()
                .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)
                .collect();
            Ok(SynthesizedAudio {
                samples,
                sample_rate,
            })
        })
        .await
        .context("TTS worker task failed")?
    }
}

fn create_kokoro(config: &TtsConfig) -> Result<OfflineTts> {
    let dir = &config.kokoro_model_dir;
    let lexicon = [dir.join("lexicon-us-en.txt"), dir.join("lexicon-zh.txt")]
        .into_iter()
        .map(|path| path_string(&path))
        .collect::<Result<Vec<_>>>()?
        .join(",");
    let config = OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            kokoro: OfflineTtsKokoroModelConfig {
                model: Some(path_string(&dir.join("model.int8.onnx"))?),
                voices: Some(path_string(&dir.join("voices.bin"))?),
                tokens: Some(path_string(&dir.join("tokens.txt"))?),
                data_dir: Some(path_string(&dir.join("espeak-ng-data"))?),
                dict_dir: Some(path_string(&dir.join("dict"))?),
                lexicon: Some(lexicon),
                length_scale: 1.0,
                ..Default::default()
            },
            num_threads: config.threads as i32,
            ..Default::default()
        },
        rule_fsts: Some(
            ["phone-zh.fst", "date-zh.fst", "number-zh.fst"]
                .into_iter()
                .map(|name| path_string(&dir.join(name)))
                .collect::<Result<Vec<_>>>()?
                .join(","),
        ),
        ..Default::default()
    };
    OfflineTts::create(&config).context("failed to load Kokoro TTS model")
}

fn create_supertonic(config: &TtsConfig) -> Result<OfflineTts> {
    let dir = &config.supertonic_model_dir;
    let model = |name: &str| path_string(&dir.join(name));
    let config = OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            supertonic: OfflineTtsSupertonicModelConfig {
                duration_predictor: Some(model("duration_predictor.int8.onnx")?),
                text_encoder: Some(model("text_encoder.int8.onnx")?),
                vector_estimator: Some(model("vector_estimator.int8.onnx")?),
                vocoder: Some(model("vocoder.int8.onnx")?),
                tts_json: Some(model("tts.json")?),
                unicode_indexer: Some(model("unicode_indexer.bin")?),
                voice_style: Some(model("voice.bin")?),
            },
            num_threads: config.threads as i32,
            ..Default::default()
        },
        ..Default::default()
    };
    OfflineTts::create(&config).context("failed to load Supertonic TTS model")
}

fn validate_files(dir: &Path, names: &[&str], model: &str) -> Result<()> {
    if !dir.is_dir() {
        bail!("{model} model directory does not exist: {}", dir.display());
    }
    for name in names {
        if !dir.join(name).exists() {
            bail!(
                "{model} model file is missing: {}",
                dir.join(name).display()
            );
        }
    }
    Ok(())
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .with_context(|| format!("model path is not valid UTF-8: {}", path.display()))
}

fn canonical_language(language: &str) -> String {
    let language = language
        .split(['-', '_'])
        .next()
        .unwrap_or(language)
        .to_ascii_lowercase();
    match language.as_str() {
        "chinese" => "zh".to_owned(),
        "english" => "en".to_owned(),
        other => other.to_owned(),
    }
}

fn kokoro_speaker(voice: &str) -> i32 {
    if voice.to_ascii_lowercase().starts_with('m') || voice.eq_ignore_ascii_case("ryan") {
        58
    } else {
        3
    }
}

fn supertonic_speaker(voice: &str) -> i32 {
    if voice.to_ascii_lowercase().starts_with('m') || voice.eq_ignore_ascii_case("ryan") {
        6
    } else {
        0
    }
}

#[cfg(test)]
pub struct DemoSynthesizer;

#[cfg(test)]
impl DemoSynthesizer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
#[cfg(test)]
impl Synthesizer for DemoSynthesizer {
    async fn synthesize(
        &self,
        text: &str,
        _language: &str,
        _voice: &str,
    ) -> Result<SynthesizedAudio> {
        const SAMPLE_RATE: u32 = 24_000;
        let sample_count = (text.chars().count().max(1) * 240).min(SAMPLE_RATE as usize * 2);
        let samples = (0..sample_count)
            .map(|index| {
                let phase = index as f32 * 2.0 * std::f32::consts::PI * 220.0 / SAMPLE_RATE as f32;
                (phase.sin() * 1_200.0) as i16
            })
            .collect();
        Ok(SynthesizedAudio {
            samples,
            sample_rate: SAMPLE_RATE,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_regional_language_codes() {
        assert_eq!(canonical_language("zh-CN"), "zh");
        assert_eq!(canonical_language("pt_BR"), "pt");
    }

    #[tokio::test]
    #[ignore = "loads the installed native TTS models"]
    async fn installed_models_generate_chinese_and_english_audio() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let synthesizer = SherpaOnnxSynthesizer::new(TtsConfig {
            kokoro_model_dir: root.join(".local/models/tts/kokoro-int8-multi-lang-v1_1"),
            supertonic_model_dir: root
                .join(".local/models/tts/sherpa-onnx-supertonic-3-tts-int8-2026-05-11"),
            threads: 2,
        })
        .unwrap();
        for (text, language) in [("你好这是中文测试", "zh"), ("Hello", "en")] {
            let audio = synthesizer
                .synthesize(text, language, "ryan")
                .await
                .unwrap();
            assert!(!audio.samples.is_empty());
            assert!(audio.sample_rate >= 16_000);
        }
    }
}
