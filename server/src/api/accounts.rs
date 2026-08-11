use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tower_cookies::Cookies;
use url::Url;
use uuid::Uuid;

use crate::{
    AppState,
    config::{MailConfig, SmtpSecurity},
    mailer::MailService,
    storage::{ManagedUserInput, UserRecord},
};

use super::{
    ApiError, UserResponse, database, hash_password, require_admin, require_authority,
    require_initialized, token_hash, validate_email, validate_enum, validate_password,
    validate_username,
};

const MAX_IMPORT_BYTES: usize = 1024 * 1024;
const MAX_IMPORT_USERS: usize = 500;
const PUBLIC_RESET_LIMIT: i64 = 3;

pub(super) fn public_router() -> Router<AppState> {
    Router::new()
        .route("/auth/password/status", get(password_reset_status))
        .route("/auth/password/forgot", post(forgot_password))
        .route("/auth/password/reset", post(reset_password))
}

pub(super) fn admin_router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/email/config",
            get(admin_mail_status).put(admin_update_mail_config),
        )
        .route("/admin/email/status", get(admin_mail_status))
        .route("/admin/users", post(admin_create_user))
        .route(
            "/admin/users/import",
            post(admin_import_users).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES + 64 * 1024)),
        )
        .route(
            "/admin/users/{user_id}/password-reset",
            post(admin_send_password_reset),
        )
}

#[derive(Serialize)]
struct PasswordResetStatus {
    email_enabled: bool,
    reset_expiry_minutes: u64,
}

async fn password_reset_status(State(state): State<AppState>) -> Json<PasswordResetStatus> {
    Json(PasswordResetStatus {
        email_enabled: state.mail.configured(),
        reset_expiry_minutes: state.mail.reset_expiry().as_secs() / 60,
    })
}

#[derive(Deserialize)]
struct ForgotPasswordInput {
    account: String,
}

#[derive(Serialize)]
struct ForgotPasswordResponse {
    message: &'static str,
}

async fn forgot_password(
    State(state): State<AppState>,
    Json(input): Json<ForgotPasswordInput>,
) -> Result<(StatusCode, Json<ForgotPasswordResponse>), ApiError> {
    require_initialized(&state).await?;
    require_authority(&state).await?;
    let account = input.account.trim();
    if account.is_empty() || account.chars().count() > 254 {
        return Err(ApiError::bad_request("请输入账号名称或邮箱地址"));
    }
    if state.mail.configured()
        && let Some(user) = database(&state)?
            .find_user_by_account(account)
            .await
            .map_err(ApiError::internal)?
    {
        tokio::spawn(async move {
            if let Err(error) = issue_password_reset(&state, user, false).await {
                tracing::warn!(error = ?error, "password reset email could not be issued");
            }
        });
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(ForgotPasswordResponse {
            message: "如果账号存在且已配置邮箱，重置链接将发送到对应邮箱",
        }),
    ))
}

#[derive(Deserialize)]
struct ResetPasswordInput {
    token: String,
    password: String,
}

