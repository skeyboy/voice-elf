use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    authority::AuthorityService,
    backends::{
        AppServices, INDEX_TTS_ID, QWEN_TTS_ID, TtsBackendInfo, TtsBackendRegistry, TtsVoiceInfo,
    },
    config::{AuthorityMode, IndexTtsConfig, QwenTtsConfig},
    index_tts_runtime::{IndexTtsRuntime, IndexTtsRuntimeStatus},
    storage::Database,
};

#[derive(Clone, Debug, Serialize)]
pub struct EffectiveTtsSelection {
    pub backend_id: String,
    pub source: String,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct QwenTtsRuntimeStatus {
    pub enabled: bool,
    pub healthy: bool,
    pub message: String,
    pub base_url: String,
    pub model: String,
}

#[derive(Clone)]
pub struct TtsManager {
    registry: TtsBackendRegistry,
    database: Option<Database>,
    authority: AuthorityService,
    index_runtime: IndexTtsRuntime,
    qwen_config: QwenTtsConfig,
    qwen_client: Client,
    qwen_base_url: Url,
}

impl TtsManager {
    pub fn new(
        registry: TtsBackendRegistry,
        database: Option<Database>,
        authority: AuthorityService,
        index_config: IndexTtsConfig,
        qwen_config: QwenTtsConfig,
    ) -> Result<Self> {
        let mut qwen_base_url =
            Url::parse(&qwen_config.base_url).context("TTS_QWEN_BASE_URL must be a valid URL")?;
        if !qwen_base_url.path().ends_with('/') {
            qwen_base_url.set_path(&format!("{}/", qwen_base_url.path()));
        }
        let qwen_client = Client::builder()
            .connect_timeout(qwen_config.connect_timeout)
            .timeout(qwen_config.connect_timeout)
            .build()
            .context("failed to create Qwen3-TTS health client")?;
        Ok(Self {
            registry,
            database,
            authority,
            index_runtime: IndexTtsRuntime::new(index_config)?,
            qwen_config,
            qwen_client,
            qwen_base_url,
        })
    }

    pub async fn providers(&self) -> Vec<TtsBackendInfo> {
        let status = self.index_runtime.status().await;
        let qwen_status = self.qwen_runtime_status().await;
        self.providers_with_status(&status, &qwen_status)
    }

    pub fn providers_with_status(
        &self,
        status: &IndexTtsRuntimeStatus,
        qwen_status: &QwenTtsRuntimeStatus,
    ) -> Vec<TtsBackendInfo> {
        let mut providers = self.registry.providers();
        if let Some(provider) = providers
            .iter_mut()
            .find(|provider| provider.id == INDEX_TTS_ID)
        {
            provider.available = status.healthy;
        }
        if let Some(provider) = providers
            .iter_mut()
            .find(|provider| provider.id == QWEN_TTS_ID)
        {
            provider.available = qwen_status.healthy;
        }
        providers
    }

    pub async fn qwen_runtime_status(&self) -> QwenTtsRuntimeStatus {
        if !self.qwen_config.enabled {
            return QwenTtsRuntimeStatus {
                enabled: false,
                healthy: false,
                message: "Qwen3-TTS 尚未启用".to_owned(),
                base_url: self.qwen_config.base_url.clone(),
                model: self.qwen_config.model.clone(),
            };
        }
        let endpoint = match self.qwen_base_url.join("audio/voices") {
            Ok(mut endpoint) => {
                endpoint
                    .query_pairs_mut()
                    .append_pair("model", &self.qwen_config.model);
                endpoint
            }
            Err(error) => {
                return QwenTtsRuntimeStatus {
                    enabled: true,
                    healthy: false,
                    message: format!("Qwen3-TTS 健康检查地址无效: {error}"),
                    base_url: self.qwen_config.base_url.clone(),
                    model: self.qwen_config.model.clone(),
                };
            }
        };
        let mut request = self.qwen_client.get(endpoint);
        if let Some(api_key) = &self.qwen_config.api_key {
            request = request.bearer_auth(api_key);
        }
        let result = request.send().await;
        let (healthy, message) = match result {
            Ok(response) if response.status().is_success() => {
                (true, format!("Qwen3-TTS 可用: {}", self.qwen_config.model))
            }
            Ok(response) => (
                false,
                format!("Qwen3-TTS 健康检查返回 HTTP {}", response.status()),
            ),
            Err(error) => (false, format!("Qwen3-TTS 无法连接: {error}")),
        };
        QwenTtsRuntimeStatus {
            enabled: true,
            healthy,
            message,
            base_url: self.qwen_config.base_url.clone(),
            model: self.qwen_config.model.clone(),
        }
    }

    pub async fn index_runtime_status(&self) -> IndexTtsRuntimeStatus {
        self.index_runtime.status().await
    }

    pub async fn run_index_action(&self, action: &str) -> Result<IndexTtsRuntimeStatus> {
        self.index_runtime.run_action(action).await
    }

    pub async fn start_index_if_installed(&self) {
        let status = self.index_runtime.status().await;
        if status.model_ready && !status.running {
            if let Err(error) = self.index_runtime.run_action("start").await {
                tracing::warn!(%error, "failed to auto-start configured IndexTTS2 service");
            }
        }
    }

    pub fn default_backend_id(&self) -> &'static str {
        self.registry.default_backend_id()
    }

    pub fn voices_for(&self, backend_id: &str) -> Option<Vec<TtsVoiceInfo>> {
        self.registry.voices_for(backend_id)
    }

    pub async fn effective_selection(&self) -> Result<EffectiveTtsSelection> {
        if self.authority.mode() == AuthorityMode::Tenant {
            let authorization = self.authority.snapshot().await;
            let backend_id = authorization
                .tts_backend_id
                .context("授权总线尚未下发租户 TTS 配置")?;
            return Ok(EffectiveTtsSelection {
                backend_id,
                source: authorization
                    .tts_config_source
                    .unwrap_or_else(|| "tenant".to_owned()),
                tenant_id: authorization.tenant_id,
                tenant_name: authorization.tenant_name,
            });
        }

        let backend_id = match &self.database {
            Some(database) => database
                .tts_system_setting()
                .await?
                .map(|setting| setting.backend_id)
                .unwrap_or_else(|| self.default_backend_id().to_owned()),
            None => self.default_backend_id().to_owned(),
        };
        Ok(EffectiveTtsSelection {
            backend_id,
            source: "system".to_owned(),
            tenant_id: None,
            tenant_name: None,
        })
    }

    pub async fn services_for_session(
        &self,
        services: &Arc<AppServices>,
    ) -> Result<(Arc<AppServices>, EffectiveTtsSelection)> {
        let selection = self.effective_selection().await?;
        if selection.backend_id == INDEX_TTS_ID {
            let status = self.index_runtime.status().await;
            if !status.healthy {
                anyhow::bail!("IndexTTS2 is not ready: {}", status.message);
            }
        }
        if selection.backend_id == QWEN_TTS_ID {
            let status = self.qwen_runtime_status().await;
            if !status.healthy {
                anyhow::bail!("Qwen3-TTS is not ready: {}", status.message);
            }
        }
        let selected = self
            .registry
            .services_for(services, &selection.backend_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "TTS backend '{}' is not configured on this instance",
                    selection.backend_id
                )
            })?;
        Ok((selected, selection))
    }
}
