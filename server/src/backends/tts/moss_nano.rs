use std::{sync::Mutex, time::Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{
    Client, Url,
    multipart::{Form, Part},
};
use serde::Deserialize;

use crate::config::MossNanoTtsConfig;

use super::{TtsChunkSink, TtsEngine, TtsRequest, canonical_language};

const SUPPORTED_LANGUAGES: &[&str] = &[
    "zh", "en", "de", "es", "fr", "ja", "it", "hu", "ko", "ru", "fa", "ar", "pl", "pt", "cs", "da",
    "sv", "el", "tr",
];

pub struct MossNanoOnnxEngine {
    config: MossNanoTtsConfig,
    client: Client,
    base_url: Url,
    unavailable_until: Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
struct StartResponse {
    stream_id: String,
    audio_url: String,
    sample_rate: u32,
    channels: u16,
}

impl MossNanoOnnxEngine {
    pub fn new(config: MossNanoTtsConfig) -> Result<Self> {
        if config.cpu_threads == 0 {
            bail!("TTS_MOSS_NANO_CPU_THREADS must be greater than zero");
        }
        let base_url =
            Url::parse(&config.base_url).context("TTS_MOSS_NANO_BASE_URL must be a valid URL")?;
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.timeout)
            .build()
            .context("failed to create MOSS Nano HTTP client")?;
        Ok(Self {
            config,
            client,
            base_url,
            unavailable_until: Mutex::new(None),
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        self.base_url
            .join(path)
            .with_context(|| format!("failed to resolve MOSS Nano endpoint '{path}'"))
    }

    fn demo_id(&self, voice: &str) -> &str {
        self.config
            .voice_map
            .get(&voice.to_ascii_uppercase())
            .map(String::as_str)
            .unwrap_or(&self.config.default_demo_id)
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
            .map_err(|_| anyhow!("MOSS Nano retry state lock was poisoned"))?;
        if unavailable_until.is_some_and(|until| until > Instant::now()) {
            bail!("MOSS Nano is temporarily unavailable; retry backoff is active");
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

    async fn synthesize_stream(&self, request: &TtsRequest, output: TtsChunkSink) -> Result<()> {
        let text = request.text.trim();
        if text.is_empty() {
            bail!("cannot synthesize empty translated text");
        }
        let mut form = Form::new()
            .text("text", text.to_owned())
            .text("cpu_threads", self.config.cpu_threads.to_string())
            .text("enable_text_normalization", "1")
            .text("enable_normalize_tts_text", "1");
        if let Some(path) = &request.reference_audio_path {
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("failed to read voice reference: {}", path.display()))?;
            let part = Part::bytes(bytes)
                .file_name("reference.wav")
                .mime_str("audio/wav")
                .context("failed to build voice reference upload")?;
            form = form.part("prompt_audio", part);
        } else {
            form = form.text("demo_id", self.demo_id(&request.voice).to_owned());
        }
        let response = self
            .authorize(
                self.client
                    .post(self.endpoint("api/generate-stream/start")?),
            )
            .multipart(form)
            .send()
            .await
            .context("MOSS Nano stream start request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("failed to read MOSS Nano start response")?;
        if !status.is_success() {
            bail!(
                "MOSS Nano stream start returned HTTP {status}: {}",
                String::from_utf8_lossy(&body).trim()
            );
        }
        let started: StartResponse = serde_json::from_slice(&body).with_context(|| {
            format!(
                "MOSS Nano start response was not valid JSON: {}",
                String::from_utf8_lossy(&body).trim()
            )
        })?;
        if started.sample_rate == 0 || started.channels == 0 {
            bail!("MOSS Nano returned invalid audio metadata");
        }
        let audio_url = self
            .base_url
            .join(&started.audio_url)
            .context("MOSS Nano returned an invalid audio URL")?;
        let audio_response = self
            .authorize(self.client.get(audio_url))
            .send()
            .await
            .context("MOSS Nano audio stream request failed")?;
        if !audio_response.status().is_success() {
            let status = audio_response.status();
            let message = audio_response.text().await.unwrap_or_default();
            bail!("MOSS Nano audio stream returned HTTP {status}: {message}");
        }

        let bytes_per_frame = usize::from(started.channels) * size_of::<i16>();
        let mut remainder = Vec::new();
        let mut stream = audio_response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            remainder.extend_from_slice(&chunk.context("MOSS Nano audio stream failed")?);
            let aligned_len = remainder.len() / bytes_per_frame * bytes_per_frame;
            if aligned_len == 0 {
                continue;
            }
            let samples = pcm16le_samples(&remainder[..aligned_len]);
            remainder.drain(..aligned_len);
            output
                .send(samples, started.sample_rate, started.channels)
                .await?;
        }
        if !remainder.is_empty() {
            bail!("MOSS Nano audio stream ended with an incomplete PCM frame");
        }

        let close_path = format!("api/generate-stream/{}/close", started.stream_id);
        if let Ok(url) = self.endpoint(&close_path) {
            let _ = self.authorize(self.client.post(url)).send().await;
        }
        if output.emitted_chunks() == 0 {
            return Err(anyhow!("MOSS Nano returned an empty audio stream"));
        }
        Ok(())
    }
}

#[async_trait]
impl TtsEngine for MossNanoOnnxEngine {
    fn name(&self) -> &'static str {
        "moss-nano-onnx"
    }

    fn supports(&self, language: &str) -> bool {
        SUPPORTED_LANGUAGES.contains(&canonical_language(language).as_str())
    }

    fn supports_voice_clone(&self) -> bool {
        true
    }

    async fn synthesize(&self, request: &TtsRequest, output: TtsChunkSink) -> Result<()> {
        self.ensure_retry_allowed()?;
        let result = self.synthesize_stream(request, output).await;
        self.update_availability(result.is_ok());
        result
    }
}

fn pcm16le_samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use axum::{
        Json, Router,
        body::Bytes,
        routing::{get, post},
    };
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;

    #[test]
    fn decodes_little_endian_pcm() {
        assert_eq!(pcm16le_samples(&[0x34, 0x12, 0xfe, 0xff]), [0x1234, -2]);
    }

    #[test]
    fn exposes_current_product_languages() {
        for language in ["zh", "en", "ja", "ko", "fr", "de", "es", "it", "pt", "ru"] {
            assert!(SUPPORTED_LANGUAGES.contains(&language));
        }
    }

    #[tokio::test]
    async fn consumes_the_official_streaming_http_shape() {
        let received_reference = Arc::new(AtomicBool::new(false));
        let start_received_reference = received_reference.clone();
        let app = Router::new()
            .route(
                "/api/generate-stream/start",
                post(move |body: Bytes| async move {
                    let body = String::from_utf8_lossy(&body);
                    start_received_reference.store(
                        body.contains("name=\"prompt_audio\"")
                            && body.contains("filename=\"reference.wav\""),
                        Ordering::Relaxed,
                    );
                    Json(json!({
                        "stream_id": "stream-test",
                        "audio_url": "/api/generate-stream/stream-test/audio",
                        "sample_rate": 48_000,
                        "channels": 2
                    }))
                }),
            )
            .route(
                "/api/generate-stream/stream-test/audio",
                get(|| async { Bytes::from_static(&[1, 0, 2, 0, 255, 255, 254, 255]) }),
            )
            .route(
                "/api/generate-stream/stream-test/close",
                post(|| async { Json(json!({ "closed": true })) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let engine = MossNanoOnnxEngine::new(MossNanoTtsConfig {
            enabled: true,
            base_url: format!("http://{address}/"),
            api_key: None,
            default_demo_id: "demo-1".to_owned(),
            voice_map: HashMap::new(),
            cpu_threads: 1,
            connect_timeout: Duration::from_secs(1),
            timeout: Duration::from_secs(2),
            retry_backoff: Duration::from_secs(1),
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let reference_path = directory.path().join("voice.wav");
        tokio::fs::write(&reference_path, b"RIFF reference")
            .await
            .unwrap();
        let (tx, mut rx) = mpsc::channel(2);
        engine
            .synthesize(
                &TtsRequest {
                    text: "hello".to_owned(),
                    language: "en".to_owned(),
                    voice: "F1".to_owned(),
                    reference_audio_path: Some(reference_path),
                },
                TtsChunkSink::new(tx).for_engine(engine.name()),
            )
            .await
            .unwrap();
        let chunk = rx.recv().await.unwrap();
        assert_eq!(chunk.engine, "moss-nano-onnx");
        assert_eq!(chunk.sample_rate, 48_000);
        assert_eq!(chunk.channels, 2);
        assert_eq!(chunk.samples, [1, 2, -1, -2]);
        assert!(received_reference.load(Ordering::Relaxed));
        server.abort();
    }
}
