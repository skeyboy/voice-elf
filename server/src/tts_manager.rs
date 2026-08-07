use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    authority::AuthorityService,
    backends::{AppServices, INDEX_TTS_ID, TtsBackendInfo, TtsBackendRegistry, TtsVoiceInfo},
    config::{AuthorityMode, IndexTtsConfig},
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

#[derive(Clone)]
pub struct TtsManager {
    registry: TtsBackendRegistry,
    database: Option<Database>,
    authority: AuthorityService,
    index_runtime: IndexTtsRuntime,
}

impl TtsManager {
    pub fn new(
        registry: TtsBackendRegistry,
        database: Option<Database>,
        authority: AuthorityService,
        index_config: IndexTtsConfig,
    ) -> Result<Self> {
        Ok(Self {
            registry,
            database,
            authority,
            index_runtime: IndexTtsRuntime::new(index_config)?,
        })
    }

    pub async fn providers(&self) -> Vec<TtsBackendInfo> {
        let status = self.index_runtime.status().await;
        self.providers_with_status(&status)
    }

    pub fn providers_with_status(&self, status: &IndexTtsRuntimeStatus) -> Vec<TtsBackendInfo> {
        let mut providers = self.registry.providers();
        if let Some(provider) = providers
            .iter_mut()
            .find(|provider| provider.id == INDEX_TTS_ID)
        {
            provider.available = status.healthy;
        }
        providers
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
