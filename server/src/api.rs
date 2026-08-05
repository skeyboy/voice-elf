use std::io::Cursor;

use anyhow::anyhow;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{
        StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower_cookies::{Cookie, Cookies, cookie::SameSite};
use uuid::Uuid;

use crate::{
    AppState,
    protocol::RoomMemberState,
    storage::{Database, RoomSummary, UserRecord, UtteranceHistory},
};

pub const AUTH_COOKIE: &str = "voice_elf_session";
const SESSION_DAYS: i64 = 7;
const MAX_VOICE_REFERENCES: usize = 5;
const MAX_VOICE_REFERENCE_BYTES: usize = 5 * 1024 * 1024;
const MIN_VOICE_REFERENCE_MS: i64 = 3_000;
const MAX_VOICE_REFERENCE_MS: i64 = 15_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", delete(logout))
        .route("/auth/me", get(me))
        .route(
            "/voice-references",
            get(list_voice_references)
                .post(create_voice_reference)
                .layer(DefaultBodyLimit::max(MAX_VOICE_REFERENCE_BYTES + 64 * 1024)),
        )
        .route(
            "/voice-references/{voice_id}",
            delete(delete_voice_reference),
        )
        .route(
            "/voice-references/{voice_id}/audio",
            get(voice_reference_audio),
        )
        .route("/rooms", get(list_rooms).post(create_room))
        .route(
            "/rooms/{room_id}",
            get(room_detail).patch(update_room).delete(delete_room),
        )
        .route("/rooms/{room_id}/join", post(join_room))
        .route(
            "/rooms/{room_id}/members/{user_id}",
            patch(update_room_member),
        )
}

#[derive(Serialize)]
struct VoiceReferenceResponse {
    id: Uuid,
    name: String,
    duration_ms: i64,
    created_at: chrono::DateTime<Utc>,
    audio_url: String,
}

impl From<crate::storage::VoiceReferenceRecord> for VoiceReferenceResponse {
    fn from(reference: crate::storage::VoiceReferenceRecord) -> Self {
        let id = reference.id;
        Self {
            id,
            name: reference.name,
            duration_ms: reference.duration_ms,
            created_at: reference.created_at,
            audio_url: format!("/api/voice-references/{id}/audio"),
        }
    }
}

async fn list_voice_references(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<Vec<VoiceReferenceResponse>>, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let references = database(&state)?
        .list_voice_references(user.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(references.into_iter().map(Into::into).collect()))
}

async fn create_voice_reference(
    State(state): State<AppState>,
    cookies: Cookies,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<VoiceReferenceResponse>), ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let mut name = None;
    let mut audio = None;
    while let Some(field) = multipart.next_field().await.map_err(ApiError::internal)? {
        match field.name() {
            Some("name") => name = Some(field.text().await.map_err(ApiError::internal)?),
            Some("audio") => {
                let bytes = field.bytes().await.map_err(ApiError::internal)?;
                if bytes.len() > MAX_VOICE_REFERENCE_BYTES {
                    return Err(ApiError::bad_request("参考音频不能超过 5 MB"));
                }
                audio = Some(bytes);
            }
            _ => {}
        }
    }
    let name = validate_voice_reference_name(name.as_deref().unwrap_or_default())?;
    let audio = audio.ok_or_else(|| ApiError::bad_request("请选择参考音频"))?;
    let duration_ms = validate_voice_reference_wav(&audio)?;
    let database = database(&state)?;
    let existing = database
        .list_voice_references(user.id)
        .await
        .map_err(ApiError::internal)?;
    if existing.len() >= MAX_VOICE_REFERENCES {
        return Err(ApiError::conflict("每个账号最多保存 5 个自定义音色"));
    }
    if existing.iter().any(|reference| reference.name == name) {
        return Err(ApiError::conflict("已有同名的自定义音色"));
    }

    let id = Uuid::new_v4();
    let audio_path = state
        .media
        .save_voice_reference(user.id, id, &audio)
        .await
        .map_err(ApiError::internal)?;
    let reference = match database
        .create_voice_reference(id, user.id, &name, &audio_path, duration_ms)
        .await
    {
        Ok(reference) => reference,
        Err(error) => {
            let _ = state.media.delete_voice_reference(&audio_path).await;
            return Err(ApiError::internal(error));
        }
    };
    Ok((StatusCode::CREATED, Json(reference.into())))
}

async fn voice_reference_audio(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(voice_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let reference = database(&state)?
        .get_voice_reference(voice_id, user.id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("自定义音色不存在"))?;
    let bytes = tokio::fs::read(&reference.audio_path)
        .await
        .map_err(ApiError::internal)?;
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, "audio/wav".parse().unwrap());
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "private, no-store".parse().unwrap());
    Ok(response)
}

