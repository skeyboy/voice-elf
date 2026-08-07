use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use url::{Host, Url};

const DEFAULT_MOSS_NANO_VOICE_MAP: &str = concat!(
    "F1=demo-1,",
    "ZH_GENTLE=demo-2,",
    "ZH_TAIWAN=demo-3,",
    "M1=demo-4,",
    "ZH_LECTURE=demo-5,",
    "ZH_MONOLOGUE=demo-6,",
    "EN_MOSS=demo-7,",
    "EN_LECTURE=demo-8,",
    "EN_NEWS=demo-9,",
    "EN_GENTLE=demo-10,",
    "EN_EXPRESSIVE=demo-11,",
    "EN_NARRATION=demo-12,",
    "JA_NEWS=demo-13",
);

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub bind: SocketAddr,
    pub web_dist: PathBuf,
    pub media_dir: PathBuf,
    pub backend_mode: BackendMode,
    pub asr: AsrConfig,
    pub moss_transcribe: MossTranscribeConfig,
    pub translator: TranslatorConfig,
    pub tts: TtsConfig,
    pub inference_timeout: Duration,
    pub database_url: Option<String>,
    pub authority: AuthorityConfig,
    pub mail: MailConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmtpSecurity {
    Wrapper,
    StartTls,
    None,
}

#[derive(Clone)]
pub struct MailConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: String,
    pub password: Option<String>,
    pub from_address: String,
    pub from_name: String,
    pub public_url: Option<String>,
    pub reset_expiry: Duration,
}

impl MailConfig {
    pub fn configured(&self) -> bool {
        self.enabled && self.password.is_some() && !self.username.trim().is_empty()
    }
}

impl std::fmt::Debug for MailConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MailConfig")
            .field("enabled", &self.enabled)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("from_address", &self.from_address)
            .field("from_name", &self.from_name)
            .field("public_url", &self.public_url)
            .field("reset_expiry", &self.reset_expiry)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorityMode {
    Standalone,
    Bus,
    Tenant,
}

impl AuthorityMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::Bus => "bus",
            Self::Tenant => "tenant",
        }
    }
}

#[derive(Clone)]
pub struct AuthorityConfig {
    pub mode: AuthorityMode,
    pub base_url: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub check_interval: Duration,
    pub request_timeout: Duration,
}

impl std::fmt::Debug for AuthorityConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorityConfig")
            .field("mode", &self.mode)
            .field("base_url", &self.base_url)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("check_interval", &self.check_interval)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendMode {
    Demo,
    Local,
}

#[derive(Clone, Debug)]
pub struct AsrConfig {
    pub binary: PathBuf,
    pub model_dir: Option<PathBuf>,
    pub stream_unfixed_chunks: usize,
    pub stream_max_new_tokens: usize,
    pub encoder_window_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct MossTranscribeConfig {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    pub max_new_tokens: usize,
}

#[derive(Clone, Debug)]
pub struct TranslatorConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub binary: PathBuf,
    pub model_path: Option<PathBuf>,
    pub threads: usize,
}

#[derive(Clone, Debug)]
pub struct TtsConfig {
    pub kokoro_model_dir: PathBuf,
    pub supertonic_model_dir: PathBuf,
    pub threads: usize,
    pub moss_nano: MossNanoTtsConfig,
    pub index_tts: IndexTtsConfig,
}

#[derive(Clone, Debug)]
pub struct MossNanoTtsConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_demo_id: String,
    pub voice_map: HashMap<String, String>,
    pub cpu_threads: usize,
    pub connect_timeout: Duration,
    pub timeout: Duration,
    pub retry_backoff: Duration,
}

