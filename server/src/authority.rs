use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::{AuthorityConfig, AuthorityMode};

#[derive(Clone, Debug, Serialize)]
pub struct InstanceAuthorization {
    pub mode: String,
    pub allowed: bool,
    pub status: String,
    pub message: String,
    pub tenant_id: Option<uuid::Uuid>,
    pub tenant_name: Option<String>,
    pub instance_id: Option<uuid::Uuid>,
    pub instance_name: Option<String>,
    pub asr_backend_id: Option<String>,
    pub asr_config_source: Option<String>,
    pub license_expires_at: Option<DateTime<Utc>>,
    pub grace_ends_at: Option<DateTime<Utc>>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub next_check_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntitlementGrant {
    pub allowed: bool,
    pub status: String,
    pub message: String,
    pub tenant_id: uuid::Uuid,
    pub tenant_name: String,
    pub instance_id: uuid::Uuid,
    pub instance_name: String,
    pub asr_backend_id: String,
    pub asr_config_source: String,
    pub license_expires_at: DateTime<Utc>,
    pub grace_ends_at: DateTime<Utc>,
    pub lease_expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
}

struct CachedToken {
    value: String,
    refresh_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct AuthorityService {
    config: AuthorityConfig,
    snapshot: Arc<RwLock<InstanceAuthorization>>,
}

impl AuthorityService {
    pub fn new(config: AuthorityConfig) -> Self {
        let mode = config.mode.as_str().to_owned();
        let (allowed, status, message) = match config.mode {
            AuthorityMode::Standalone => (
                true,
                "standalone".to_owned(),
                "当前实例使用独立运行模式".to_owned(),
            ),
            AuthorityMode::Bus => (
                true,
                "authorized".to_owned(),
                "当前实例是授权总线".to_owned(),
            ),
            AuthorityMode::Tenant => (
                false,
                "checking".to_owned(),
                "正在向授权总线验证实例身份".to_owned(),
            ),
        };
        Self {
            config,
            snapshot: Arc::new(RwLock::new(InstanceAuthorization {
                mode,
                allowed,
                status,
                message,
                tenant_id: None,
                tenant_name: None,
                instance_id: None,
                instance_name: None,
                asr_backend_id: None,
                asr_config_source: None,
                license_expires_at: None,
                grace_ends_at: None,
                lease_expires_at: None,
                last_checked_at: None,
                next_check_at: None,
            })),
        }
    }

    pub fn mode(&self) -> AuthorityMode {
        self.config.mode
    }

    pub async fn snapshot(&self) -> InstanceAuthorization {
        self.snapshot.read().await.clone()
    }

    pub async fn allowed(&self) -> bool {
        self.snapshot.read().await.allowed
    }

    pub fn start(&self) {
        if self.config.mode != AuthorityMode::Tenant {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            service.run_checks().await;
        });
    }

