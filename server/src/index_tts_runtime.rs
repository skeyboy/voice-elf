use std::{
    fs::{self, OpenOptions},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Url};
use serde::Serialize;
use tokio::{process::Command, time::timeout};

use crate::config::IndexTtsConfig;

const REQUIRED_MODEL_FILES: &[&str] = &[
    "config.yaml",
    "bpe.model",
    "gpt.pth",
    "s2mel.pth",
    "wav2vec2bert_stats.pt",
];

#[derive(Clone, Debug, Serialize)]
pub struct IndexTtsRuntimeStatus {
    pub phase: String,
    pub script_available: bool,
    pub model_ready: bool,
    pub running: bool,
    pub healthy: bool,
    pub action: Option<String>,
    pub message: String,
    pub model_dir: String,
    pub log_path: String,
}

#[derive(Default)]
struct RuntimeState {
    action: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone)]
pub struct IndexTtsRuntime {
    config: IndexTtsConfig,
    health_url: Url,
    client: Client,
    state: Arc<Mutex<RuntimeState>>,
}

impl IndexTtsRuntime {
    pub fn new(config: IndexTtsConfig) -> Result<Self> {
        let health_url = Url::parse(&config.base_url)
            .context("TTS_INDEX_BASE_URL must be a valid URL")?
            .join("health")
            .context("failed to resolve IndexTTS health endpoint")?;
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(Duration::from_secs(5))
            .build()
            .context("failed to create IndexTTS health client")?;
        Ok(Self {
            config,
            health_url,
            client,
            state: Arc::new(Mutex::new(RuntimeState::default())),
        })
    }

    pub async fn status(&self) -> IndexTtsRuntimeStatus {
        let script_available = self.config.manager_script.is_file();
        let model_ready = model_is_ready(&self.config);
        let (action, last_error) = self
            .state
            .lock()
            .map(|state| (state.action.clone(), state.last_error.clone()))
            .unwrap_or_else(|_| (None, Some("IndexTTS 运行时状态锁不可用".to_owned())));
        let healthy = model_ready && self.health_check().await;
        let (running, process_message) = if script_available && action.is_none() && !healthy {
            self.process_status().await
        } else {
            (healthy, String::new())
        };
        let (phase, message) = if let Some(action) = &action {
            let message = match action.as_str() {
                "installing" => "正在下载模型并安装 IndexTTS2 运行环境",
                "starting" => "正在启动 IndexTTS2 模型服务",
                "stopping" => "正在停止 IndexTTS2 模型服务",
                _ => "正在执行 IndexTTS2 管理操作",
            };
            (action.clone(), message.to_owned())
        } else if !script_available {
            (
                "unavailable".to_owned(),
                "IndexTTS2 管理脚本不存在".to_owned(),
            )
        } else if !model_ready {
            (
                "not_installed".to_owned(),
                "模型尚未安装，下载约需 5.9 GB".to_owned(),
            )
        } else if healthy {
            ("ready".to_owned(), "IndexTTS2 模型服务运行正常".to_owned())
        } else if running {
            (
                "starting".to_owned(),
                if process_message.is_empty() {
                    "模型已加载，服务仍在初始化".to_owned()
                } else {
                    process_message
                },
            )
        } else if let Some(error) = last_error {
            ("error".to_owned(), error)
        } else {
            ("stopped".to_owned(), "模型已安装，服务尚未启动".to_owned())
        };
        IndexTtsRuntimeStatus {
            phase,
            script_available,
            model_ready,
            running,
            healthy,
            action,
            message,
            model_dir: self.config.model_dir.display().to_string(),
            log_path: self
                .config
                .runtime_dir
                .join("manager.log")
                .display()
                .to_string(),
        }
    }