async fn delete_voice_reference(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(voice_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let reference = database(&state)?
        .delete_voice_reference(voice_id, user.id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("自定义音色不存在"))?;
    state
        .media
        .delete_voice_reference(&reference.audio_path)
        .await
        .map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_voice_reference_name(value: &str) -> Result<String, ApiError> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 32 {
        return Err(ApiError::bad_request("音色名称长度必须为 1 到 32 个字符"));
    }
    Ok(name.to_owned())
}

fn validate_voice_reference_wav(bytes: &[u8]) -> Result<i64, ApiError> {
    if bytes.len() > MAX_VOICE_REFERENCE_BYTES {
        return Err(ApiError::bad_request("参考音频不能超过 5 MB"));
    }
    let mut reader = hound::WavReader::new(Cursor::new(bytes))
        .map_err(|_| ApiError::bad_request("参考音频必须为有效的 WAV 文件"))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int
        || spec.bits_per_sample != 16
        || spec.channels != 1
        || !(16_000..=48_000).contains(&spec.sample_rate)
    {
        return Err(ApiError::bad_request(
            "参考音频必须为 16-48 kHz 的单声道 PCM16 WAV",
        ));
    }
    let duration_ms = i64::from(reader.duration()) * 1_000 / i64::from(spec.sample_rate);
    if !(MIN_VOICE_REFERENCE_MS..=MAX_VOICE_REFERENCE_MS).contains(&duration_ms) {
        return Err(ApiError::bad_request("参考音频时长必须为 3 到 15 秒"));
    }
    let mut squared_total = 0.0_f64;
    let mut peak = 0_i32;
    let mut sample_count = 0_u64;
    for sample in reader.samples::<i16>() {
        let sample = sample.map_err(|_| ApiError::bad_request("参考音频数据不完整"))?;
        let value = i32::from(sample);
        peak = peak.max(value.abs());
        squared_total += f64::from(value) * f64::from(value);
        sample_count += 1;
    }
    let rms = (squared_total / sample_count.max(1) as f64).sqrt() / f64::from(i16::MAX);
    if peak < 655 || rms < 0.005 {
        return Err(ApiError::bad_request("参考音频音量过低，请重新录制"));
    }
    Ok(duration_ms)
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "请先登录")
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "API request failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "服务器处理请求失败")
    }

    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

#[derive(Clone, Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub username: String,
    pub created_at: chrono::DateTime<Utc>,
}

impl From<UserRecord> for UserResponse {
    fn from(user: UserRecord) -> Self {
        Self {
            id: user.id,
            username: user.username,
            created_at: user.created_at,
        }
    }
}

async fn register(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(credentials): Json<Credentials>,
) -> Result<(StatusCode, Json<UserResponse>), ApiError> {
    let database = database(&state)?;
    let username = validate_username(&credentials.username)?;
    validate_password(&credentials.password)?;
    if database
        .find_user_by_username(&username)
        .await
        .map_err(ApiError::internal)?
        .is_some()
    {
        return Err(ApiError::conflict("账号名称已存在"));
    }
    let password = credentials.password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    let user = database
        .create_user(&username, &password_hash)
        .await
        .map_err(ApiError::internal)?;
    issue_session(database, &cookies, user.id).await?;
    Ok((StatusCode::CREATED, Json(user.into())))
}

async fn login(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(credentials): Json<Credentials>,
) -> Result<Json<UserResponse>, ApiError> {
    let database = database(&state)?;
    let username = credentials.username.trim();
    let Some(user) = database
        .find_user_by_username(username)
        .await
        .map_err(ApiError::internal)?
    else {
        return Err(ApiError::unauthorized());
    };
    let password_hash = user.password_hash.clone();
    let password = credentials.password;
    let valid = tokio::task::spawn_blocking(move || verify_password(&password, &password_hash))
        .await
        .map_err(ApiError::internal)?;
    if !valid {
        return Err(ApiError::unauthorized());
    }
    issue_session(database, &cookies, user.id).await?;
    Ok(Json(user.into()))
}

async fn logout(State(state): State<AppState>, cookies: Cookies) -> Result<StatusCode, ApiError> {
    if let Some(token) = cookies
        .get(AUTH_COOKIE)
        .map(|cookie| cookie.value().to_owned())
        && let Some(database) = &state.database
    {
        database
            .delete_auth_session(&token_hash(&token))
            .await
            .map_err(ApiError::internal)?;
    }
    let mut cookie = Cookie::new(AUTH_COOKIE, "");
    cookie.set_path("/");
    cookies.remove(cookie);
    Ok(StatusCode::NO_CONTENT)
}

