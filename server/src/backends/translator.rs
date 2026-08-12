use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, process::Command, sync::mpsc, time::timeout};

use crate::config::TranslatorConfig;

use super::{TranslationTerm, Translator, language_name, short_demo_delay};

pub struct DemoTranslator;

impl DemoTranslator {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Translator for DemoTranslator {
    async fn translate_streaming(
        &self,
        text: &str,
        _source_language: &str,
        target_language: &str,
        _terminology: &[TranslationTerm],
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        short_demo_delay(Duration::from_millis(110)).await;
        let translated = match target_language {
            "zh" => "已收到语音，实时翻译链路运行正常。".to_owned(),
            "ja" => "音声を受信しました。リアルタイム翻訳は正常に動作しています。".to_owned(),
            "ko" => "음성을 수신했습니다. 실시간 번역이 정상적으로 작동합니다.".to_owned(),
            "fr" => "Audio recu. La traduction en temps reel fonctionne correctement.".to_owned(),
            "de" => "Audio empfangen. Die Echtzeitubersetzung funktioniert.".to_owned(),
            "es" => {
                "Audio recibido. La traduccion en tiempo real funciona correctamente.".to_owned()
            }
            "en" => "Audio received. Real-time translation is working.".to_owned(),
            _ => format!("[{target_language}] {text}"),
        };
        for chunk in translated.chars().collect::<Vec<_>>().chunks(4) {
            let _ = updates.send(chunk.iter().collect());
            short_demo_delay(Duration::from_millis(20)).await;
        }
        Ok(translated)
    }
}

pub struct LocalLlmTranslator {
    client: Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl LocalLlmTranslator {
    pub fn new(config: TranslatorConfig, timeout: Duration) -> Result<Self> {
        let endpoint = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        Ok(Self {
            client: Client::builder().timeout(timeout).build()?,
            endpoint,
            model: config.model,
            api_key: config.api_key,
        })
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

#[async_trait]
impl Translator for LocalLlmTranslator {
    async fn translate_streaming(
        &self,
        text: &str,
        source_language: &str,
        target_language: &str,
        terminology: &[TranslationTerm],
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let system = format!(
            "Translate from {} to {}. Return only the translation. Preserve names, numbers, and meaning.{}",
            language_name(source_language),
            language_name(target_language),
            glossary_instruction(terminology)
        );
        let request = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: &system,
                },
                ChatMessage {
                    role: "user",
                    content: text,
                },
            ],
            temperature: 0.1,
            stream: false,
        };

        let mut builder = self.client.post(&self.endpoint).json(&request);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }
        let response = builder
            .send()
            .await
            .context("local LLM request failed")?
            .error_for_status()
            .context("local LLM returned an error status")?
            .json::<ChatResponse>()
            .await
            .context("invalid local LLM response")?;
        let translated = response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content.trim().to_owned())
            .filter(|text| !text.is_empty())
            .context("local LLM returned no translation")?;
        if translated == text && source_language != target_language {
            bail!("local LLM returned the source text unchanged");
        }
        let _ = updates.send(translated.clone());
        Ok(translated)
    }
}

pub struct LlamaCppTranslator {
    binary: PathBuf,
    model_path: PathBuf,
    threads: usize,
    timeout: Duration,
}

impl LlamaCppTranslator {
    pub fn new(config: TranslatorConfig, timeout: Duration) -> Result<Self> {
        Ok(Self {
            binary: config.binary,
            model_path: config
                .model_path
                .context("LOCAL_LLM_MODEL_PATH is required for llama.cpp translation")?,
            threads: config.threads.max(1),
            timeout,
        })
    }
}

#[async_trait]
impl Translator for LlamaCppTranslator {
    async fn translate_streaming(
        &self,
        text: &str,
        source_language: &str,
        target_language: &str,
        terminology: &[TranslationTerm],
        updates: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let prompt = format!(
            "<|im_start|>system\nTranslate from {} to {}. Return only the translation. Preserve names, numbers, and meaning.{} /no_think<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
            language_name(source_language),
            language_name(target_language),
            glossary_instruction(terminology),
            text
        );
        let mut child = Command::new(&self.binary)
            .arg("-m")
            .arg(&self.model_path)
            .arg("-p")
            .arg(prompt)
            .arg("-n")
            .arg("256")
            .arg("-t")
            .arg(self.threads.to_string())
            .arg("--temp")
            .arg("0.1")
            .arg("--top-p")
            .arg("0.9")
            .arg("--no-display-prompt")
            .arg("--no-warmup")
            .arg("--single-turn")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start llama.cpp translator at {}",
                    self.binary.display()
                )
            })?;
        let mut stdout = child
            .stdout
            .take()
            .context("llama.cpp translator stdout was unavailable")?;
        let mut stderr = child
            .stderr
            .take()
            .context("llama.cpp translator stderr was unavailable")?;
        let operation = async move {
            let output_reader = async move {
                let mut output = Vec::new();
                let mut pending = Vec::new();
                let mut stream = LlamaOutputStream::default();
                let mut buffer = [0_u8; 256];
                loop {
                    let read = stdout.read(&mut buffer).await?;
                    if read == 0 {
                        break;
                    }
                    output.extend_from_slice(&buffer[..read]);
                    pending.extend_from_slice(&buffer[..read]);
                    if let Some(text) = take_valid_utf8(&mut pending)? {
                        stream.push(&text, &updates);
                    }
                }
                if !pending.is_empty() {
                    bail!("llama.cpp translation ended with invalid UTF-8");
                }
                Result::<Vec<u8>>::Ok(output)
            };
            let error_reader = async move {
                let mut output = Vec::new();
                stderr.read_to_end(&mut output).await?;
                Result::<Vec<u8>>::Ok(output)
            };
            tokio::try_join!(output_reader, error_reader)
        };
        let (stdout, stderr) = timeout(self.timeout, operation)
            .await
            .context("local Qwen translator timed out")??;
        let status = child.wait().await?;
        if !status.success() {
            bail!(
                "local Qwen translator failed: {}",
                String::from_utf8_lossy(&stderr).trim()
            );
        }
        let raw = String::from_utf8(stdout).context("local Qwen translation was not UTF-8")?;
        let translated = clean_llama_output(&raw);
        if translated.is_empty() {
            bail!("local Qwen translator returned an empty translation");
        }
        Ok(translated)
    }
}

