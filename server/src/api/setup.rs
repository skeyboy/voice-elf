use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower_cookies::Cookies;
use url::Url;

use super::{
    ApiError, UserResponse, database, hash_password, issue_session, require_authority,
    validate_email, validate_password, validate_username,
};
use crate::{
    AppState,
    authority::InstanceAuthorization,
    storage::{InitializeSystemOutcome, SystemInstallation},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/setup/status", get(setup_status))
        .route("/setup/initialize", post(initialize))
}

#[derive(Serialize)]
struct SetupStatus {
    initialized: bool,
    database_ready: bool,
    initialization_allowed: bool,
    deployment_mode: String,
    backend: String,
    authorization: InstanceAuthorization,
    email_ready: bool,
    profile: Option<SystemInstallation>,
}

async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    let authorization = state.authority.snapshot().await;
    let backend = state
        .asr
        .effective_selection()
        .await
        .map(|selection| selection.backend_id)
        .unwrap_or_else(|_| state.asr.default_backend_id().to_owned());
    let profile = match &state.database {
        Some(database) => database
            .system_installation()
            .await
            .map_err(ApiError::internal)?,
        None => None,
    };
    Ok(Json(SetupStatus {
        initialized: profile.is_some(),
        database_ready: state.database.is_some(),
        initialization_allowed: state.database.is_some() && authorization.allowed,
        deployment_mode: state.authority.mode().as_str().to_owned(),
        backend,
        authorization,
        email_ready: state.mail.configured(),
        profile,
    }))
}

#[derive(Deserialize)]
struct InitializeInput {
    setup_token: String,
    system_name: String,
    organization_name: String,
    public_url: String,
    admin_username: String,
    admin_email: String,
    admin_password: String,
}

#[derive(Serialize)]
struct InitializeResponse {
    profile: SystemInstallation,
    user: UserResponse,
}

async fn initialize(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<InitializeInput>,
) -> Result<(StatusCode, Json<InitializeResponse>), ApiError> {
    require_authority(&state).await?;
    let database = database(&state)?;
    if database
        .system_installation()
        .await
        .map_err(ApiError::internal)?
        .is_some()
    {
        return Err(ApiError::conflict("系统已经完成初始化"));
    }
    if super::token_hash(input.setup_token.trim()) != state.setup_token_hash.as_ref() {
        return Err(ApiError::forbidden("初始化口令不正确"));
    }

    let system_name = validate_text(&input.system_name, 64, "系统名称")?;
    let organization_name = validate_text(&input.organization_name, 120, "组织名称")?;
    let public_url = validate_public_url(&input.public_url)?;
    let username = validate_username(&input.admin_username)?;
    let email = validate_email(&input.admin_email)?;
    validate_password(&input.admin_password)?;
    let password = input.admin_password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;

    let (user, profile) = match database
        .initialize_system(
            &system_name,
            &organization_name,
            Some(&public_url),
            state.authority.mode().as_str(),
            &username,
            &email,
            &password_hash,
        )
        .await
        .map_err(ApiError::internal)?
    {
        InitializeSystemOutcome::Created(user, profile) => (user, profile),
        InitializeSystemOutcome::AlreadyInitialized => {
            return Err(ApiError::conflict("系统已经完成初始化"));
        }
    };
    issue_session(database, &cookies, user.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(InitializeResponse {
            profile,
            user: user.into(),
        }),
    ))
}

fn validate_text(value: &str, maximum: usize, label: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > maximum {
        return Err(ApiError::bad_request(format!(
            "{label}长度必须为 1 到 {maximum} 个字符"
        )));
    }
    Ok(value.to_owned())
}

fn validate_public_url(value: &str) -> Result<String, ApiError> {
    let value = value.trim().trim_end_matches('/');
    let url = Url::parse(value).map_err(|_| ApiError::bad_request("请输入有效的系统访问地址"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::bad_request(
            "系统访问地址必须是无账号、查询参数和片段的 HTTP(S) 地址",
        ));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_public_system_urls() {
        assert_eq!(
            validate_public_url("https://voice.example.com/").unwrap(),
            "https://voice.example.com"
        );
        assert!(validate_public_url("https://user@voice.example.com").is_err());
        assert!(validate_public_url("file:///tmp/voice").is_err());
    }
}