async fn me(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<UserResponse>, ApiError> {
    Ok(Json(authenticate(&state, &cookies).await?.into()))
}

#[derive(Deserialize)]
struct RoomInput {
    name: String,
    #[serde(default = "default_source_language")]
    source_language: String,
    #[serde(default = "default_target_language")]
    target_language: String,
    #[serde(default = "default_max_utterance_seconds")]
    max_utterance_seconds: u32,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

#[derive(Serialize)]
struct RoomDetailResponse {
    room: RoomSummary,
    members: Vec<RoomMemberState>,
    utterances: Vec<UtteranceHistory>,
}

#[derive(Deserialize)]
struct RoomMemberInput {
    is_muted: bool,
}

async fn list_rooms(
    State(state): State<AppState>,
    cookies: Cookies,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<RoomSummary>>, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let rooms = database(&state)?
        .list_rooms(user.id, query.q.as_deref())
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(rooms))
}

async fn create_room(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<RoomInput>,
) -> Result<(StatusCode, Json<RoomSummary>), ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let (name, source_language, target_language, max_utterance_seconds) = validate_room(input)?;
    let database = database(&state)?;
    let room = database
        .create_room(
            user.id,
            &name,
            &source_language,
            &target_language,
            max_utterance_seconds as i32,
        )
        .await
        .map_err(ApiError::internal)?;
    let summary = database
        .room_summary(&room, user.id)
        .await
        .map_err(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn join_room(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(room_id): Path<Uuid>,
) -> Result<Json<RoomSummary>, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let database = database(&state)?;
    let room = database
        .get_room(room_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("房间不存在"))?;
    database
        .join_room(room_id, user.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(
        database
            .room_summary(&room, user.id)
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn room_detail(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(room_id): Path<Uuid>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<RoomDetailResponse>, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let database = database(&state)?;
    let room = database
        .get_room(room_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("房间不存在"))?;
    if !database
        .can_view_room(room_id, user.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::forbidden("请先加入房间"));
    }
    let summary = database
        .room_summary(&room, user.id)
        .await
        .map_err(ApiError::internal)?;
    let utterances = database
        .list_utterances(room_id, query.q.as_deref())
        .await
        .map_err(ApiError::internal)?;
    let member_records = database
        .list_room_members(room_id)
        .await
        .map_err(ApiError::internal)?;
    let members = state.rooms.member_states(room_id, &member_records);
    Ok(Json(RoomDetailResponse {
        room: summary,
        members,
        utterances,
    }))
}

async fn update_room_member(
    State(state): State<AppState>,
    cookies: Cookies,
    Path((room_id, member_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<RoomMemberInput>,
) -> Result<Json<RoomMemberState>, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let database = database(&state)?;
    let room = database
        .get_room(room_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("房间不存在"))?;
    if room.owner_id != user.id {
        return Err(ApiError::forbidden("只有房主可以管理发言权限"));
    }
    if member_id == room.owner_id {
        return Err(ApiError::bad_request("房主不能禁言自己"));
    }
    database
        .set_room_member_muted(room_id, user.id, member_id, input.is_muted)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("房间成员不存在"))?;
    state
        .rooms
        .set_muted(room_id, member_id, input.is_muted)
        .await;
    let members = database
        .list_room_members(room_id)
        .await
        .map_err(ApiError::internal)?;
    let member = state
        .rooms
        .member_states(room_id, &members)
        .into_iter()
        .find(|member| member.user_id == member_id)
        .ok_or_else(|| ApiError::not_found("房间成员不存在"))?;
    Ok(Json(member))
}

async fn update_room(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(room_id): Path<Uuid>,
    Json(input): Json<RoomInput>,
) -> Result<Json<RoomSummary>, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let (name, source_language, target_language, max_utterance_seconds) = validate_room(input)?;
    let database = database(&state)?;
    let room = database
        .update_room(
            room_id,
            user.id,
            &name,
            &source_language,
            &target_language,
            max_utterance_seconds as i32,
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::forbidden("只有房主可以修改房间"))?;
    Ok(Json(
        database
            .room_summary(&room, user.id)
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn delete_room(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(room_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let database = database(&state)?;
    let media_paths = database
        .room_media_paths(room_id)
        .await
        .map_err(ApiError::internal)?;
    if !database
        .delete_room(room_id, user.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::forbidden("只有房主可以删除房间"));
    }
    for path in media_paths {
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, %path, "failed to delete room audio file");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn authenticate(state: &AppState, cookies: &Cookies) -> Result<UserRecord, ApiError> {
    let token = cookies
        .get(AUTH_COOKIE)
        .map(|cookie| cookie.value().to_owned())
        .ok_or_else(ApiError::unauthorized)?;
    database(state)?
        .user_by_session_hash(&token_hash(&token))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::unauthorized)
}

async fn issue_session(
    database: &Database,
    cookies: &Cookies,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    database
        .create_auth_session(
            user_id,
            &token_hash(&token),
            Utc::now() + Duration::days(SESSION_DAYS),
        )
        .await
        .map_err(ApiError::internal)?;
    let mut cookie = Cookie::new(AUTH_COOKIE, token);
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(tower_cookies::cookie::time::Duration::days(SESSION_DAYS));
    cookies.add(cookie);
    Ok(())
}

fn database(state: &AppState) -> Result<&Database, ApiError> {
    state
        .database
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("账号功能需要 PostgreSQL"))
}

fn validate_username(value: &str) -> Result<String, ApiError> {
    let username = value.trim();
    let length = username.chars().count();
    if !(3..=32).contains(&length) {
        return Err(ApiError::bad_request("账号名称长度必须为 3 到 32 个字符"));
    }
    if !username
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(ApiError::bad_request(
            "账号名称只能包含文字、数字、下划线和连字符",
        ));
    }
    Ok(username.to_owned())
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if !(8..=128).contains(&password.chars().count()) {
        return Err(ApiError::bad_request("密码长度必须为 8 到 128 个字符"));
    }
    Ok(())
}

fn validate_room(input: RoomInput) -> Result<(String, String, String, u32), ApiError> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad_request("房间名称长度必须为 1 到 120 个字符"));
    }
    const LANGUAGES: &[&str] = &[
        "auto", "zh", "en", "ja", "ko", "fr", "de", "es", "it", "pt", "ru",
    ];
    let source = input.source_language.to_ascii_lowercase();
    let target = input.target_language.to_ascii_lowercase();
    if !LANGUAGES.contains(&source.as_str()) {
        return Err(ApiError::bad_request("不支持该源语言"));
    }
    if target == "auto" || !LANGUAGES.contains(&target.as_str()) {
        return Err(ApiError::bad_request("不支持该目标语言"));
    }
    if !(5..=20).contains(&input.max_utterance_seconds) {
        return Err(ApiError::bad_request("最长断句必须为 5 到 20 秒"));
    }
    Ok((name.to_owned(), source, target, input.max_utterance_seconds))
}

