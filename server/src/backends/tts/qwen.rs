use std::{sync::Mutex, time::Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde::Serialize;

use crate::config::QwenTtsConfig;

use super::{TtsChunkSink, TtsEngine, TtsRequest, canonical_language, index_tts::decode_pcm16_wav};

const OUTPUT_CHUNK_MILLIS: usize = 100;
const VOICES: &[&str] = &[
    "vivian", "serena", "uncle_fu", "dylan", "eric", "ryan", "aiden", "ono_anna", "sohee",
];

pub struct QwenTtsEngine {
    config: QwenTtsConfig,
    client: Client,
    base_url: Url,
    unavailable_until: Mutex<Option<Instant>>,
}

#[derive(Serialize)]
struct SpeechRequest<'a> {
    input: &'a str,
    model: &'a str,
    voice: &'a str,
    language: &'a str,
    lang_code: &'a str,
    response_format: &'static str,
    task_type: &'static str,
}

impl QwenTtsEngine {
    pub fn new(config: QwenTtsConfig) -> Result<Self> {
        let base_url = normalized_base_url(&config.base_url)?;
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.timeout)
            .build()
            .context("failed to create Qwen3-TTS HTTP client")?;
        Ok(Self {
            config,
            client,
            base_url,
            unavailable_until: Mutex::new(None),
        })
    }

    fn endpoint(&self) -> Result<Url> {
        self.base_url
            .join("audio/speech")
            .context("failed to resolve Qwen3-TTS speech endpoint")
    }

    fn ensure_retry_allowed(&self) -> Result<()> {
        let unavailable_until = *self
            .unavailable_until
            .lock()
            .map_err(|_| anyhow!("Qwen3-TTS retry state lock was poisoned"))?;
        if unavailable_until.is_some_and(|until| until > Instant::now()) {
            bail!("Qwen3-TTS is temporarily unavailable; retry backoff is active");
        }
        Ok(())
    }

    fn update_availability(&self, available: bool) {
        if let Ok(mut unavailable_until) = self.unavailable_until.lock() {
            *unavailable_until = if available {
                None
            } else {
                Some(Instant::now() + self.config.retry_backoff)
            };
        }
    }

    async fn synthesize_wav(&self, request: &TtsRequest, output: TtsChunkSink) -> Result<()> {
        let text = request.text.trim();
        if text.is_empty() {
            bail!("cannot synthesize empty translated text");
        }
        let requested_voice = request.voice.trim().to_ascii_lowercase();
        let voice = if VOICES.contains(&requested_voice.as_str()) {
            requested_voice.as_str()
        } else {
            self.config.default_voice_id.as_str()
        };
        let language = qwen_language(&request.language);
        let payload = SpeechRequest {
            input: text,
            model: &self.config.model,
            voice,
            language,
            lang_code: language,
            response_format: "wav",
            task_type: "CustomVoice",
        };
        let mut request_builder = self.client.post(self.endpoint()?).json(&payload);
        if let Some(api_key) = &self.config.api_key {
            request_builder = request_builder.bearer_auth(api_key);
        }
        let response = request_builder
            .send()
            .await
            .context("Qwen3-TTS synthesis request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("failed to read Qwen3-TTS synthesis response")?;
        if !status.is_success() {
            bail!(
                "Qwen3-TTS returned HTTP {status}: {}",
                String::from_utf8_lossy(&body).trim()
            );
        }
        let (samples, sample_rate, channels) =
            tokio::task::spawn_blocking(move || decode_pcm16_wav(&body))
                .await
                .context("Qwen3-TTS WAV decoder task failed")??;
        let chunk_samples = (sample_rate as usize * channels as usize * OUTPUT_CHUNK_MILLIS
            / 1_000)
            .max(channels as usize);
        for chunk in samples.chunks(chunk_samples) {
            output.send(chunk.to_vec(), sample_rate, channels).await?;
        }
        if output.emitted_chunks() == 0 {
            bail!("Qwen3-TTS returned empty audio");
        }
        Ok(())
    }
}

#[async_trait]
impl TtsEngine for QwenTtsEngine {
    fn name(&self) -> &'static str {
        "qwen3-tts"
    }

    fn supports(&self, language: &str) -> bool {
        matches!(
            canonical_language(language).as_str(),
            "zh" | "en" | "ja" | "ko" | "de" | "fr" | "ru" | "pt" | "es" | "it"
        )
    }

    async fn synthesize(&self, request: &TtsRequest, output: TtsChunkSink) -> Result<()> {
        self.ensure_retry_allowed()?;
        let result = self.synthesize_wav(request, output).await;
        self.update_availability(result.is_ok());
        result
    }
}

pub(crate) fn normalized_base_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value).context("TTS_QWEN_BASE_URL must be a valid URL")?;
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn qwen_language(language: &str) -> &'static str {
    match canonical_language(language).as_str() {
        "zh" => "Chinese",
        "en" => "English",
        "ja" => "Japanese",
        "ko" => "Korean",
        "de" => "German",
        "fr" => "French",
        "ru" => "Russian",
        "pt" => "Portuguese",
        "es" => "Spanish",
        "it" => "Italian",
        _ => "Auto",
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, sync::Arc, time::Duration};

    use axum::{Router, body::Bytes, routing::post};
    use hound::{SampleFormat, WavSpec, WavWriter};
    use tokio::sync::{Mutex, mpsc};

    use super::*;

    fn wav_bytes() -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = WavWriter::new(
                &mut cursor,
                WavSpec {
                    channels: 1,
                    sample_rate: 24_000,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                },
            )
            .unwrap();
            for sample in [10_i16, -20, 30, -40] {
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
        }
        cursor.into_inner()
    }

    #[tokio::test]
    async fn sends_openai_compatible_qwen_speech_request() {
        let received = Arc::new(Mutex::new(String::new()));
        let captured = received.clone();
        let response_wav = wav_bytes();
        let app = Router::new().route(
            "/v1/audio/speech",
            post(move |body: Bytes| {
                let captured = captured.clone();
                let wav = response_wav.clone();
                async move {
                    *captured.lock().await = String::from_utf8_lossy(&body).into_owned();
                    ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let engine = QwenTtsEngine::new(QwenTtsConfig {
            enabled: true,
            manager_script: std::path::PathBuf::from("scripts/qwen-tts.sh"),
            base_url: format!("http://{address}/v1"),
            model: "Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice".to_owned(),
            api_key: None,
            default_voice_id: "vivian".to_owned(),
            connect_timeout: Duration::from_secs(1),
            timeout: Duration::from_secs(2),
            retry_backoff: Duration::from_secs(1),
        })
        .unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        engine
            .synthesize(
                &TtsRequest {
                    text: "你好".to_owned(),
                    language: "zh-CN".to_owned(),
                    voice: "vivian".to_owned(),
                    reference_audio_path: None,
                },
                TtsChunkSink::new(tx).for_engine(engine.name()),
            )
            .await
            .unwrap();
        assert_eq!(rx.recv().await.unwrap().samples, [10, -20, 30, -40]);
        let body = received.lock().await;
        assert!(body.contains("\"voice\":\"vivian\""));
        assert!(body.contains("\"language\":\"Chinese\""));
        assert!(body.contains("\"lang_code\":\"Chinese\""));
        assert!(body.contains("\"response_format\":\"wav\""));
    }
}
