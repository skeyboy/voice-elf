use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};
use url::Url;

use crate::{config::FunAsrConfig, protocol::INPUT_SAMPLE_RATE};

use super::{
    CompletedTranscriptionEngine, LiveTranscription, NoSpeechDetected, Transcriber, Transcription,
};

const STREAM_FRAME_SAMPLES: usize = 960;

#[derive(Clone, Debug, Serialize)]
pub struct FunAsrRuntimeStatus {
    pub enabled: bool,
    pub healthy: bool,
    pub message: String,
}

impl FunAsrRuntimeStatus {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            healthy: false,
            message: "FunASR 未启用".to_owned(),
        }
    }
}

pub struct FunAsrTranscriber {
    config: Arc<FunAsrConfig>,
}

impl FunAsrTranscriber {
    pub fn new(config: FunAsrConfig) -> Result<Self> {
        let url = Url::parse(&config.websocket_url).context("FUNASR_WEBSOCKET_URL is invalid")?;
        if !matches!(url.scheme(), "ws" | "wss") {
            bail!("FUNASR_WEBSOCKET_URL must use ws:// or wss://");
        }
        if !matches!(config.mode.as_str(), "online" | "2pass") {
            bail!("FUNASR_MODE must be 'online' or '2pass' for live recognition");
        }
        if config.chunk_interval == 0 || config.chunk_size.contains(&0) {
            bail!("FunASR chunk size and interval values must be greater than zero");
        }
        Ok(Self {
            config: Arc::new(config),
        })
    }

    pub async fn runtime_status(&self) -> FunAsrRuntimeStatus {
        let request = match self.websocket_request() {
            Ok(request) => request,
            Err(error) => {
                return FunAsrRuntimeStatus {
                    enabled: true,
                    healthy: false,
                    message: format!("FunASR WebSocket 请求无效: {error}"),
                };
            }
        };
        let result = timeout(self.config.connect_timeout, connect_async(request)).await;
        match result {
            Ok(Ok((mut socket, _))) => {
                let _ = socket.close(None).await;
                FunAsrRuntimeStatus {
                    enabled: true,
                    healthy: true,
                    message: format!("FunASR WebSocket 可连接: {}", self.config.websocket_url),
                }
            }
            Ok(Err(error)) => FunAsrRuntimeStatus {
                enabled: true,
                healthy: false,
                message: format!("FunASR WebSocket 连接失败: {error}"),
            },
            Err(_) => FunAsrRuntimeStatus {
                enabled: true,
                healthy: false,
                message: format!(
                    "FunASR WebSocket 连接超时（{} 秒）",
                    self.config.connect_timeout.as_secs()
                ),
            },
        }
    }

    async fn open_live(
        &self,
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<LiveTranscription> {
        let (socket, _) = timeout(
            self.config.connect_timeout,
            connect_async(
                self.websocket_request()
                    .context("failed to build FunASR WebSocket request")?,
            ),
        )
        .await
        .context("FunASR WebSocket connection timed out")?
        .context("failed to connect to FunASR WebSocket")?;
        let (mut sink, mut stream) = socket.split();
        let init = json!({
            "mode": self.config.mode,
            "chunk_size": self.config.chunk_size,
            "chunk_interval": self.config.chunk_interval,
            "encoder_chunk_look_back": self.config.encoder_chunk_look_back,
            "decoder_chunk_look_back": self.config.decoder_chunk_look_back,
            "audio_fs": INPUT_SAMPLE_RATE,
            "wav_name": "voice-elf-live",
            "wav_format": "pcm",
            "is_speaking": true,
            "hotwords": "",
            "itn": true,
        });
        sink.send(Message::Text(init.to_string().into()))
            .await
            .context("failed to initialize FunASR stream")?;

        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let language = source_language.to_owned();
        let result_timeout = self.config.result_timeout;
        let task = tokio::spawn(async move {
            let operation = async move {
                let mut audio_open = true;
                let mut online_text = String::new();
                let mut offline_text = String::new();
                loop {
                    let event = if audio_open {
                        tokio::select! {
                            audio = audio_rx.recv() => StreamEvent::Audio(audio),
                            message = stream.next() => StreamEvent::Server(message),
                        }
                    } else {
                        StreamEvent::Server(stream.next().await)
                    };
                    match event {
                        StreamEvent::Audio(Some(pcm)) => {
                            sink.send(Message::Binary(pcm16_le_bytes(&pcm).into()))
                                .await
                                .context("failed to send PCM to FunASR")?;
                        }
                        StreamEvent::Audio(None) => {
                            audio_open = false;
                            sink.send(Message::Text(
                                json!({"is_speaking": false, "is_end": true})
                                    .to_string()
                                    .into(),
                            ))
                            .await
                            .context("failed to finish FunASR stream")?;
                        }
                        StreamEvent::Server(Some(Ok(Message::Text(message)))) => {
                            let response: FunAsrResponse =
                                serde_json::from_str(message.as_str())
                                    .context("FunASR returned invalid JSON")?;
                            if let Some(error) = response.error.filter(|value| !value.is_empty()) {
                                bail!("FunASR recognition failed: {error}");
                            }
                            let text = response.text.unwrap_or_default();
                            match response.mode.as_deref() {
                                Some("2pass-online") | Some("online") => {
                                    if !text.is_empty() {
                                        online_text.push_str(&text);
                                        let _ = updates.send(text);
                                    }
                                }
                                Some("2pass-offline") | Some("offline") => {
                                    offline_text.push_str(&text);
                                }
                                _ => {}
                            }
                            if response.is_final == Some(true)
                                || (!audio_open
                                    && response.mode.as_deref() == Some("2pass-offline"))
                            {
                                break;
                            }
                        }
                        StreamEvent::Server(Some(Ok(Message::Ping(payload)))) => {
                            sink.send(Message::Pong(payload)).await?;
                        }
                        StreamEvent::Server(Some(Ok(Message::Close(_))) | None) => break,
                        StreamEvent::Server(Some(Ok(_))) => {}
                        StreamEvent::Server(Some(Err(error))) => {
                            return Err(error).context("FunASR WebSocket receive failed");
                        }
                    }
                }
                let text = if offline_text.trim().is_empty() {
                    online_text
                } else {
                    offline_text
                };
                if text.trim().is_empty() {
                    return Err(NoSpeechDetected.into());
                }
                Ok(Transcription::plain(text, language))
            };
            timeout(result_timeout, operation)
                .await
                .context("FunASR result timed out")?
        });
        Ok(LiveTranscription::new(audio_tx, task))
    }

    fn websocket_request(
        &self,
    ) -> tokio_tungstenite::tungstenite::Result<tokio_tungstenite::tungstenite::http::Request<()>>
    {
        let mut request = self.config.websocket_url.as_str().into_client_request()?;
        request
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("binary"));
        Ok(request)
    }
}

