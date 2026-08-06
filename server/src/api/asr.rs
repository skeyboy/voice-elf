use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, patch},
};
use serde::{Deserialize, Serialize};
use tower_cookies::Cookies;
use uuid::Uuid;

use super::{ApiError, database, require_admin};
use crate::{
    AppState,
    asr_manager::EffectiveAsrSelection,
    backends::AsrBackendInfo,
    config::AuthorityMode,
    storage::{AsrSystemSetting, AuthorityTenantRecord},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/asr", get(management).patch(update_system))
        .route(
            "/admin/authority/tenants/{tenant_id}/asr",
            patch(update_tenant),
        )
}

#[derive(Serialize)]
struct AsrManagement {
    providers: Vec<AsrBackendInfo>,
    system_setting: AsrSystemSetting,
    effective: EffectiveAsrSelection,
    can_update_system: bool,
    applies_to: &'static str,
}

async fn management(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<AsrManagement>, ApiError> {
    require_admin(&state, &cookies).await?;
    let system_setting = database(&state)?
        .asr_system_setting()
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::unavailable("系统 ASR 默认配置尚未初始化"))?;
    let effective = state
        .asr
        .effective_selection()
        .await
        .map_err(|error| ApiError::unavailable(format!("ASR 配置不可用: {error}")))?;
    Ok(Json(AsrManagement {
        providers: state.asr.providers(),
        system_setting,
        effective,
        can_update_system: state.authority.mode() != AuthorityMode::Tenant,
        applies_to: "new_room_pipelines",
    }))
}

#[derive(Deserialize)]
struct AsrBackendInput {
    backend_id: String,
}

async fn update_system(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<AsrBackendInput>,
) -> Result<Json<AsrManagement>, ApiError> {
    let user = require_admin(&state, &cookies).await?;
    if state.authority.mode() == AuthorityMode::Tenant {
        return Err(ApiError::forbidden("租户实例的 ASR 配置由授权总线管理"));
    }
    let backend_id = validate_backend(&state, &input.backend_id)?;
    database(&state)?
        .update_asr_system_setting(backend_id, user.id)
        .await
        .map_err(ApiError::internal)?;
    management(State(state), cookies).await
}

#[derive(Deserialize)]
struct TenantAsrInput {
    backend_id: Option<String>,
}

async fn update_tenant(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<TenantAsrInput>,
) -> Result<Json<AuthorityTenantRecord>, ApiError> {
    require_admin(&state, &cookies).await?;
    if state.authority.mode() != AuthorityMode::Bus {
        return Err(ApiError::not_found("当前实例未启用授权总线"));
    }
    let backend_id = input
        .backend_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| validate_backend(&state, value))
        .transpose()?;
    let tenant = database(&state)?
        .update_authority_tenant_asr(tenant_id, backend_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("租户不存在"))?;
    Ok(Json(tenant))
}

fn validate_backend<'a>(state: &AppState, backend_id: &'a str) -> Result<&'a str, ApiError> {
    let backend_id = backend_id.trim();
    let provider = state
        .asr
        .providers()
        .into_iter()
        .find(|provider| provider.id == backend_id)
        .ok_or_else(|| ApiError::bad_request("未知的 ASR 后端"))?;
    if !provider.available {
        return Err(ApiError::unavailable(format!(
            "ASR 后端 {} 尚未在当前实例配置",
            provider.name
        )));
    }
    Ok(backend_id)
}
