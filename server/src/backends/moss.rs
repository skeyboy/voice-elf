use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::{Client, multipart};
use serde::Deserialize;
use url::Url;

use crate::{audio::pcm16_wav_bytes, config::MossTranscribeConfig, protocol::INPUT_SAMPLE_RATE};

use super::{CompletedTranscriptionEngine, Transcription, TranscriptionSegment};

pub struct MossTranscribeEngine {
    client: Client,
    endpoint: Url,
    model: String,
    api_key: Option<String>,
    max_new_tokens: usize,
}

impl MossTranscribeEngine {
    pub fn new(config: MossTranscribeConfig) -> Result<Self> {
        let endpoint = transcription_endpoint(&config.base_url)?;
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .context("failed to build MOSS transcription client")?;
        Ok(Self {
            client,
            endpoint,
            model: config.model,
            api_key: config.api_key,
            max_new_tokens: config.max_new_tokens.clamp(256, 65_536),
        })
    }
}

#[async_trait]
impl CompletedTranscriptionEngine for MossTranscribeEngine {
    fn name(&self) -> &'static str {
        "moss-transcribe-diarize"
    }

    async fn transcribe_completed(
        &self,
        pcm: &[i16],
        source_language: &str,
    ) -> Result<Transcription> {
        let audio = pcm16_wav_bytes(pcm, INPUT_SAMPLE_RATE)
            .context("failed to encode MOSS transcription audio")?;
        let file = multipart::Part::bytes(audio)
            .file_name("utterance.wav")
            .mime_str("audio/wav")?;
        let mut form = multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "verbose_json")
            .text("temperature", "0")
            .text("max_new_tokens", self.max_new_tokens.to_string())
            .part("file", file);
        if source_language != "auto" {
            form = form.text("language", source_language.to_owned());
        }
        let mut request = self.client.post(self.endpoint.clone()).multipart(form);
        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .send()
            .await
            .context("MOSS transcription request failed")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read MOSS transcription response")?;
        if !status.is_success() {
            let diagnostic = body.chars().take(1_000).collect::<String>();
            bail!("MOSS transcription returned HTTP {status}: {diagnostic}");
        }
        let payload: MossResponse = serde_json::from_str(&body).with_context(|| {
            format!(
                "invalid MOSS transcription JSON: {}",
                body.chars().take(300).collect::<String>()
            )
        })?;
        let segments = payload
            .segments
            .into_iter()
            .map(|segment| TranscriptionSegment {
                start: segment.start,
                end: segment.end,
                speaker: segment.speaker,
                text: segment.text,
            })
            .collect::<Vec<_>>();
        let text = if segments.is_empty() {
            payload.text.trim().to_owned()
        } else {
            segments
                .iter()
                .map(|segment| segment.text.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        if text.is_empty() {
            bail!("MOSS transcription returned empty text");
        }
        Ok(Transcription {
            text,
            language: payload
                .language
                .filter(|language| !language.trim().is_empty())
                .unwrap_or_else(|| source_language.to_owned()),
            segments,
        })
    }
}

#[derive(Deserialize)]
struct MossResponse {
    text: String,
    language: Option<String>,
    #[serde(default)]
    segments: Vec<MossSegment>,
}

#[derive(Deserialize)]
struct MossSegment {
    start: f64,
    end: f64,
    #[serde(default, alias = "speaker_label")]
    speaker: Option<String>,
    text: String,
}

fn transcription_endpoint(base_url: &str) -> Result<Url> {
    let base_url = base_url.trim_end_matches('/');
    let endpoint = if base_url.ends_with("/audio/transcriptions") {
        base_url.to_owned()
    } else {
        format!("{base_url}/audio/transcriptions")
    };
    Url::parse(&endpoint).context("MOSS_TRANSCRIBE_BASE_URL must be a valid HTTP URL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_openai_base_or_full_transcription_url() {
        assert_eq!(
            transcription_endpoint("http://127.0.0.1:8000/v1")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            transcription_endpoint("http://127.0.0.1:8000/v1/audio/transcriptions")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
    }

    #[test]
    fn parses_verbose_speaker_segments() {
        let payload: MossResponse = serde_json::from_str(
            r#"{
                "text": "[0.10][S01]hello[0.70]",
                "language": "en",
                "segments": [{
                    "start": 0.1,
                    "end": 0.7,
                    "speaker_label": "S01",
                    "text": "hello"
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(payload.language.as_deref(), Some("en"));
        assert_eq!(payload.segments[0].speaker.as_deref(), Some("S01"));
        assert_eq!(payload.segments[0].text, "hello");
    }
}