    pub async fn run_action(&self, action: &str) -> Result<IndexTtsRuntimeStatus> {
        let (script_action, state_action) = match action {
            "install" => ("enable", "installing"),
            "start" => ("start", "starting"),
            "stop" => ("stop", "stopping"),
            _ => bail!("unsupported IndexTTS action"),
        };
        if !self.config.manager_script.is_file() {
            bail!(
                "IndexTTS manager script does not exist: {}",
                self.config.manager_script.display()
            );
        }
        if action == "start" && !model_is_ready(&self.config) {
            bail!("IndexTTS model is not installed");
        }
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("IndexTTS runtime state lock was poisoned"))?;
            if let Some(active) = &state.action {
                bail!("IndexTTS action '{active}' is already running");
            }
            state.action = Some(state_action.to_owned());
            state.last_error = None;
        }
        let runtime = self.clone();
        let script_action = script_action.to_owned();
        tokio::spawn(async move {
            let result = runtime.execute(&script_action).await;
            if let Ok(mut state) = runtime.state.lock() {
                state.action = None;
                state.last_error = result.err().map(|error| format!("{error:#}"));
            }
        });
        Ok(self.status().await)
    }

    async fn execute(&self, action: &str) -> Result<()> {
        fs::create_dir_all(&self.config.runtime_dir).with_context(|| {
            format!(
                "failed to create IndexTTS runtime directory: {}",
                self.config.runtime_dir.display()
            )
        })?;
        let log_path = self.config.runtime_dir.join("manager.log");
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("failed to open {}", log_path.display()))?;
        let stderr = stdout
            .try_clone()
            .context("failed to clone IndexTTS manager log handle")?;
        let status = Command::new(&self.config.manager_script)
            .arg(action)
            .env("TTS_INDEX_MODEL_DIR", &self.config.model_dir)
            .env("INDEX_TTS_MODEL_DIR", &self.config.model_dir)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .status()
            .await
            .with_context(|| {
                format!(
                    "failed to run IndexTTS manager: {}",
                    self.config.manager_script.display()
                )
            })?;
        if !status.success() {
            bail!(
                "IndexTTS manager action '{action}' failed with {status}; see {}",
                log_path.display()
            );
        }
        Ok(())
    }

    async fn health_check(&self) -> bool {
        let request = self.client.get(self.health_url.clone());
        let request = match &self.config.api_key {
            Some(api_key) => request.bearer_auth(api_key),
            None => request,
        };
        request
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    async fn process_status(&self) -> (bool, String) {
        let output = timeout(
            Duration::from_secs(5),
            Command::new(&self.config.manager_script)
                .arg("status")
                .env("TTS_INDEX_MODEL_DIR", &self.config.model_dir)
                .env("INDEX_TTS_MODEL_DIR", &self.config.model_dir)
                .output(),
        )
        .await;
        match output {
            Ok(Ok(output)) if output.status.success() => (
                true,
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            ),
            _ => (false, String::new()),
        }
    }
}

fn model_is_ready(config: &IndexTtsConfig) -> bool {
    REQUIRED_MODEL_FILES
        .iter()
        .all(|file| config.model_dir.join(file).is_file())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use axum::{Json, Router, routing::get};
    use tempfile::tempdir;

    use super::*;

    fn config(model_dir: std::path::PathBuf) -> IndexTtsConfig {
        IndexTtsConfig {
            enabled: false,
            base_url: "http://127.0.0.1:18084/".to_owned(),
            api_key: None,
            manager_script: model_dir.join("index-tts.sh"),
            runtime_dir: model_dir.join("run"),
            model_dir,
            default_voice_id: "F1".to_owned(),
            voice_map: HashMap::new(),
            connect_timeout: Duration::from_millis(50),
            timeout: Duration::from_secs(1),
            retry_backoff: Duration::from_secs(1),
        }
    }

    #[test]
    fn requires_the_complete_official_checkpoint_set() {
        let directory = tempdir().unwrap();
        let config = config(directory.path().to_owned());
        assert!(!model_is_ready(&config));
        for file in REQUIRED_MODEL_FILES {
            fs::write(directory.path().join(file), b"test").unwrap();
        }
        assert!(model_is_ready(&config));
    }

    #[tokio::test]
    async fn reports_ready_only_after_the_sidecar_health_check_passes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/health",
            get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let directory = tempdir().unwrap();
        for file in REQUIRED_MODEL_FILES {
            fs::write(directory.path().join(file), b"test").unwrap();
        }
        fs::write(directory.path().join("index-tts.sh"), b"#!/bin/sh\n").unwrap();
        let mut config = config(directory.path().to_owned());
        config.base_url = format!("http://{address}/");
        let runtime = IndexTtsRuntime::new(config).unwrap();

        let status = runtime.status().await;
        assert_eq!(status.phase, "ready");
        assert!(status.model_ready);
        assert!(status.running);
        assert!(status.healthy);
    }

    #[tokio::test]
    async fn refuses_to_start_an_incomplete_model() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("index-tts.sh"), b"#!/bin/sh\n").unwrap();
        let runtime = IndexTtsRuntime::new(config(directory.path().to_owned())).unwrap();
        let error = runtime.run_action("start").await.unwrap_err();
        assert!(error.to_string().contains("model is not installed"));
    }
}
