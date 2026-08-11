use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    authority::AuthorityService,
    backends::{AppServices, AsrBackendInfo, AsrBackendRegistry, FunAsrRuntimeStatus},
    config::AuthorityMode,
    storage::Database,
};

#[derive(Clone, Debug, Serialize)]
pub struct EffectiveAsrSelection {
    pub backend_id: String,
    pub source: String,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
}

#[derive(Clone)]
pub struct AsrManager {
    registry: AsrBackendRegistry,
    database: Option<Database>,
    authority: AuthorityService,
}

impl AsrManager {
    pub fn new(
        registry: AsrBackendRegistry,
        database: Option<Database>,
        authority: AuthorityService,
    ) -> Self {
        Self {
            registry,
            database,
            authority,
        }
    }

    pub fn providers(&self) -> Vec<AsrBackendInfo> {
        self.registry.providers()
    }

    pub async fn fun_asr_runtime_status(&self) -> FunAsrRuntimeStatus {
        self.registry.fun_asr_runtime_status().await
    }

    pub fn provider_available(&self, backend_id: &str) -> bool {
        self.registry.is_available(backend_id)
    }

    pub fn default_backend_id(&self) -> &'static str {
        self.registry.default_backend_id()
    }

    pub async fn effective_selection(&self) -> Result<EffectiveAsrSelection> {
        if self.authority.mode() == AuthorityMode::Tenant {
            let authorization = self.authority.snapshot().await;
            let backend_id = authorization
                .asr_backend_id
                .context("授权总线尚未下发租户 ASR 配置")?;
            return Ok(EffectiveAsrSelection {
                backend_id,
                source: authorization
                    .asr_config_source
                    .unwrap_or_else(|| "tenant".to_owned()),
                tenant_id: authorization.tenant_id,
                tenant_name: authorization.tenant_name,
            });
        }

        let backend_id = match &self.database {
            Some(database) => database
                .asr_system_setting()
                .await?
                .map(|setting| setting.backend_id)
                .unwrap_or_else(|| self.default_backend_id().to_owned()),
            None => self.default_backend_id().to_owned(),
        };
        Ok(EffectiveAsrSelection {
            backend_id,
            source: "system".to_owned(),
            tenant_id: None,
            tenant_name: None,
        })
    }

    pub async fn services_for_session(
        &self,
        services: &Arc<AppServices>,
    ) -> Result<(Arc<AppServices>, EffectiveAsrSelection)> {
        let selection = self.effective_selection().await?;
        let selected = self
            .registry
            .services_for(services, &selection.backend_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "ASR backend '{}' is not configured on this instance",
                    selection.backend_id
                )
            })?;
        Ok((selected, selection))
    }
}