fn glossary_instruction(terms: &[TranslationTerm]) -> String {
    if terms.is_empty() {
        return String::new();
    }
    let mappings = terms
        .iter()
        .take(200)
        .map(|term| {
            let aliases = if term.aliases.is_empty() {
                String::new()
            } else {
                format!(" (aliases: {})", term.aliases.join(", "))
            };
            format!("{}{} => {}", term.source, aliases, term.target)
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(" Apply this mandatory terminology glossary exactly: {mappings}.")
}

fn take_valid_utf8(pending: &mut Vec<u8>) -> Result<Option<String>> {
    let mut output = String::new();
    match std::str::from_utf8(pending) {
        Ok(text) => {
            if !text.is_empty() {
                output.push_str(text);
            }
            pending.clear();
            Ok((!output.is_empty()).then_some(output))
        }
        Err(error) if error.error_len().is_none() => {
            let valid = error.valid_up_to();
            if valid > 0 {
                output.push_str(std::str::from_utf8(&pending[..valid])?);
                pending.drain(..valid);
            }
            Ok((!output.is_empty()).then_some(output))
        }
        Err(error) => bail!("llama.cpp translation output was not UTF-8: {error}"),
    }
}

#[derive(Default)]
struct LlamaOutputStream {
    raw: String,
    emitted: String,
}

impl LlamaOutputStream {
    fn push(&mut self, chunk: &str, updates: &mpsc::UnboundedSender<String>) {
        self.raw.push_str(chunk);
        let visible = clean_llama_stream(&self.raw);
        let Some(delta) = visible.strip_prefix(&self.emitted) else {
            return;
        };
        if !delta.is_empty() {
            let _ = updates.send(delta.to_owned());
            self.emitted = visible;
        }
    }
}

const TERMINAL_MARKERS: [&str; 4] = [
    "[end of text]",
    "> EOF by user",
    "<|im_end|>",
    "<|endoftext|>",
];

fn clean_llama_stream(output: &str) -> String {
    let mut visible = if output.contains("<think>") {
        let Some((_, answer)) = output.rsplit_once("</think>") else {
            return String::new();
        };
        answer
    } else {
        output
    };
    if let Some(index) = TERMINAL_MARKERS
        .iter()
        .filter_map(|marker| visible.find(marker))
        .min()
    {
        visible = &visible[..index];
    }
    let mut end = visible.len();
    for marker in TERMINAL_MARKERS {
        for (index, _) in marker.char_indices().skip(1) {
            if visible[..end].ends_with(&marker[..index]) {
                end -= index;
                break;
            }
        }
    }
    visible[..end].trim_start().to_owned()
}

fn clean_llama_output(output: &str) -> String {
    let assistant_output = output
        .rsplit_once("\nAssistant:\n")
        .map(|(_, answer)| answer)
        .unwrap_or(output);
    let without_thinking = assistant_output
        .rsplit_once("</think>")
        .map(|(_, answer)| answer)
        .unwrap_or(assistant_output);
    without_thinking
        .replace("<|im_end|>", "")
        .replace("<|endoftext|>", "")
        .split("[end of text]")
        .next()
        .unwrap_or_default()
        .split("> EOF by user")
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::{LlamaOutputStream, clean_llama_output};

    #[test]
    fn removes_qwen_thinking_and_special_tokens() {
        assert_eq!(
            clean_llama_output("<think>reasoning</think>\n你好。<|im_end|>"),
            "你好。"
        );
        assert_eq!(
            clean_llama_output("User:\nhello\n\nAssistant:\n你好。\n"),
            "你好。"
        );
    }

    #[test]
    fn streams_only_translation_content() {
        let (updates, mut output) = mpsc::unbounded_channel();
        let mut stream = LlamaOutputStream::default();
        stream.push("<think>ignored", &updates);
        assert!(output.try_recv().is_err());
        stream.push("</think>\n\nHello", &updates);
        assert_eq!(output.try_recv().unwrap(), "Hello");
        stream.push(" world.[end", &updates);
        assert_eq!(output.try_recv().unwrap(), " world.");
        stream.push(" of text]\n", &updates);
        assert!(output.try_recv().is_err());
    }
}
