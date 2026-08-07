mod fallback;
mod index_tts;
mod moss_nano;
mod sherpa;

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::TtsConfig;

pub use fallback::FallbackTtsEngine;
pub use index_tts::IndexTtsEngine;
pub use moss_nano::MossNanoOnnxEngine;
pub use sherpa::{KokoroEngine, SupertonicEngine};

#[derive(Clone, Debug)]
pub struct TtsRequest {
    pub text: String,
    pub language: String,
    pub voice: String,
    pub reference_audio_path: Option<PathBuf>,
}

#[derive(Debug)]
pub struct TtsAudioChunk {
    pub engine: &'static str,
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Clone)]
pub struct TtsChunkSink {
    output: mpsc::Sender<TtsAudioChunk>,
    engine: &'static str,
    emitted_chunks: Arc<AtomicUsize>,
}

impl TtsChunkSink {
    pub fn new(output: mpsc::Sender<TtsAudioChunk>) -> Self {
        Self {
            output,
            engine: "unassigned",
            emitted_chunks: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn for_engine(&self, engine: &'static str) -> Self {
        Self {
            output: self.output.clone(),
            engine,
            emitted_chunks: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn emitted_chunks(&self) -> usize {
        self.emitted_chunks.load(Ordering::Relaxed)
    }

    pub async fn send(&self, samples: Vec<i16>, sample_rate: u32, channels: u16) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        if sample_rate == 0 || channels == 0 || samples.len() % usize::from(channels) != 0 {
            bail!("{} returned invalid audio metadata", self.engine);
        }
        self.output
            .send(TtsAudioChunk {
                engine: self.engine,
                samples,
                sample_rate,
                channels,
            })
            .await
            .map_err(|_| anyhow::anyhow!("TTS audio receiver closed"))?;
        self.emitted_chunks.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
pub trait TtsEngine: Send + Sync {
    fn name(&self) -> &'static str;

    fn supports(&self, language: &str) -> bool;

    fn supports_voice_clone(&self) -> bool {
        false
    }

    async fn synthesize(&self, request: &TtsRequest, output: TtsChunkSink) -> Result<()>;
}

pub fn build_tts_engine(config: &TtsConfig) -> Result<Arc<dyn TtsEngine>> {
    let mut engines: Vec<Arc<dyn TtsEngine>> = Vec::new();
    if config.moss_nano.enabled {
        engines.push(Arc::new(MossNanoOnnxEngine::new(config.moss_nano.clone())?));
    }
    engines.push(Arc::new(KokoroEngine::new(config.clone())?));
    engines.push(Arc::new(SupertonicEngine::new(config.clone())?));
    Ok(Arc::new(FallbackTtsEngine::new(engines)?))
}

pub(crate) fn canonical_language(language: &str) -> String {
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
impl TtsEngine for DemoSynthesizer {
    fn name(&self) -> &'static str {
        "demo"
    }

    fn supports(&self, _language: &str) -> bool {
        true
    }

    async fn synthesize(&self, request: &TtsRequest, output: TtsChunkSink) -> Result<()> {
        const SAMPLE_RATE: u32 = 24_000;
        let sample_count =
            (request.text.chars().count().max(1) * 240).min(SAMPLE_RATE as usize * 2);
        let samples = (0..sample_count)
            .map(|index| {
                let phase = index as f32 * 2.0 * std::f32::consts::PI * 220.0 / SAMPLE_RATE as f32;
                (phase.sin() * 1_200.0) as i16
            })
            .collect();
        output
            .for_engine(self.name())
            .send(samples, SAMPLE_RATE, 1)
            .await
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
}