enum StreamEvent {
    Audio(Option<Vec<i16>>),
    Server(Option<std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>),
}

#[derive(Deserialize)]
struct FunAsrResponse {
    mode: Option<String>,
    text: Option<String>,
    is_final: Option<bool>,
    error: Option<String>,
}

fn pcm16_le_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

#[async_trait]
impl CompletedTranscriptionEngine for FunAsrTranscriber {
    fn name(&self) -> &'static str {
        "funasr"
    }

    async fn transcribe_completed(
        &self,
        pcm: &[i16],
        source_language: &str,
    ) -> Result<Transcription> {
        let (updates, _) = mpsc::unbounded_channel();
        self.transcribe_streaming(pcm, source_language, updates)
            .await
    }
}

#[async_trait]
impl Transcriber for FunAsrTranscriber {
    async fn start_live(
        &self,
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Option<LiveTranscription>> {
        Ok(Some(self.open_live(source_language, updates).await?))
    }

    async fn transcribe_streaming(
        &self,
        pcm: &[i16],
        source_language: &str,
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<Transcription> {
        let live = self.open_live(source_language, updates).await?;
        for frame in pcm.chunks(STREAM_FRAME_SAMPLES) {
            live.push(frame)?;
        }
        live.finish().await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            Message,
            handshake::server::{Request, Response},
            http::HeaderValue,
        },
    };

    use super::*;

    #[tokio::test]
    async fn streams_pcm_and_uses_the_offline_correction_as_final_text() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket =
                accept_hdr_async(stream, |request: &Request, mut response: Response| {
                    assert_eq!(
                        request
                            .headers()
                            .get("Sec-WebSocket-Protocol")
                            .and_then(|value| value.to_str().ok()),
                        Some("binary")
                    );
                    response
                        .headers_mut()
                        .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("binary"));
                    Ok(response)
                })
                .await
                .unwrap();
            let init = socket.next().await.unwrap().unwrap();
            let Message::Text(init) = init else {
                panic!("expected initialization JSON");
            };
            let init: serde_json::Value = serde_json::from_str(init.as_str()).unwrap();
            assert_eq!(init["mode"], "2pass");
            assert_eq!(init["audio_fs"], 16_000);

            let audio = socket.next().await.unwrap().unwrap();
            assert!(matches!(audio, Message::Binary(data) if data.len() == 1_920));
            socket
                .send(Message::Text(
                    json!({"mode": "2pass-online", "text": "实时"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();

            let finish = socket.next().await.unwrap().unwrap();
            let Message::Text(finish) = finish else {
                panic!("expected finish JSON");
            };
            let finish: serde_json::Value = serde_json::from_str(finish.as_str()).unwrap();
            assert_eq!(finish["is_speaking"], false);
            assert_eq!(finish["is_end"], true);
            socket
                .send(Message::Text(
                    json!({
                        "mode": "2pass-offline",
                        "text": "实时识别结果。",
                        "is_final": true
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let transcriber = FunAsrTranscriber::new(FunAsrConfig {
            enabled: true,
            manager_script: std::path::PathBuf::from("scripts/funasr.sh"),
            websocket_url: format!("ws://{address}/"),
            mode: "2pass".to_owned(),
            chunk_size: [5, 10, 5],
            chunk_interval: 10,
            encoder_chunk_look_back: 4,
            decoder_chunk_look_back: 0,
            connect_timeout: Duration::from_secs(1),
            result_timeout: Duration::from_secs(2),
        })
        .unwrap();
        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
        let result = transcriber
            .transcribe_streaming(&vec![0; 960], "zh", updates_tx)
            .await
            .unwrap();

        assert_eq!(updates_rx.recv().await.as_deref(), Some("实时"));
        assert_eq!(result.text, "实时识别结果。");
        assert_eq!(result.language, "zh");
        server.await.unwrap();
    }
}
