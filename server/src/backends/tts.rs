use std::{
    f32::consts::TAU,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use hound::{SampleFormat, WavReader};
use tempfile::tempdir;
use tokio::{process::Command, time::timeout};

use crate::config::TtsConfig;

use super::{SynthesizedAudio, Synthesizer, language_name, short_demo_delay};

pub struct DemoSynthesizer;

impl DemoSynthesizer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Synthesizer for DemoSynthesizer {
    async fn synthesize(
        &self,
        text: &str,
        language: &str,
        _voice: &str,
    ) -> Result<SynthesizedAudio> {
        short_demo_delay(Duration::from_millis(180)).await;
        if !cfg!(test) && cfg!(target_os = "macos") {
            match synthesize_macos_voice(text, language).await {
                Ok(audio) => return Ok(audio),
                Err(error) => {
                    tracing::warn!(%error, "system TTS failed; using the demo chime");
                }
            }
        }
        Ok(demo_chime())
    }
}

fn demo_chime() -> SynthesizedAudio {
    let sample_rate = 24_000;
    let duration_seconds = 0.72;
    let sample_count = (sample_rate as f32 * duration_seconds) as usize;
    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        let time = index as f32 / sample_rate as f32;
        let frequency = if time < 0.34 { 523.25 } else { 659.25 };
        let attack = (time / 0.025).min(1.0);
        let release = ((duration_seconds - time) / 0.12).clamp(0.0, 1.0);
        let gap = if (0.31..0.38).contains(&time) {
            0.0
        } else {
            1.0
        };
        let value = (time * frequency * TAU).sin() * attack * release * gap * 0.16;
        samples.push((value * i16::MAX as f32) as i16);
    }
    SynthesizedAudio {
        samples,
        sample_rate,
    }
}

async fn synthesize_macos_voice(text: &str, language: &str) -> Result<SynthesizedAudio> {
    let directory = tempdir().context("failed to create system TTS output directory")?;
    let source_path = directory.path().join("speech.aiff");
    let output_path = directory.path().join("speech.wav");
    let voice = match language {
        "zh" => "Tingting",
        "ja" => "Kyoko",
        "ko" => "Yuna",
        "fr" => "Thomas",
        "de" => "Anna",
        "es" => "Monica",
        "it" => "Alice",
        "pt" => "Joana",
        "ru" => "Milena",
        _ => "Samantha",
    };

    let say = Command::new("say")
        .arg("-v")
        .arg(voice)
        .arg("-o")
        .arg(&source_path)
        .arg(text)
        .kill_on_drop(true)
        .output();
    let say_output = timeout(Duration::from_secs(30), say)
        .await
        .context("system TTS timed out")?
        .context("failed to start macOS system TTS")?;
    if !say_output.status.success() {
        bail!(
            "macOS system TTS failed: {}",
            String::from_utf8_lossy(&say_output.stderr).trim()
        );
    }

    let conversion = Command::new("ffmpeg")
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(&source_path)
        .arg("-ar")
        .arg("24000")
        .arg("-ac")
        .arg("1")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(&output_path)
        .kill_on_drop(true)
        .output();
    let conversion_output = timeout(Duration::from_secs(30), conversion)
        .await
        .context("system TTS conversion timed out")?
        .context("failed to start FFmpeg for system TTS")?;
    if !conversion_output.status.success() {
        bail!(
            "system TTS conversion failed: {}",
            String::from_utf8_lossy(&conversion_output.stderr).trim()
        );
    }
    read_wav(&output_path)
}

pub struct QwenTtsSynthesizer {
    binary: PathBuf,
    model_dir: PathBuf,
    default_speaker: String,
    device: String,
    timeout: Duration,
}

impl QwenTtsSynthesizer {
    pub fn new(config: TtsConfig, timeout: Duration) -> Result<Self> {
        let model_dir = config
            .model_dir
            .context("QWEN_TTS_MODEL_DIR is required for the local backend")?;
        Ok(Self {
            binary: config.binary,
            model_dir,
            default_speaker: config.speaker,
            device: config.device,
            timeout,
        })
    }
}

#[async_trait]
impl Synthesizer for QwenTtsSynthesizer {
    async fn synthesize(
        &self,
        text: &str,
        language: &str,
        voice: &str,
    ) -> Result<SynthesizedAudio> {
        let directory = tempdir().context("failed to create TTS output directory")?;
        let output_path = directory.path().join("speech.wav");
        let speaker = if voice.is_empty() {
            &self.default_speaker
        } else {
            voice
        };
        let child = Command::new(&self.binary)
            .arg("--model-dir")
            .arg(&self.model_dir)
            .arg("--text")
            .arg(text)
            .arg("--speaker")
            .arg(speaker)
            .arg("--language")
            .arg(language_name(language).to_ascii_lowercase())
            .arg("--device")
            .arg(&self.device)
            .arg("--output-dir")
            .arg(directory.path())
            .arg("--output")
            .arg(&output_path)
            .kill_on_drop(true)
            .output();
        let output = timeout(self.timeout, child)
            .await
            .context("Qwen TTS timed out")?
            .with_context(|| {
                format!(
                    "failed to start Qwen TTS binary at {}",
                    self.binary.display()
                )
            })?;
        if !output.status.success() {
            bail!(
                "Qwen TTS failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        read_wav(&output_path)
    }
}

fn read_wav(path: &Path) -> Result<SynthesizedAudio> {
    let mut reader = WavReader::open(path).context("failed to open Qwen TTS WAV output")?;
    let spec = reader.spec();
    if spec.channels != 1 {
        bail!(
            "Qwen TTS returned {} channels; mono is required",
            spec.channels
        );
    }

    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .context("invalid PCM16 samples in TTS output")?,
        (SampleFormat::Int, bits) => {
            let shift = bits.saturating_sub(16) as u32;
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| (value >> shift) as i16))
                .collect::<Result<Vec<_>, _>>()
                .context("invalid integer samples in TTS output")?
        }
        (SampleFormat::Float, _) => reader
            .samples::<f32>()
            .map(|sample| sample.map(|value| (value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16))
            .collect::<Result<Vec<_>, _>>()
            .context("invalid float samples in TTS output")?,
    };
    if samples.is_empty() {
        bail!("Qwen TTS produced an empty WAV file");
    }
    Ok(SynthesizedAudio {
        samples,
        sample_rate: spec.sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn demo_audio_is_pcm16_at_24khz() {
        let audio = DemoSynthesizer::new()
            .synthesize("hello", "en", "ryan")
            .await
            .unwrap();
        assert_eq!(audio.sample_rate, 24_000);
        assert!(audio.samples.len() > 10_000);
        assert!(audio.samples.iter().any(|sample| *sample != 0));
    }
}