async fn reset_password(
    State(state): State<AppState>,
    Json(input): Json<ResetPasswordInput>,
) -> Result<StatusCode, ApiError> {
    require_initialized(&state).await?;
    require_authority(&state).await?;
    if input.token.len() != 64 || !input.token.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("密码重置链接无效或已过期"));
    }
    validate_password(&input.password)?;
    let password = input.password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    database(&state)?
        .reset_password(&token_hash(&input.token), &password_hash)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::bad_request("密码重置链接无效或已过期"))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_mail_status(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<crate::mailer::MailStatus>, ApiError> {
    require_admin(&state, &cookies).await?;
    Ok(Json(state.mail.status()))
}

#[derive(Deserialize)]
struct AdminMailConfigInput {
    enabled: bool,
    host: String,
    port: u16,
    security: String,
    username: String,
    password: Option<String>,
    #[serde(default)]
    clear_password: bool,
    from_address: String,
    from_name: String,
    public_url: Option<String>,
    reset_expiry_minutes: u64,
}

async fn admin_update_mail_config(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<AdminMailConfigInput>,
) -> Result<Json<crate::mailer::MailStatus>, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    validate_enum(
        &input.security,
        &["wrapper", "starttls", "none"],
        "SMTP 安全模式",
    )?;
    let host = input.host.trim();
    let username = input.username.trim();
    let from_address = validate_email(&input.from_address)?;
    let from_name = input.from_name.trim();
    if host.is_empty() || host.chars().count() > 255 {
        return Err(ApiError::bad_request(
            "SMTP 主机不能为空且不能超过 255 个字符",
        ));
    }
    if input.port == 0 {
        return Err(ApiError::bad_request("SMTP 端口必须在 1 到 65535 之间"));
    }
    if !(5..=1440).contains(&input.reset_expiry_minutes) {
        return Err(ApiError::bad_request(
            "重置链接有效期必须在 5 到 1440 分钟之间",
        ));
    }
    if username.chars().count() > 255 || from_name.is_empty() || from_name.chars().count() > 128 {
        return Err(ApiError::bad_request("SMTP 用户名或发件人名称格式无效"));
    }
    let public_url = input
        .public_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_owned());
    if let Some(public_url) = &public_url {
        let url = Url::parse(public_url).map_err(|_| ApiError::bad_request("系统访问地址无效"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ApiError::bad_request(
                "系统访问地址必须是无凭据的 HTTP(S) 地址",
            ));
        }
    }
    let current = state.mail.config();
    let password = if input.clear_password {
        None
    } else {
        input
            .password
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .or(current.password)
    };
    let config = MailConfig {
        enabled: input.enabled,
        host: host.to_owned(),
        port: input.port,
        security: match input.security.as_str() {
            "starttls" => SmtpSecurity::StartTls,
            "none" => SmtpSecurity::None,
            _ => SmtpSecurity::Wrapper,
        },
        username: username.to_owned(),
        password,
        from_address,
        from_name: from_name.to_owned(),
        public_url,
        reset_expiry: std::time::Duration::from_secs(input.reset_expiry_minutes * 60),
    };
    MailService::new(config.clone()).map_err(|error| ApiError::bad_request(error.to_string()))?;
    database(&state)?
        .save_email_setting(admin.id, &config)
        .await
        .map_err(ApiError::internal)?;
    state.mail.update(config).map_err(ApiError::internal)?;
    Ok(Json(state.mail.status()))
}

#[derive(Deserialize)]
struct AdminCreateUserInput {
    username: String,
    email: String,
    password: String,
    role: String,
    status: String,
}

async fn admin_create_user(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<AdminCreateUserInput>,
) -> Result<(StatusCode, Json<UserResponse>), ApiError> {
    require_admin(&state, &cookies).await?;
    let database = database(&state)?;
    let username = validate_username(&input.username)?;
    let email = validate_email(&input.email)?;
    validate_password(&input.password)?;
    validate_enum(&input.role, &["admin", "member"], "人员角色")?;
    validate_enum(
        &input.status,
        &["pending", "active", "suspended"],
        "人员状态",
    )?;
    ensure_account_available(database, &username, &email).await?;
    let password = input.password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    let mut users = database
        .create_managed_users(&[ManagedUserInput {
            username,
            email,
            password_hash,
            role: input.role,
            status: input.status,
        }])
        .await
        .map_err(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(users.remove(0).into())))
}

#[derive(Deserialize)]
struct CsvUserRow {
    username: String,
    email: String,
    password: String,
    #[serde(default = "default_role")]
    role: String,
    #[serde(default = "default_status")]
    status: String,
}

#[derive(Serialize)]
struct ImportUsersResponse {
    imported: usize,
}

