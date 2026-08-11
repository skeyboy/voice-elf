use axum::{
    Form, Json, Router,
    extract::{Path, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{AUTHORIZATION, CACHE_CONTROL, PRAGMA},
    },
    routing::{get, patch, post},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ApiError, database, hash_password, require_admin, token_hash, validate_enum,
    validate_optional_enum, validate_order, validate_sort, verify_password,
};
use crate::{
    AppState,
    authority::{EntitlementGrant, InstanceAuthorization},
    config::AuthorityMode,
    storage::{
        AuthorityInstanceRecord, AuthorityInstanceSummary, AuthorityTenantRecord,
        AuthorityTenantSummary, AuthorityTokenContext, Paginated,
    },
};

const ACCESS_TOKEN_MINUTES: i64 = 10;

pub(super) fn public_router() -> Router<AppState> {
    Router::new().route("/instance/authorization", get(instance_authorization))
}

pub(super) fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/authority/oauth/token", post(issue_access_token))
        .route("/authority/entitlements/check", post(check_entitlement))
        .route(
            "/admin/authority/tenants",
            get(list_tenants).post(create_tenant),
        )
        .route("/admin/authority/tenants/{tenant_id}", patch(update_tenant))
        .route(
            "/admin/authority/tenants/{tenant_id}/instances",
            get(list_instances).post(create_instance),
        )
        .route(
            "/admin/authority/instances/{instance_id}",
            patch(update_instance),
        )
        .route(
            "/admin/authority/instances/{instance_id}/rotate-secret",
            post(rotate_instance_secret),
        )
}

async fn instance_authorization(State(state): State<AppState>) -> Json<InstanceAuthorization> {
    Json(state.authority.snapshot().await)
}

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    client_id: String,
    client_secret: String,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: i64,
}

async fn issue_access_token(
    State(state): State<AppState>,
    Form(input): Form<TokenRequest>,
) -> Result<(HeaderMap, Json<TokenResponse>), ApiError> {
    require_bus(&state)?;
    if input.grant_type != "client_credentials" {
        return Err(ApiError::bad_request("仅支持 client_credentials 授权类型"));
    }
    let database = database(&state)?;
    let Some(instance) = database
        .get_authority_instance_by_client_id(input.client_id.trim())
        .await
        .map_err(ApiError::internal)?
    else {
        return Err(ApiError::unauthorized());
    };
    if instance.status != "active" {
        return Err(ApiError::unauthorized());
    }
    let secret_hash = instance.secret_hash.clone();
    let secret = input.client_secret;
    let valid = tokio::task::spawn_blocking(move || verify_password(&secret, &secret_hash))
        .await
        .map_err(ApiError::internal)?;
    if !valid {
        return Err(ApiError::unauthorized());
    }
    let token = format!(
        "veat_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    let expires_at = Utc::now() + Duration::minutes(ACCESS_TOKEN_MINUTES);
    database
        .create_authority_access_token(instance.id, &token_hash(&token), expires_at)
        .await
        .map_err(ApiError::internal)?;
    Ok((
        sensitive_response_headers(),
        Json(TokenResponse {
            access_token: token,
            token_type: "Bearer",
            expires_in: ACCESS_TOKEN_MINUTES * 60,
        }),
    ))
}

async fn check_entitlement(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<EntitlementGrant>, ApiError> {
    require_bus(&state)?;
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or_else(ApiError::unauthorized)?;
    let database = database(&state)?;
    let context: AuthorityTokenContext = database
        .authority_context_by_token_hash(&token_hash(bearer))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::unauthorized)?;
    let system_backend_id = database
        .asr_system_setting()
        .await
        .map_err(ApiError::internal)?
        .map(|setting| setting.backend_id)
        .ok_or_else(|| ApiError::unavailable("系统 ASR 默认配置尚未初始化"))?;
    let (asr_backend_id, asr_config_source) = match &context.tenant.asr_backend_id {
        Some(backend_id) => (backend_id.as_str(), "tenant"),
        None => (system_backend_id.as_str(), "system"),
    };
    let system_tts_backend_id = database
        .tts_system_setting()
        .await
        .map_err(ApiError::internal)?
        .map(|setting| setting.backend_id)
        .ok_or_else(|| ApiError::unavailable("系统 TTS 默认配置尚未初始化"))?;
    let (tts_backend_id, tts_config_source) = match &context.tenant.tts_backend_id {
        Some(backend_id) => (backend_id.as_str(), "tenant"),
        None => (system_tts_backend_id.as_str(), "system"),
    };
    let grant = evaluate_entitlement(
        &context.tenant,
        &context.instance,
        Utc::now(),
        asr_backend_id,
        asr_config_source,
        tts_backend_id,
        tts_config_source,
    );
    database
        .record_authority_check(
            context.tenant.id,
            context.instance.id,
            grant.allowed,
            &grant.status,
        )
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(grant))
}

