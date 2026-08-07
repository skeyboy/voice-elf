use std::{io::Cursor, sync::Mutex, time::Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::{
    Client, Url,
    multipart::{Form, Part},
};

use crate::config::IndexTtsConfig;

use super::{TtsChunkSink, TtsEngine, TtsRequest};

const OUTPUT_CHUNK_MILLIS: usize = 100;

pub struct IndexTtsEngine {
    config: IndexTtsConfig,
    client: Client,
    base_url: Url,
    unavailable_until: Mutex<Option<Instant>>,
}

impl IndexTtsEngine {
    pub fn new(config: IndexTtsConfig) -> Result<Self> {
        let base_url =
            Url::parse(&config.base_url).context("TTS_INDEX_BASE_URL must be a valid URL")?;
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.timeout)
            .build()
            .context("failed to create IndexTTS HTTP client")?;
        Ok(Self {
            config,
            client,
            base_url,
            unavailable_until: Mutex::new(None),
        })
    }

    fn endpoint(&self) -> Result<Url> {
        self.base_url
            .join("v1/tts")
            .context("failed to resolve IndexTTS endpoint")
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.config.api_key {
            Some(api_key) => request.bearer_auth(api_key),
            None => request,
        }
    }

    fn ensure_retry_allowed(&self) -> Result<()> {
        let unavailable_until = *self
            .unavailable_until
            .lock()
            .map_err(|_| anyhow!("IndexTTS retry state lock was poisoned"))?;
        if unavailable_until.is_some_and(|until| until > Instant::now()) {
            bail!("IndexTTS is temporarily unavailable; retry backoff is active");
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
        let mut form = Form::new()
            .text("text", text.to_owned())
            .text("language", request.language.clone())
            .text("voice", request.voice.clone());
        if let Some(path) = &request.reference_audio_path {
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("failed to read voice reference: {}", path.display()))?;
            form = form.part(
                "reference_audio",
                Part::bytes(bytes)
                    .file_name("reference.wav")
                    .mime_str("audio/wav")
                    .context("failed to build IndexTTS voice reference upload")?,
            );
        }
        let response = self
            .authorize(self.client.post(self.endpoint()?))
            .multipart(form)
            .send()
            .await
            .context("IndexTTS synthesis request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("failed to read IndexTTS synthesis response")?;
        if !status.is_success() {
            bail!(
                "IndexTTS returned HTTP {status}: {}",
                String::from_utf8_lossy(&body).trim()
            );
        }
        let (samples, sample_rate, channels) =
            tokio::task::spawn_blocking(move || decode_pcm16_wav(&body))
                .await
                .context("IndexTTS WAV decoder task failed")??;
        let chunk_samples = (sample_rate as usize * channels as usize * OUTPUT_CHUNK_MILLIS
            / 1_000)
            .max(channels as usize);
        for chunk in samples.chunks(chunk_samples) {
            output.send(chunk.to_vec(), sample_rate, channels).await?;
        }
        if output.emitted_chunks() == 0 {
            bail!("IndexTTS returned empty audio");
        }
        Ok(())
    }
}

#[async_trait]
impl TtsEngine for IndexTtsEngine {
    fn name(&self) -> &'static str {
        "index-tts2"
    }

    fn supports(&self, language: &str) -> bool {
        matches!(super::canonical_language(language).as_str(), "zh" | "en")
    }

    fn supports_voice_clone(&self) -> bool {
        true
    }

    async fn synthesize(&self, request: &TtsRequest, output: TtsChunkSink) -> Result<()> {
        self.ensure_retry_allowed()?;
        let result = self.synthesize_wav(request, output).await;
        self.update_availability(result.is_ok());
        result
    }
}

fn decode_pcm16_wav(bytes: &[u8]) -> Result<(Vec<i16>, u32, u16)> {
    let mut reader = hound::WavReader::new(Cursor::new(bytes)).context("invalid IndexTTS WAV")?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        bail!("IndexTTS WAV must use 16-bit PCM samples");
    }
    if spec.sample_rate == 0 || spec.channels == 0 {
        bail!("IndexTTS WAV contains invalid audio metadata");
    }
    let samples = reader
        .samples::<i16>()
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to decode IndexTTS WAV samples")?;
    Ok((samples, spec.sample_rate, spec.channels))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use axum::{Router, body::Bytes, routing::post};
    use tokio::sync::mpsc;

    use super::*;

    fn wav_bytes() -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut cursor,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 24_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for sample in [1_i16, -2, 3, -4] {
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn decodes_pcm16_wav() {
        let (samples, sample_rate, channels) = decode_pcm16_wav(&wav_bytes()).unwrap();
        assert_eq!(samples, [1, -2, 3, -4]);
        assert_eq!(sample_rate, 24_000);
        assert_eq!(channels, 1);
    }

    #[tokio::test]
    async fn sends_reference_audio_to_project_sidecar() {
        let received_reference = Arc::new(AtomicBool::new(false));
        let handler_flag = received_reference.clone();
        let response_wav = wav_bytes();
        let app = Router::new().route(
            "/v1/tts",
            post(move |body: Bytes| {
                let flag = handler_flag.clone();
                let wav = response_wav.clone();
                async move {
                    let body = String::from_utf8_lossy(&body);
                    flag.store(
                        body.contains("name=\"reference_audio\"") && body.contains("reference.wav"),
                        Ordering::Relaxed,
                    );
                    ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let engine = IndexTtsEngine::new(IndexTtsConfig {
            enabled: true,
            base_url: format!("http://{address}/"),
            api_key: None,
            manager_script: std::path::PathBuf::from("index-tts.sh"),
            model_dir: std::path::PathBuf::from("checkpoints"),
            runtime_dir: std::path::PathBuf::from("run"),
            default_voice_id: "F1".to_owned(),
            voice_map: std::collections::HashMap::new(),
            connect_timeout: Duration::from_secs(1),
            timeout: Duration::from_secs(2),
            retry_backoff: Duration::from_secs(1),
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let reference = directory.path().join("voice.wav");
        tokio::fs::write(&reference, wav_bytes()).await.unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        engine
            .synthesize(
                &TtsRequest {
                    text: "你好".to_owned(),
                    language: "zh-CN".to_owned(),
                    voice: "F1".to_owned(),
                    reference_audio_path: Some(reference),
                },
                TtsChunkSink::new(tx).for_engine(engine.name()),
            )
            .await
            .unwrap();
        assert_eq!(rx.recv().await.unwrap().samples, [1, -2, 3, -4]);
        assert!(received_reference.load(Ordering::Relaxed));
    }
}