#[derive(Clone, Debug)]
pub struct IndexTtsConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: Option<String>,
    pub manager_script: PathBuf,
    pub model_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub default_voice_id: String,
    pub voice_map: HashMap<String, String>,
    pub connect_timeout: Duration,
    pub timeout: Duration,
    pub retry_backoff: Duration,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let bind = env::var("VOICE_ELF_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3001".to_owned())
            .parse()
            .context("VOICE_ELF_BIND must be a socket address")?;

        let backend_mode = parse_backend_mode(env::var("VOICE_ELF_BACKEND").ok())?;

        let timeout_seconds = env::var("VOICE_ELF_INFERENCE_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(120);

        let authority = authority_config_from_env()?;
        let mail = mail_config_from_env()?;

        Ok(Self {
            bind,
            web_dist: env::var("VOICE_ELF_WEB_DIST")
                .map(resolve_workspace_path)
                .unwrap_or_else(|_| resolve_workspace_path("web/dist")),
            media_dir: env::var("VOICE_ELF_MEDIA_DIR")
                .map(resolve_workspace_path)
                .unwrap_or_else(|_| resolve_workspace_path("media")),
            backend_mode,
            asr: AsrConfig {
                binary: env::var("QWEN_ASR_BINARY")
                    .map(resolve_workspace_executable)
                    .unwrap_or_else(|_| PathBuf::from("qwen_asr")),
                model_dir: env::var("QWEN_ASR_MODEL_DIR")
                    .ok()
                    .map(resolve_workspace_path),
                stream_unfixed_chunks: env::var("QWEN_ASR_STREAM_UNFIXED_CHUNKS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
                stream_max_new_tokens: env::var("QWEN_ASR_STREAM_MAX_NEW_TOKENS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(32),
                encoder_window_seconds: env::var("QWEN_ASR_ENCODER_WINDOW_SECONDS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(4),
            },
            moss_transcribe: MossTranscribeConfig {
                enabled: env_flag("MOSS_TRANSCRIBE_ENABLED"),
                base_url: env::var("MOSS_TRANSCRIBE_BASE_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:8000/v1".to_owned()),
                model: env::var("MOSS_TRANSCRIBE_MODEL")
                    .unwrap_or_else(|_| "OpenMOSS-Team/MOSS-Transcribe-Diarize".to_owned()),
                api_key: env::var("MOSS_TRANSCRIBE_API_KEY")
                    .ok()
                    .filter(|value| !value.trim().is_empty()),
                timeout: Duration::from_secs(
                    env::var("MOSS_TRANSCRIBE_TIMEOUT_SECONDS")
                        .ok()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(300),
                ),
                max_new_tokens: env::var("MOSS_TRANSCRIBE_MAX_NEW_TOKENS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(5_120),
            },
            translator: TranslatorConfig {
                base_url: env::var("LOCAL_LLM_BASE_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".to_owned()),
                model: env::var("LOCAL_LLM_MODEL").unwrap_or_else(|_| "qwen3:4b".to_owned()),
                api_key: env::var("LOCAL_LLM_API_KEY").ok(),
                binary: env::var("LOCAL_LLM_BINARY")
                    .map(resolve_workspace_executable)
                    .unwrap_or_else(|_| PathBuf::from("llama-cli")),
                model_path: env::var("LOCAL_LLM_MODEL_PATH")
                    .ok()
                    .map(resolve_workspace_path),
                threads: env::var("LOCAL_LLM_THREADS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(8),
            },
            tts: TtsConfig {
                kokoro_model_dir: env::var("TTS_KOKORO_MODEL_DIR")
                    .map(resolve_workspace_path)
                    .unwrap_or_else(|_| {
                        resolve_workspace_path(".local/models/tts/kokoro-int8-multi-lang-v1_1")
                    }),
                supertonic_model_dir: env::var("TTS_SUPERTONIC_MODEL_DIR")
                    .map(resolve_workspace_path)
                    .unwrap_or_else(|_| {
                        resolve_workspace_path(
                            ".local/models/tts/sherpa-onnx-supertonic-3-tts-int8-2026-05-11",
                        )
                    }),
                threads: env::var("TTS_THREADS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(2),
                moss_nano: MossNanoTtsConfig {
                    enabled: env_flag_default("TTS_MOSS_NANO_ENABLED", true),
                    base_url: env::var("TTS_MOSS_NANO_BASE_URL")
                        .unwrap_or_else(|_| "http://127.0.0.1:18083/".to_owned()),
                    api_key: env::var("TTS_MOSS_NANO_API_KEY")
                        .ok()
                        .filter(|value| !value.trim().is_empty()),
                    default_demo_id: env::var("TTS_MOSS_NANO_DEFAULT_DEMO_ID")
                        .unwrap_or_else(|_| "demo-1".to_owned()),
                    voice_map: parse_key_value_map(
                        &env::var("TTS_MOSS_NANO_VOICE_MAP")
                            .unwrap_or_else(|_| DEFAULT_MOSS_NANO_VOICE_MAP.to_owned()),
                    ),
                    cpu_threads: env::var("TTS_MOSS_NANO_CPU_THREADS")
                        .ok()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(4),
                    connect_timeout: Duration::from_secs(
                        env::var("TTS_MOSS_NANO_CONNECT_TIMEOUT_SECONDS")
                            .ok()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(2),
                    ),
                    timeout: Duration::from_secs(
                        env::var("TTS_MOSS_NANO_TIMEOUT_SECONDS")
                            .ok()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(180),
                    ),
                    retry_backoff: Duration::from_secs(
                        env::var("TTS_MOSS_NANO_RETRY_BACKOFF_SECONDS")
                            .ok()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(30),
                    ),
                },
                index_tts: IndexTtsConfig {
                    enabled: env_flag("TTS_INDEX_ENABLED"),
                    base_url: env::var("TTS_INDEX_BASE_URL")
                        .unwrap_or_else(|_| "http://127.0.0.1:18084/".to_owned()),
                    api_key: env::var("TTS_INDEX_API_KEY")
                        .ok()
                        .filter(|value| !value.trim().is_empty()),
                    manager_script: env::var("TTS_INDEX_MANAGER_SCRIPT")
                        .map(resolve_workspace_path)
                        .unwrap_or_else(|_| resolve_workspace_path("scripts/index-tts.sh")),
                    model_dir: env::var("TTS_INDEX_MODEL_DIR")
                        .map(resolve_workspace_path)
                        .unwrap_or_else(|_| resolve_workspace_path(".local/models/tts/index-tts2")),
                    runtime_dir: env::var("TTS_INDEX_RUNTIME_DIR")
                        .map(resolve_workspace_path)
                        .unwrap_or_else(|_| resolve_workspace_path(".local/run/index-tts")),
                    default_voice_id: env::var("TTS_INDEX_DEFAULT_VOICE_ID")
                        .unwrap_or_else(|_| "F1".to_owned()),
                    voice_map: parse_key_value_map(
                        &env::var("TTS_INDEX_VOICE_MAP").unwrap_or_default(),
                    ),
                    connect_timeout: Duration::from_secs(
                        env::var("TTS_INDEX_CONNECT_TIMEOUT_SECONDS")
                            .ok()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(3),
                    ),
                    timeout: Duration::from_secs(
                        env::var("TTS_INDEX_TIMEOUT_SECONDS")
                            .ok()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(300),
                    ),
                    retry_backoff: Duration::from_secs(
                        env::var("TTS_INDEX_RETRY_BACKOFF_SECONDS")
                            .ok()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(30),
                    ),
                },
            },
            inference_timeout: Duration::from_secs(timeout_seconds),
            database_url: env::var("DATABASE_URL").ok().filter(|url| !url.is_empty()),
            authority,
            mail,
        })
    }

    pub fn validate_local(&self) -> Result<()> {
        if self.backend_mode != BackendMode::Local {
            return Ok(());
        }
        let asr_model = self
            .asr
            .model_dir
            .as_ref()
            .context("QWEN_ASR_MODEL_DIR is required when VOICE_ELF_BACKEND=local")?;
        if !asr_model.is_dir() {
            anyhow::bail!(
                "QWEN_ASR_MODEL_DIR is not a directory: {}",
                asr_model.display()
            );
        }
        if let Some(model) = &self.translator.model_path
            && !model.is_file()
        {
            anyhow::bail!("LOCAL_LLM_MODEL_PATH is not a file: {}", model.display());
        }
        Ok(())
    }
}

fn mail_config_from_env() -> Result<MailConfig> {
    let security = match env::var("VOICE_ELF_SMTP_SECURITY")
        .unwrap_or_else(|_| "wrapper".to_owned())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "wrapper" | "tls" | "ssl" => SmtpSecurity::Wrapper,
        "starttls" => SmtpSecurity::StartTls,
        "none" => SmtpSecurity::None,
        value => anyhow::bail!(
            "VOICE_ELF_SMTP_SECURITY must be 'wrapper', 'starttls', or 'none', got '{value}'"
        ),
    };
    let username =
        env::var("VOICE_ELF_SMTP_USERNAME").unwrap_or_else(|_| "lylapp@163.com".to_owned());
    let from_address = env::var("VOICE_ELF_SMTP_FROM_ADDRESS").unwrap_or_else(|_| username.clone());
    let public_url = env::var("VOICE_ELF_PUBLIC_URL")
        .ok()
        .map(|value| value.trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty());
    if let Some(value) = &public_url {
        let url = Url::parse(value).context("VOICE_ELF_PUBLIC_URL must be a valid URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || url.username() != ""
            || url.password().is_some()
        {
            anyhow::bail!("VOICE_ELF_PUBLIC_URL must be an HTTP(S) origin without credentials");
        }
    }
    Ok(MailConfig {
        enabled: env_flag_default("VOICE_ELF_SMTP_ENABLED", true),
        host: env::var("VOICE_ELF_SMTP_HOST").unwrap_or_else(|_| "smtp.163.com".to_owned()),
        port: env::var("VOICE_ELF_SMTP_PORT")
            .ok()
            .map(|value| value.parse())
            .transpose()
            .context("VOICE_ELF_SMTP_PORT must be a valid port")?
            .unwrap_or(465),
        security,
        username,
        password: env::var("VOICE_ELF_SMTP_PASSWORD")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        from_address,
        from_name: env::var("VOICE_ELF_SMTP_FROM_NAME").unwrap_or_else(|_| "Voice Elf".to_owned()),
        public_url,
        reset_expiry: Duration::from_secs(
            env::var("VOICE_ELF_PASSWORD_RESET_MINUTES")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(30)
                .clamp(5, 1440)
                * 60,
        ),
    })
}

fn parse_backend_mode(value: Option<String>) -> Result<BackendMode> {
    let value = value
        .unwrap_or_else(|| "local".to_owned())
        .to_ascii_lowercase();
    match value.as_str() {
        "demo" => Ok(BackendMode::Demo),
        "local" => Ok(BackendMode::Local),
        value => anyhow::bail!("VOICE_ELF_BACKEND must be 'demo' or 'local', got '{value}'"),
    }
}

fn authority_config_from_env() -> Result<AuthorityConfig> {
    let mode = match env::var("VOICE_ELF_AUTHORITY_MODE")
        .unwrap_or_else(|_| "standalone".to_owned())
        .to_ascii_lowercase()
        .as_str()
    {
        "standalone" => AuthorityMode::Standalone,
        "bus" => AuthorityMode::Bus,
        "tenant" => AuthorityMode::Tenant,
        value => anyhow::bail!(
            "VOICE_ELF_AUTHORITY_MODE must be 'standalone', 'bus', or 'tenant', got '{value}'"
        ),
    };
    let base_url = env::var("VOICE_ELF_AUTHORITY_URL")
        .ok()
        .map(|value| value.trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty());
    let client_id = env::var("VOICE_ELF_AUTHORITY_CLIENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let client_secret = env::var("VOICE_ELF_AUTHORITY_CLIENT_SECRET")
        .ok()
        .filter(|value| !value.trim().is_empty());

    if mode == AuthorityMode::Tenant {
        let value = base_url
            .as_deref()
            .context("VOICE_ELF_AUTHORITY_URL is required in tenant mode")?;
        validate_authority_url(value)?;
        if client_id.is_none() {
            anyhow::bail!("VOICE_ELF_AUTHORITY_CLIENT_ID is required in tenant mode");
        }
        if client_secret.is_none() {
            anyhow::bail!("VOICE_ELF_AUTHORITY_CLIENT_SECRET is required in tenant mode");
        }
    }

    Ok(AuthorityConfig {
        mode,
        base_url,
        client_id,
        client_secret,
        check_interval: Duration::from_secs(
            env::var("VOICE_ELF_AUTHORITY_CHECK_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(300)
                .max(30),
        ),
        request_timeout: Duration::from_secs(
            env::var("VOICE_ELF_AUTHORITY_TIMEOUT_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(10)
                .clamp(2, 60),
        ),
    })
}

fn validate_authority_url(value: &str) -> Result<()> {
    let url = Url::parse(value).context("VOICE_ELF_AUTHORITY_URL must be a valid URL")?;
    if url.scheme() == "https" {
        return Ok(());
    }
    let loopback = matches!(url.host(), Some(Host::Domain("localhost")))
        || matches!(url.host(), Some(Host::Ipv4(address)) if address.is_loopback())
        || matches!(url.host(), Some(Host::Ipv6(address)) if address.is_loopback());
    if url.scheme() != "http" || !loopback {
        anyhow::bail!("VOICE_ELF_AUTHORITY_URL must use HTTPS except for loopback development");
    }
    Ok(())
}

fn env_flag(name: &str) -> bool {
    env_flag_default(name, false)
}

fn env_flag_default(name: &str, default: bool) -> bool {
    env::var(name).map_or(default, |value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn parse_key_value_map(value: &str) -> HashMap<String, String> {
    value
        .split(',')
        .filter_map(|entry| entry.split_once('='))
        .map(|(key, value)| (key.trim().to_ascii_uppercase(), value.trim().to_owned()))
        .filter(|(key, value)| !key.is_empty() && !value.is_empty())
        .collect()
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("server crate must be inside the workspace")
}

fn resolve_workspace_path(path: impl Into<PathBuf>) -> PathBuf {
    let path = path.into();
    if path.is_absolute() {
        path
    } else {
        workspace_root().join(path)
    }
}

fn resolve_workspace_executable(path: impl Into<PathBuf>) -> PathBuf {
    let path = path.into();
    if path.is_absolute() || path.components().count() == 1 {
        path
    } else {
        workspace_root().join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_demo_backend_to_be_explicit() {
        assert_eq!(parse_backend_mode(None).unwrap(), BackendMode::Local);
        assert_eq!(
            parse_backend_mode(Some("demo".to_owned())).unwrap(),
            BackendMode::Demo
        );
    }

    #[test]
    fn resolves_resource_paths_from_the_workspace_root() {
        assert_eq!(
            resolve_workspace_path(".local/models/qwen3-asr-0.6b"),
            workspace_root().join(".local/models/qwen3-asr-0.6b")
        );
        assert_eq!(
            resolve_workspace_executable(".local/bin/qwen_asr"),
            workspace_root().join(".local/bin/qwen_asr")
        );
    }

    #[test]
    fn preserves_absolute_paths_and_path_commands() {
        let absolute = workspace_root().join("models/asr");
        assert_eq!(resolve_workspace_path(&absolute), absolute);
        assert_eq!(
            resolve_workspace_executable("qwen_asr"),
            PathBuf::from("qwen_asr")
        );
    }

    #[test]
    fn default_moss_voice_map_contains_each_unique_reference_voice() {
        let voices = parse_key_value_map(DEFAULT_MOSS_NANO_VOICE_MAP);
        assert_eq!(voices.len(), 13);
        assert_eq!(voices.get("F1").map(String::as_str), Some("demo-1"));
        assert_eq!(voices.get("M1").map(String::as_str), Some("demo-4"));
        assert_eq!(voices.get("JA_NEWS").map(String::as_str), Some("demo-13"));
    }
}