#[derive(Deserialize)]
struct TenantListQuery {
    q: Option<String>,
    status: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
}

#[derive(Deserialize)]
struct TenantInput {
    name: String,
    slug: String,
    status: String,
    license_expires_at: DateTime<Utc>,
    grace_ends_at: DateTime<Utc>,
    warning_days: i32,
    offline_lease_minutes: i32,
}

#[derive(Deserialize)]
struct TenantUpdateInput {
    name: String,
    status: String,
    license_expires_at: DateTime<Utc>,
    grace_ends_at: DateTime<Utc>,
    warning_days: i32,
    offline_lease_minutes: i32,
}

async fn list_tenants(
    State(state): State<AppState>,
    cookies: tower_cookies::Cookies,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Paginated<AuthorityTenantSummary>>, ApiError> {
    require_bus_admin(&state, &cookies).await?;
    let status = validate_optional_enum(
        query.status.as_deref(),
        &["active", "suspended", "revoked"],
        "租户状态",
    )?;
    let sort = validate_sort(
        query.sort.as_deref(),
        &["created_at", "name", "license_expires_at"],
        "created_at",
    )?;
    let descending = validate_order(query.order.as_deref())?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(10, 50);
    Ok(Json(
        database(&state)?
            .list_authority_tenants(
                query.q.as_deref(),
                status,
                sort,
                descending,
                page,
                page_size,
            )
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn create_tenant(
    State(state): State<AppState>,
    cookies: tower_cookies::Cookies,
    Json(input): Json<TenantInput>,
) -> Result<(StatusCode, Json<AuthorityTenantRecord>), ApiError> {
    require_bus_admin(&state, &cookies).await?;
    let name = validate_name(&input.name, "租户名称")?;
    let slug = validate_slug(&input.slug)?;
    validate_tenant_settings(
        &input.status,
        input.license_expires_at,
        input.grace_ends_at,
        input.warning_days,
        input.offline_lease_minutes,
    )?;
    let tenant = database(&state)?
        .create_authority_tenant(
            &name,
            &slug,
            &input.status,
            input.license_expires_at,
            input.grace_ends_at,
            input.warning_days,
            input.offline_lease_minutes,
        )
        .await
        .map_err(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(tenant)))
}

async fn update_tenant(
    State(state): State<AppState>,
    cookies: tower_cookies::Cookies,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<TenantUpdateInput>,
) -> Result<Json<AuthorityTenantRecord>, ApiError> {
    require_bus_admin(&state, &cookies).await?;
    let name = validate_name(&input.name, "租户名称")?;
    validate_tenant_settings(
        &input.status,
        input.license_expires_at,
        input.grace_ends_at,
        input.warning_days,
        input.offline_lease_minutes,
    )?;
    let tenant = database(&state)?
        .update_authority_tenant(
            tenant_id,
            &name,
            &input.status,
            input.license_expires_at,
            input.grace_ends_at,
            input.warning_days,
            input.offline_lease_minutes,
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("租户不存在"))?;
    Ok(Json(tenant))
}

#[derive(Deserialize)]
struct InstanceInput {
    name: String,
}

#[derive(Deserialize)]
struct InstanceStatusInput {
    status: String,
}

#[derive(Serialize)]
struct IssuedCredential {
    instance: AuthorityInstanceSummary,
    client_secret: String,
}

async fn list_instances(
    State(state): State<AppState>,
    cookies: tower_cookies::Cookies,
    Path(tenant_id): Path<Uuid>,
) -> Result<Json<Vec<AuthorityInstanceSummary>>, ApiError> {
    require_bus_admin(&state, &cookies).await?;
    if database(&state)?
        .get_authority_tenant(tenant_id)
        .await
        .map_err(ApiError::internal)?
        .is_none()
    {
        return Err(ApiError::not_found("租户不存在"));
    }
    Ok(Json(
        database(&state)?
            .list_authority_instances(tenant_id)
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn create_instance(
    State(state): State<AppState>,
    cookies: tower_cookies::Cookies,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<InstanceInput>,
) -> Result<(StatusCode, HeaderMap, Json<IssuedCredential>), ApiError> {
    require_bus_admin(&state, &cookies).await?;
    if database(&state)?
        .get_authority_tenant(tenant_id)
        .await
        .map_err(ApiError::internal)?
        .is_none()
    {
        return Err(ApiError::not_found("租户不存在"));
    }
    let name = validate_name(&input.name, "实例名称")?;
    let (client_id, client_secret, secret_hash) = new_credential().await?;
    let instance = database(&state)?
        .create_authority_instance(tenant_id, &name, &client_id, &secret_hash)
        .await
        .map_err(ApiError::internal)?;
    Ok((
        StatusCode::CREATED,
        sensitive_response_headers(),
        Json(IssuedCredential {
            instance: instance.into(),
            client_secret,
        }),
    ))
}

async fn update_instance(
    State(state): State<AppState>,
    cookies: tower_cookies::Cookies,
    Path(instance_id): Path<Uuid>,
    Json(input): Json<InstanceStatusInput>,
) -> Result<Json<AuthorityInstanceSummary>, ApiError> {
    require_bus_admin(&state, &cookies).await?;
    validate_enum(&input.status, &["active", "revoked"], "实例状态")?;
    let instance = database(&state)?
        .update_authority_instance_status(instance_id, &input.status)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("实例不存在"))?;
    Ok(Json(instance.into()))
}

async fn rotate_instance_secret(
    State(state): State<AppState>,
    cookies: tower_cookies::Cookies,
    Path(instance_id): Path<Uuid>,
) -> Result<(HeaderMap, Json<IssuedCredential>), ApiError> {
    require_bus_admin(&state, &cookies).await?;
    let (_, client_secret, secret_hash) = new_credential().await?;
    let instance = database(&state)?
        .rotate_authority_instance_secret(instance_id, &secret_hash)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("实例不存在"))?;
    Ok((
        sensitive_response_headers(),
        Json(IssuedCredential {
            instance: instance.into(),
            client_secret,
        }),
    ))
}

fn sensitive_response_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers
}

async fn new_credential() -> Result<(String, String, String), ApiError> {
    let client_id = format!("vei_{}", Uuid::new_v4().simple());
    let client_secret = format!("ves_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let secret = client_secret.clone();
    let hash = tokio::task::spawn_blocking(move || hash_password(&secret))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    Ok((client_id, client_secret, hash))
}

async fn require_bus_admin(
    state: &AppState,
    cookies: &tower_cookies::Cookies,
) -> Result<(), ApiError> {
    require_bus(state)?;
    require_admin(state, cookies).await?;
    Ok(())
}

fn require_bus(state: &AppState) -> Result<(), ApiError> {
    if state.authority.mode() != AuthorityMode::Bus {
        return Err(ApiError::not_found("当前实例未启用授权总线"));
    }
    Ok(())
}

fn validate_name(value: &str, field: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 120 {
        return Err(ApiError::bad_request(format!(
            "{field}长度必须为 1 到 120 个字符"
        )));
    }
    Ok(value.to_owned())
}

fn validate_slug(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_lowercase();
    if !(3..=48).contains(&value.len())
        || !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        || value.starts_with('-')
        || value.ends_with('-')
    {
        return Err(ApiError::bad_request(
            "租户标识须为 3 到 48 位小写字母、数字或连字符",
        ));
    }
    Ok(value)
}

fn validate_tenant_settings(
    status: &str,
    license_expires_at: DateTime<Utc>,
    grace_ends_at: DateTime<Utc>,
    warning_days: i32,
    offline_lease_minutes: i32,
) -> Result<(), ApiError> {
    validate_enum(status, &["active", "suspended", "revoked"], "租户状态")?;
    if grace_ends_at < license_expires_at {
        return Err(ApiError::bad_request("宽限期结束时间不能早于授权到期时间"));
    }
    if !(1..=180).contains(&warning_days) {
        return Err(ApiError::bad_request("提前提醒天数须为 1 到 180"));
    }
    if !(5..=10_080).contains(&offline_lease_minutes) {
        return Err(ApiError::bad_request("离线租约须为 5 到 10080 分钟"));
    }
    Ok(())
}

fn evaluate_entitlement(
    tenant: &AuthorityTenantRecord,
    instance: &AuthorityInstanceRecord,
    now: DateTime<Utc>,
    asr_backend_id: &str,
    asr_config_source: &str,
    tts_backend_id: &str,
    tts_config_source: &str,
) -> EntitlementGrant {
    let (allowed, status, message) = if instance.status != "active" {
        (false, "blocked", "当前部署实例已被撤销")
    } else if tenant.status == "suspended" {
        (false, "blocked", "租户授权已暂停，请联系平台管理员")
    } else if tenant.status == "revoked" {
        (false, "blocked", "租户授权已撤销")
    } else if now > tenant.grace_ends_at {
        (false, "blocked", "租户授权及宽限期均已到期")
    } else if now > tenant.license_expires_at {
        (true, "grace", "租户授权已到期，当前处于宽限期")
    } else if now + Duration::days(i64::from(tenant.warning_days)) >= tenant.license_expires_at {
        (true, "warning", "租户授权即将到期")
    } else {
        (true, "authorized", "租户实例授权有效")
    };
    let lease_expires_at = if allowed {
        std::cmp::min(
            now + Duration::minutes(i64::from(tenant.offline_lease_minutes)),
            tenant.grace_ends_at,
        )
    } else {
        now
    };
    EntitlementGrant {
        allowed,
        status: status.to_owned(),
        message: message.to_owned(),
        tenant_id: tenant.id,
        tenant_name: tenant.name.clone(),
        instance_id: instance.id,
        instance_name: instance.name.clone(),
        asr_backend_id: asr_backend_id.to_owned(),
        asr_config_source: asr_config_source.to_owned(),
        tts_backend_id: tts_backend_id.to_owned(),
        tts_config_source: tts_config_source.to_owned(),
        license_expires_at: tenant.license_expires_at,
        grace_ends_at: tenant.grace_ends_at,
        lease_expires_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(expires_at: DateTime<Utc>, grace_ends_at: DateTime<Utc>) -> AuthorityTenantRecord {
        AuthorityTenantRecord {
            id: Uuid::new_v4(),
            name: "Test tenant".to_owned(),
            slug: "test-tenant".to_owned(),
            status: "active".to_owned(),
            license_expires_at: expires_at,
            grace_ends_at,
            warning_days: 30,
            offline_lease_minutes: 1_440,
            asr_backend_id: None,
            tts_backend_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn instance(tenant_id: Uuid) -> AuthorityInstanceRecord {
        AuthorityInstanceRecord {
            id: Uuid::new_v4(),
            tenant_id,
            name: "Production".to_owned(),
            client_id: "client".to_owned(),
            secret_hash: "hash".to_owned(),
            status: "active".to_owned(),
            last_seen_at: None,
            last_authorized_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn transitions_from_warning_to_grace_and_blocked() {
        let now = Utc::now();
        let warning_tenant = tenant(now + Duration::days(5), now + Duration::days(12));
        let warning = evaluate_entitlement(
            &warning_tenant,
            &instance(warning_tenant.id),
            now,
            "qwen-local",
            "system",
            "local-fallback",
            "system",
        );
        assert_eq!(warning.status, "warning");
        assert_eq!(warning.asr_backend_id, "qwen-local");
        assert_eq!(warning.asr_config_source, "system");
        let grace_tenant = tenant(now - Duration::days(1), now + Duration::days(2));
        assert_eq!(
            evaluate_entitlement(
                &grace_tenant,
                &instance(grace_tenant.id),
                now,
                "qwen-local",
                "system",
                "local-fallback",
                "system",
            )
            .status,
            "grace"
        );
        let expired_tenant = tenant(now - Duration::days(3), now - Duration::days(1));
        assert!(
            !evaluate_entitlement(
                &expired_tenant,
                &instance(expired_tenant.id),
                now,
                "qwen-local",
                "system",
                "local-fallback",
                "system",
            )
            .allowed
        );
    }
}
