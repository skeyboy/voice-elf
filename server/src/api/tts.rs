use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};
use tower_cookies::Cookies;
use uuid::Uuid;

use super::{ApiError, authenticate, database, require_admin};
use crate::{
    AppState,
    backends::{TtsBackendInfo, TtsVoiceInfo},
    config::AuthorityMode,
    index_tts_runtime::IndexTtsRuntimeStatus,
    storage::{AuthorityTenantRecord, TtsSystemSetting},
    tts_manager::EffectiveTtsSelection,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/tts", get(management).patch(update_system))
        .route("/tts/voices", get(voice_catalog))
        .route("/admin/tts/index-tts/{action}", post(index_runtime_action))
        .route("/admin/tts/voices/{voice_id}", patch(update_voice_alias))
        .route(
            "/admin/authority/tenants/{tenant_id}/tts",
            patch(update_tenant),
        )
}

#[derive(Serialize)]
struct TtsManagement {
    providers: Vec<TtsBackendInfo>,
    system_setting: TtsSystemSetting,
    effective: EffectiveTtsSelection,
    can_update_system: bool,
    applies_to: &'static str,
    voices: Vec<ManagedTtsVoice>,
    index_tts_runtime: IndexTtsRuntimeStatus,
}

#[derive(Clone, Serialize)]
struct ManagedTtsVoice {
    id: String,
    default_name: String,
    display_name: String,
    alias: Option<String>,
    group: String,
    description: String,
    languages: Vec<String>,
}

#[derive(Serialize)]
struct TtsVoiceCatalog {
    provider: TtsBackendInfo,
    voices: Vec<ManagedTtsVoice>,
    supports_custom_voices: bool,
}

async fn management(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<TtsManagement>, ApiError> {
    require_admin(&state, &cookies).await?;
    let system_setting = database(&state)?
        .tts_system_setting()
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::unavailable("系统 TTS 默认配置尚未初始化"))?;
    let index_tts_runtime = state.tts.index_runtime_status().await;
    let (effective, _, voices) = load_voice_catalog(&state).await?;
    Ok(Json(TtsManagement {
        providers: state.tts.providers_with_status(&index_tts_runtime),
        system_setting,
        effective,
        can_update_system: state.authority.mode() != AuthorityMode::Tenant,
        applies_to: "new_room_pipelines",
        voices,
        index_tts_runtime,
    }))
}

async fn voice_catalog(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<TtsVoiceCatalog>, ApiError> {
    authenticate(&state, &cookies).await?;
    let (_, provider, voices) = load_voice_catalog(&state).await?;
    Ok(Json(TtsVoiceCatalog {
        supports_custom_voices: provider.voice_clone,
        provider,
        voices,
    }))
}

async fn load_voice_catalog(
    state: &AppState,
) -> Result<(EffectiveTtsSelection, TtsBackendInfo, Vec<ManagedTtsVoice>), ApiError> {
    let effective = state
        .tts
        .effective_selection()
        .await
        .map_err(|error| ApiError::unavailable(format!("TTS 配置不可用: {error}")))?;
    let provider = state
        .tts
        .providers()
        .await
        .into_iter()
        .find(|provider| provider.id == effective.backend_id)
        .ok_or_else(|| ApiError::unavailable("当前 TTS Provider 不存在"))?;
    let voices = state
        .tts
        .voices_for(&effective.backend_id)
        .ok_or_else(|| ApiError::unavailable("当前 TTS Provider 未提供音色目录"))?;
    let aliases = database(state)?
        .list_tts_voice_aliases(&effective.backend_id)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|record| (record.voice_id, record.alias))
        .collect::<HashMap<_, _>>();
    let voices = voices
        .into_iter()
        .map(|voice| {
            let alias = aliases.get(&voice.id).cloned();
            managed_voice(voice, alias)
        })
        .collect();
    Ok((effective, provider, voices))
}

fn managed_voice(voice: TtsVoiceInfo, alias: Option<String>) -> ManagedTtsVoice {
    ManagedTtsVoice {
        display_name: alias.clone().unwrap_or_else(|| voice.name.clone()),
        id: voice.id,
        default_name: voice.name,
        alias,
        group: voice.group,
        description: voice.description,
        languages: voice.languages,
    }
}

#[derive(Deserialize)]
struct TtsBackendInput {
    backend_id: String,
}