    async fn run_checks(self) {
        let client = match Client::builder()
            .timeout(self.config.request_timeout)
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                self.apply_failure(format!("无法初始化授权客户端: {error}"))
                    .await;
                return;
            }
        };
        let mut token: Option<CachedToken> = None;
        loop {
            if let Err(error) = self.check_once(&client, &mut token).await {
                tracing::warn!(error = %error, "tenant authority check failed");
                self.apply_failure(error.to_string()).await;
            }
            tokio::time::sleep(self.config.check_interval).await;
        }
    }

    async fn check_once(
        &self,
        client: &Client,
        cached_token: &mut Option<CachedToken>,
    ) -> anyhow::Result<()> {
        let now = Utc::now();
        if cached_token
            .as_ref()
            .is_none_or(|token| token.refresh_at <= now)
        {
            *cached_token = Some(self.fetch_token(client).await?);
        }
        let token = cached_token.as_ref().expect("token was refreshed");
        let base_url = self
            .config
            .base_url
            .as_deref()
            .expect("validated authority URL");
        let response = client
            .post(format!("{base_url}/api/authority/entitlements/check"))
            .bearer_auth(&token.value)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            *cached_token = None;
            self.apply_rejection().await;
            return Ok(());
        }
        let response = response.error_for_status()?;
        let grant = response.json::<EntitlementGrant>().await?;
        self.apply_grant(grant).await;
        Ok(())
    }

    async fn fetch_token(&self, client: &Client) -> anyhow::Result<CachedToken> {
        let base_url = self
            .config
            .base_url
            .as_deref()
            .expect("validated authority URL");
        let response = client
            .post(format!("{base_url}/api/authority/oauth/token"))
            .form(&[
                ("grant_type", "client_credentials"),
                (
                    "client_id",
                    self.config
                        .client_id
                        .as_deref()
                        .expect("validated client id"),
                ),
                (
                    "client_secret",
                    self.config
                        .client_secret
                        .as_deref()
                        .expect("validated client secret"),
                ),
            ])
            .send()
            .await?
            .error_for_status()?;
        let token = response.json::<TokenResponse>().await?;
        Ok(CachedToken {
            value: token.access_token,
            refresh_at: Utc::now() + chrono::Duration::seconds((token.expires_in - 60).max(30)),
        })
    }

    async fn apply_grant(&self, grant: EntitlementGrant) {
        let checked_at = Utc::now();
        let mut snapshot = self.snapshot.write().await;
        *snapshot = InstanceAuthorization {
            mode: AuthorityMode::Tenant.as_str().to_owned(),
            allowed: grant.allowed,
            status: grant.status,
            message: grant.message,
            tenant_id: Some(grant.tenant_id),
            tenant_name: Some(grant.tenant_name),
            instance_id: Some(grant.instance_id),
            instance_name: Some(grant.instance_name),
            asr_backend_id: Some(grant.asr_backend_id),
            asr_config_source: Some(grant.asr_config_source),
            license_expires_at: Some(grant.license_expires_at),
            grace_ends_at: Some(grant.grace_ends_at),
            lease_expires_at: Some(grant.lease_expires_at),
            last_checked_at: Some(checked_at),
            next_check_at: Some(
                checked_at
                    + chrono::Duration::from_std(self.config.check_interval)
                        .unwrap_or_else(|_| chrono::Duration::minutes(5)),
            ),
        };
    }

    async fn apply_failure(&self, error: String) {
        let now = Utc::now();
        let mut snapshot = self.snapshot.write().await;
        let lease_valid = snapshot
            .lease_expires_at
            .is_some_and(|expires_at| expires_at > now && snapshot.allowed);
        snapshot.allowed = lease_valid;
        snapshot.status = if lease_valid { "warning" } else { "blocked" }.to_owned();
        snapshot.message = if lease_valid {
            "授权总线暂时不可达，当前实例正在使用最近一次离线租约".to_owned()
        } else {
            "无法验证实例授权，离线租约已失效".to_owned()
        };
        snapshot.last_checked_at = Some(now);
        snapshot.next_check_at = Some(
            now + chrono::Duration::from_std(self.config.check_interval)
                .unwrap_or_else(|_| chrono::Duration::minutes(5)),
        );
        tracing::debug!(error = %error, lease_valid, "updated tenant authorization snapshot");
    }

    async fn apply_rejection(&self) {
        let now = Utc::now();
        let mut snapshot = self.snapshot.write().await;
        snapshot.allowed = false;
        snapshot.status = "blocked".to_owned();
        snapshot.message = "授权总线已拒绝当前实例凭据，请检查实例状态或更新密钥".to_owned();
        snapshot.lease_expires_at = Some(now);
        snapshot.last_checked_at = Some(now);
        snapshot.next_check_at = Some(
            now + chrono::Duration::from_std(self.config.check_interval)
                .unwrap_or_else(|_| chrono::Duration::minutes(5)),
        );
    }
}