fn default_source_language() -> String {
    "auto".to_owned()
}

fn default_target_language() -> String {
    "zh".to_owned()
}

fn default_max_utterance_seconds() -> u32 {
    20
}

fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::encode_b64(Uuid::new_v4().as_bytes())
        .map_err(|error| anyhow!(error.to_string()))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| anyhow!(error.to_string()))
}

fn verify_password(password: &str, encoded: &str) -> bool {
    let Ok(hash) = PasswordHash::new(encoded) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hashes_verify_without_storing_plaintext() {
        let hash = hash_password("correct-horse").unwrap();
        assert!(!hash.contains("correct-horse"));
        assert!(verify_password("correct-horse", &hash));
        assert!(!verify_password("wrong-password", &hash));
    }

    #[test]
    fn validates_account_and_room_inputs() {
        assert!(validate_username("voice_user-1").is_ok());
        assert!(validate_username("x").is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(
            validate_room(RoomInput {
                name: "Daily room".to_owned(),
                source_language: "en".to_owned(),
                target_language: "zh".to_owned(),
                max_utterance_seconds: 20,
            })
            .is_ok()
        );
        assert!(
            validate_room(RoomInput {
                name: "Too short chunks".to_owned(),
                source_language: "en".to_owned(),
                target_language: "zh".to_owned(),
                max_utterance_seconds: 4,
            })
            .is_err()
        );
    }

    #[test]
    fn hashes_session_tokens_deterministically() {
        assert_eq!(token_hash("token"), token_hash("token"));
        assert_ne!(token_hash("token"), "token");
    }

    #[test]
    fn validates_private_voice_reference_input() {
        assert_eq!(
            validate_voice_reference_name("  我的声音  ").unwrap(),
            "我的声音"
        );
        assert!(validate_voice_reference_name("").is_err());

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reference.wav");
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for index in 0..16_000 * 3 {
            writer
                .write_sample(((index % 200) as i16 - 100) * 100)
                .unwrap();
        }
        writer.finalize().unwrap();
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(validate_voice_reference_wav(&bytes).unwrap(), 3_000);
        assert!(validate_voice_reference_wav(b"not a wav").is_err());
    }
}