async fn admin_import_users(
    State(state): State<AppState>,
    cookies: Cookies,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<ImportUsersResponse>), ApiError> {
    require_admin(&state, &cookies).await?;
    let mut csv_bytes = None;
    while let Some(field) = multipart.next_field().await.map_err(ApiError::internal)? {
        if field.name() == Some("file") {
            let bytes = field.bytes().await.map_err(ApiError::internal)?;
            if bytes.len() > MAX_IMPORT_BYTES {
                return Err(ApiError::bad_request("CSV 文件不能超过 1 MB"));
            }
            csv_bytes = Some(bytes);
            break;
        }
    }
    let csv_bytes = csv_bytes.ok_or_else(|| ApiError::bad_request("请选择 CSV 文件"))?;
    let csv_bytes = csv_bytes
        .strip_prefix(&[0xef, 0xbb, 0xbf])
        .unwrap_or(&csv_bytes);
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(csv_bytes);
    let mut rows = Vec::new();
    let mut usernames = HashSet::new();
    let mut emails = HashSet::new();
    for (index, result) in reader.deserialize::<CsvUserRow>().enumerate() {
        if rows.len() >= MAX_IMPORT_USERS {
            return Err(ApiError::bad_request("单次最多导入 500 个用户"));
        }
        let line = index + 2;
        let row = result
            .map_err(|error| ApiError::bad_request(format!("CSV 第 {line} 行格式错误：{error}")))?;
        let username = validate_username(&row.username)
            .map_err(|_| ApiError::bad_request(format!("CSV 第 {line} 行账号名称无效")))?;
        let email = validate_email(&row.email)
            .map_err(|_| ApiError::bad_request(format!("CSV 第 {line} 行邮箱地址无效")))?;
        validate_password(&row.password)
            .map_err(|_| ApiError::bad_request(format!("CSV 第 {line} 行密码长度无效")))?;
        let role = if row.role.is_empty() {
            default_role()
        } else {
            row.role
        };
        let status = if row.status.is_empty() {
            default_status()
        } else {
            row.status
        };
        validate_enum(&role, &["admin", "member"], "人员角色")
            .map_err(|_| ApiError::bad_request(format!("CSV 第 {line} 行角色无效")))?;
        validate_enum(&status, &["pending", "active", "suspended"], "人员状态")
            .map_err(|_| ApiError::bad_request(format!("CSV 第 {line} 行状态无效")))?;
        if !usernames.insert(username.clone()) {
            return Err(ApiError::bad_request(format!(
                "CSV 第 {line} 行账号名称重复"
            )));
        }
        if !emails.insert(email.clone()) {
            return Err(ApiError::bad_request(format!("CSV 第 {line} 行邮箱重复")));
        }
        rows.push((username, email, row.password, role, status));
    }
    if rows.is_empty() {
        return Err(ApiError::bad_request("CSV 中没有可导入的用户"));
    }
    let database = database(&state)?;
    for (username, email, _, _, _) in &rows {
        ensure_account_available(database, username, email).await?;
    }
    let inputs = tokio::task::spawn_blocking(move || {
        rows.into_iter()
            .map(|(username, email, password, role, status)| {
                Ok(ManagedUserInput {
                    username,
                    email,
                    password_hash: hash_password(&password)?,
                    role,
                    status,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    let imported = database
        .create_managed_users(&inputs)
        .await
        .map_err(ApiError::internal)?
        .len();
    Ok((StatusCode::CREATED, Json(ImportUsersResponse { imported })))
}

async fn admin_send_password_reset(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_admin(&state, &cookies).await?;
    if !state.mail.configured() {
        return Err(ApiError::unavailable(
            "SMTP 尚未配置，请先在管理端完成邮箱配置",
        ));
    }
    let user = database(&state)?
        .get_user(user_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("人员不存在"))?;
    if user.email.is_none() {
        return Err(ApiError::bad_request("请先为该人员补录邮箱地址"));
    }
    issue_password_reset(&state, user, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_account_available(
    database: &crate::storage::Database,
    username: &str,
    email: &str,
) -> Result<(), ApiError> {
    if database
        .find_user_by_username(username)
        .await
        .map_err(ApiError::internal)?
        .is_some()
    {
        return Err(ApiError::conflict(format!("账号名称 {username} 已存在")));
    }
    if database
        .find_user_by_email(email)
        .await
        .map_err(ApiError::internal)?
        .is_some()
    {
        return Err(ApiError::conflict(format!("邮箱地址 {email} 已被使用")));
    }
    Ok(())
}

async fn issue_password_reset(
    state: &AppState,
    user: UserRecord,
    force: bool,
) -> Result<(), ApiError> {
    let email = user
        .email
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("账号未配置邮箱地址"))?;
    let database = database(state)?;
    if !force {
        let count = database
            .password_reset_request_count(user.id, Utc::now() - Duration::hours(1))
            .await
            .map_err(ApiError::internal)?;
        if count >= PUBLIC_RESET_LIMIT {
            return Ok(());
        }
    }
    let installation = database
        .system_installation()
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::unavailable("系统尚未完成初始化"))?;
    let public_url = state
        .mail
        .public_url()
        .or_else(|| installation.public_url.clone())
        .ok_or_else(|| ApiError::unavailable("系统访问地址尚未配置"))?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at =
        Utc::now() + Duration::from_std(state.mail.reset_expiry()).map_err(ApiError::internal)?;
    database
        .create_password_reset_token(user.id, &token_hash(&token), expires_at)
        .await
        .map_err(ApiError::internal)?;
    let mut reset_url =
        Url::parse(&public_url).map_err(|_| ApiError::unavailable("系统访问地址配置无效"))?;
    reset_url.set_path("/reset-password");
    reset_url.set_query(None);
    reset_url.query_pairs_mut().append_pair("token", &token);
    state
        .mail
        .send_password_reset(
            email,
            &user.username,
            &installation.system_name,
            reset_url.as_str(),
        )
        .await
        .map_err(ApiError::internal)
}

fn default_role() -> String {
    "member".to_owned()
}

fn default_status() -> String {
    "active".to_owned()
}