async fn update_system(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<TtsBackendInput>,
) -> Result<Json<TtsManagement>, ApiError> {
    let user = require_admin(&state, &cookies).await?;
    if state.authority.mode() == AuthorityMode::Tenant {
        return Err(ApiError::forbidden("租户实例的 TTS 配置由授权总线管理"));
    }
    let backend_id = validate_backend(&state, &input.backend_id).await?;
    database(&state)?
        .update_tts_system_setting(backend_id, user.id)
        .await
        .map_err(ApiError::internal)?;
    management(State(state), cookies).await
}

#[derive(Deserialize)]
struct VoiceAliasInput {
    alias: Option<String>,
}

async fn update_voice_alias(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(voice_id): Path<String>,
    Json(input): Json<VoiceAliasInput>,
) -> Result<Json<TtsManagement>, ApiError> {
    let user = require_admin(&state, &cookies).await?;
    let effective = state
        .tts
        .effective_selection()
        .await
        .map_err(|error| ApiError::unavailable(format!("TTS 配置不可用: {error}")))?;
    let canonical_voice_id = state
        .tts
        .voices_for(&effective.backend_id)
        .and_then(|voices| {
            voices
                .into_iter()
                .find(|voice| voice.id.eq_ignore_ascii_case(&voice_id))
                .map(|voice| voice.id)
        })
        .ok_or_else(|| ApiError::not_found("当前 TTS Provider 不支持该音色"))?;
    let alias = normalize_voice_alias(input.alias.as_deref())?;
    database(&state)?
        .update_tts_voice_alias(&effective.backend_id, &canonical_voice_id, alias, user.id)
        .await
        .map_err(ApiError::internal)?;
    management(State(state), cookies).await
}

fn normalize_voice_alias(value: Option<&str>) -> Result<Option<&str>, ApiError> {
    let alias = value.map(str::trim).filter(|value| !value.is_empty());
    if alias.is_some_and(|value| value.chars().count() > 64 || value.chars().any(char::is_control))
    {
        return Err(ApiError::bad_request("音色别名长度须为 1 到 64 个字符"));
    }
    Ok(alias)
}

#[derive(Deserialize)]
struct TenantTtsInput {
    backend_id: Option<String>,
}

async fn update_tenant(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<TenantTtsInput>,
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
        .map(str::to_owned);
    let backend_id = match backend_id {
        Some(value) => Some(validate_backend(&state, &value).await?.to_owned()),
        None => None,
    };
    let tenant = database(&state)?
        .update_authority_tenant_tts(tenant_id, backend_id.as_deref())
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("租户不存在"))?;
    Ok(Json(tenant))
}

async fn validate_backend<'a>(state: &AppState, backend_id: &'a str) -> Result<&'a str, ApiError> {
    let backend_id = backend_id.trim();
    let provider = state
        .tts
        .providers()
        .await
        .into_iter()
        .find(|provider| provider.id == backend_id)
        .ok_or_else(|| ApiError::bad_request("未知的 TTS 后端"))?;
    if !provider.available {
        return Err(ApiError::unavailable(format!(
            "TTS 后端 {} 尚未在当前实例配置",
            provider.name
        )));
    }
    Ok(backend_id)
}

async fn index_runtime_action(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(action): Path<String>,
) -> Result<Json<IndexTtsRuntimeStatus>, ApiError> {
    require_admin(&state, &cookies).await?;
    if !matches!(action.as_str(), "install" | "start" | "stop") {
        return Err(ApiError::bad_request("未知的 IndexTTS2 管理操作"));
    }
    let status = state
        .tts
        .run_index_action(&action)
        .await
        .map_err(|error| ApiError::unavailable(format!("IndexTTS2 操作失败: {error}")))?;
    Ok(Json(status))
}

#[cfg(test)]
mod tests {
    use super::normalize_voice_alias;

    #[test]
    fn validates_and_normalizes_voice_aliases() {
        assert_eq!(
            normalize_voice_alias(Some("  主播声线  ")).unwrap(),
            Some("主播声线")
        );
        assert_eq!(normalize_voice_alias(Some("   ")).unwrap(), None);
        assert!(normalize_voice_alias(Some("bad\nname")).is_err());
        assert!(normalize_voice_alias(Some(&"a".repeat(65))).is_err());
    }
}
