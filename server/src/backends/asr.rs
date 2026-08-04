use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
    time::timeout,
};

use crate::{audio::pcm16_bytes, config::AsrConfig, protocol::INPUT_SAMPLE_RATE};

use super::{LiveTranscription, Transcriber, Transcription, language_name, short_demo_delay};

#[derive(Debug, thiserror::Error)]
#[error("未识别到清晰语音")]
pub struct NoSpeechDetected;

pub struct DemoTranscriber;

impl DemoTranscriber {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Transcriber for DemoTranscriber {
    async fn start_live(
        &self,
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Option<LiveTranscription>> {
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let language = if source_language == "auto" {
            "en".to_owned()
        } else {
            source_language.to_owned()
        };
        let task_language = language.clone();
        let task = tokio::spawn(async move {
            let mut sample_count = 0_usize;
            let mut emitted_preview = false;
            while let Some(chunk) = audio_rx.recv().await {
                sample_count += chunk.len();
                if !emitted_preview && sample_count >= INPUT_SAMPLE_RATE as usize / 2 {
                    emitted_preview = true;
                    let preview = match task_language.as_str() {
                        "zh" => "正在接收语音…",
                        "ja" => "音声を受信中…",
                        _ => "Receiving speech…",
                    };
                    let _ = updates.send(preview.to_owned());
                }
            }
            let seconds = sample_count as f32 / INPUT_SAMPLE_RATE as f32;
            let text = match task_language.as_str() {
                "zh" => format!("收到了一段 {seconds:.1} 秒的语音。"),
                "ja" => format!("{seconds:.1} 秒の音声を受信しました。"),
                _ => format!("Received a {seconds:.1} second voice sample."),
            };
            Ok(Transcription {
                text,
                language: task_language,
            })
        });
        Ok(Some(LiveTranscription::new(audio_tx, task)))
    }

    async fn transcribe_streaming(
        &self,
        pcm: &[i16],
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Transcription> {
        short_demo_delay(Duration::from_millis(160)).await;
        let seconds = pcm.len() as f32 / INPUT_SAMPLE_RATE as f32;
        let detected = if source_language == "auto" {
            "en"
        } else {
            source_language
        };
        let text = match detected {
            "zh" => format!("收到了一段 {seconds:.1} 秒的语音。"),
            "ja" => format!("{seconds:.1} 秒の音声を受信しました。"),
            _ => format!("Received a {seconds:.1} second voice sample."),
        };
        for chunk in text.chars().collect::<Vec<_>>().chunks(4) {
            let _ = updates.send(chunk.iter().collect());
            short_demo_delay(Duration::from_millis(30)).await;
        }
        Ok(Transcription {
            text,
            language: detected.to_owned(),
        })
    }
}

pub struct QwenAsrTranscriber {
    binary: PathBuf,
    model_dir: PathBuf,
    timeout: Duration,
    stream_unfixed_chunks: usize,
    stream_max_new_tokens: usize,
    encoder_window_seconds: u32,
}

impl QwenAsrTranscriber {
    pub fn new(config: AsrConfig, timeout: Duration) -> Result<Self> {
        let model_dir = config
            .model_dir
            .context("QWEN_ASR_MODEL_DIR is required for the local backend")?;
        Ok(Self {
            binary: config.binary,
            model_dir,
            timeout,
            stream_unfixed_chunks: config.stream_unfixed_chunks.min(8),
            stream_max_new_tokens: config.stream_max_new_tokens.clamp(4, 64),
            encoder_window_seconds: config.encoder_window_seconds.clamp(1, 8),
        })
    }

    async fn transcribe_batch(&self, pcm: &[i16], source_language: &str) -> Result<String> {
        let mut command = Command::new(&self.binary);
        command
            .arg("-d")
            .arg(&self.model_dir)
            .arg("--stdin")
            .arg("--silent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if source_language != "auto" {
            command
                .arg("--language")
                .arg(language_name(source_language));
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start Qwen ASR binary at {}",
                self.binary.display()
            )
        })?;
        let mut stdin = child
            .stdin
            .take()
            .context("Qwen ASR stdin was unavailable")?;
        stdin
            .write_all(&pcm16_bytes(pcm))
            .await
            .context("failed to send PCM to Qwen ASR fallback")?;
        drop(stdin);
        let output = timeout(self.timeout, child.wait_with_output())
            .await
            .context("Qwen ASR fallback timed out")??;
        if !output.status.success() {
            bail!(
                "Qwen ASR fallback failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let text = String::from_utf8(output.stdout)
            .context("Qwen ASR fallback output was not UTF-8")?
            .trim()
            .to_owned();
        Ok(text)
    }

    async fn transcribe_batch_with_language_fallback(
        &self,
        pcm: &[i16],
        source_language: &str,
    ) -> Result<Transcription> {
        let text = self.transcribe_batch(pcm, source_language).await?;
        if !text.is_empty() {
            return Ok(Transcription {
                text,
                language: source_language.to_owned(),
            });
        }
        if source_language != "auto" {
            tracing::warn!(
                source_language,
                "Qwen ASR forced language returned no text; retrying auto detection"
            );
            let text = self.transcribe_batch(pcm, "auto").await?;
            if !text.is_empty() {
                return Ok(Transcription {
                    text,
                    language: "auto".to_owned(),
                });
            }
        }
        Err(NoSpeechDetected.into())
    }
}

#[async_trait]
impl Transcriber for QwenAsrTranscriber {
    async fn start_live(
        &self,
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Option<LiveTranscription>> {
        let mut command = Command::new(&self.binary);
        command
            .arg("-d")
            .arg(&self.model_dir)
            .arg("--stdin")
            .arg("--stream")
            .arg("--stream-unfixed-chunks")
            .arg(self.stream_unfixed_chunks.to_string())
            .arg("--stream-max-new-tokens")
            .arg(self.stream_max_new_tokens.to_string())
            .arg("--enc-window-sec")
            .arg(self.encoder_window_seconds.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if source_language != "auto" {
            command
                .arg("--language")
                .arg(language_name(source_language));
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start live Qwen ASR binary at {}",
                self.binary.display()
            )
        })?;
        let mut stdin = child
            .stdin
            .take()
            .context("live Qwen ASR stdin was unavailable")?;
        let mut stdout = child
            .stdout
            .take()
            .context("live Qwen ASR stdout was unavailable")?;
        let mut stderr = child
            .stderr
            .take()
            .context("live Qwen ASR stderr was unavailable")?;
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let inference_timeout = self.timeout;
        let language = source_language.to_owned();
        let task = tokio::spawn(async move {
            let operation = async move {
                let writer = async move {
                    while let Some(pcm) = audio_rx.recv().await {
                        stdin
                            .write_all(&pcm16_bytes(&pcm))
                            .await
                            .context("failed to stream live PCM to Qwen ASR")?;
                    }
                    drop(stdin);
                    Result::<()>::Ok(())
                };
                let reader = async move {
                    let mut output = Vec::new();
                    let mut pending = Vec::new();
                    let mut buffer = [0_u8; 1024];
                    loop {
                        let read = stdout.read(&mut buffer).await?;
                        if read == 0 {
                            break;
                        }
                        output.extend_from_slice(&buffer[..read]);
                        pending.extend_from_slice(&buffer[..read]);
                        emit_valid_utf8(&mut pending, &updates)?;
                    }
                    if !pending.is_empty() {
                        bail!("live Qwen ASR output ended with invalid UTF-8");
                    }
                    Result::<Vec<u8>>::Ok(output)
                };
                let error_reader = async move {
                    let mut output = Vec::new();
                    stderr.read_to_end(&mut output).await?;
                    Result::<Vec<u8>>::Ok(output)
                };
                tokio::try_join!(writer, reader, error_reader)
            };
            let (_, stdout, stderr) = timeout(inference_timeout, operation)
                .await
                .context("live Qwen ASR timed out")??;
            let status = child.wait().await?;
            if !status.success() {
                bail!(
                    "live Qwen ASR failed: {}",
                    String::from_utf8_lossy(&stderr).trim()
                );
            }
            let text = String::from_utf8(stdout)
                .context("live Qwen ASR output was not UTF-8")?
                .trim()
                .to_owned();
            if text.is_empty() {
                return Err(NoSpeechDetected.into());
            }
            Ok(Transcription { text, language })
        });
        Ok(Some(LiveTranscription::new(audio_tx, task)))
    }

    async fn transcribe_streaming(
        &self,
        pcm: &[i16],
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Transcription> {
        let prepared = prepare_asr_audio(pcm)?;
        let transcription = self
            .transcribe_batch_with_language_fallback(&prepared, source_language)
            .await?;
        let _ = updates.send(transcription.text.clone());
        Ok(transcription)
    }
}

fn prepare_asr_audio(pcm: &[i16]) -> Result<Vec<i16>> {
    if pcm.is_empty() {
        return Err(NoSpeechDetected.into());
    }
    let mean = pcm.iter().map(|sample| *sample as i64).sum::<i64>() as f64 / pcm.len() as f64;
    let centered = pcm
        .iter()
        .map(|sample| (*sample as f64 - mean).round() as i32)
        .collect::<Vec<_>>();
    let peak = centered
        .iter()
        .map(|sample| sample.abs())
        .max()
        .unwrap_or(0);
    if peak < 96 {
        return Err(NoSpeechDetected.into());
    }
    let gain = if peak < 20_000 {
        (26_000_f64 / peak as f64).clamp(1.0, 8.0)
    } else {
        1.0
    };
    Ok(centered
        .into_iter()
        .map(|sample| (sample as f64 * gain).round().clamp(-32_768.0, 32_767.0) as i16)
        .collect())
}

fn emit_valid_utf8(pending: &mut Vec<u8>, updates: &mpsc::UnboundedSender<String>) -> Result<()> {
    match std::str::from_utf8(pending) {
        Ok(text) => {
            if !text.is_empty() {
                let _ = updates.send(text.to_owned());
            }
            pending.clear();
            Ok(())
        }
        Err(error) if error.error_len().is_none() => {
            let valid = error.valid_up_to();
            if valid > 0 {
                let text = std::str::from_utf8(&pending[..valid])?.to_owned();
                let _ = updates.send(text);
                pending.drain(..valid);
            }
            Ok(())
        }
        Err(error) => bail!("Qwen ASR output was not UTF-8: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_quiet_audio_without_changing_length() {
        let input = [0_i16, 120, -100, 80, -90];
        let output = prepare_asr_audio(&input).unwrap();
        assert_eq!(output.len(), input.len());
        assert!(output.iter().map(|sample| sample.abs()).max().unwrap() > 500);
    }

    #[test]
    fn rejects_silent_audio_before_starting_qwen() {
        let error = prepare_asr_audio(&[0_i16; 16_000]).unwrap_err();
        assert!(error.downcast_ref::<NoSpeechDetected>().is_some());
    }
}
