use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};
use tower_cookies::Cookies;
use uuid::Uuid;

use crate::{
    AppState,
    storage::{
        BlockedWord, BlockedWordInput, RoomTerminologyBinding, TerminologyDictionary,
        TerminologyDictionaryInput, TerminologyEntry, TerminologyEntryInput,
    },
};

use super::{ApiError, authenticate, database, require_admin};

pub(super) fn public_router() -> Router<AppState> {
    Router::new()
        .route(
            "/terminology-dictionaries",
            get(list_available_dictionaries),
        )
        .route(
            "/rooms/{room_id}/terminology-dictionary",
            get(room_binding).patch(update_room_binding),
        )
}

pub(super) fn admin_router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/terminology-dictionaries",
            get(list_dictionaries).post(create_dictionary),
        )
        .route(
            "/admin/terminology-dictionaries/{id}",
            patch(update_dictionary).delete(delete_dictionary),
        )
        .route(
            "/admin/terminology-dictionaries/{id}/entries",
            get(list_entries).post(create_entry),
        )
        .route(
            "/admin/terminology-dictionaries/{id}/import",
            post(import_entries),
        )
        .route(
            "/admin/terminology-entries/{id}",
            patch(update_entry).delete(delete_entry),
        )
        .route(
            "/admin/blocked-words",
            get(list_blocked_words).post(create_blocked_word),
        )
        .route("/admin/blocked-words/import", post(import_blocked_words))
        .route(
            "/admin/blocked-words/{id}",
            patch(update_blocked_word).delete(delete_blocked_word),
        )
}

async fn list_available_dictionaries(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<Vec<TerminologyDictionary>>, ApiError> {
    authenticate(&state, &cookies).await?;
    Ok(Json(
        database(&state)?
            .list_terminology_dictionaries(false)
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn list_dictionaries(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<Vec<TerminologyDictionary>>, ApiError> {
    require_admin(&state, &cookies).await?;
    Ok(Json(
        database(&state)?
            .list_terminology_dictionaries(true)
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn create_dictionary(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<TerminologyDictionaryInput>,
) -> Result<(StatusCode, Json<TerminologyDictionary>), ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    let input = validate_dictionary(input)?;
    let value = database(&state)?
        .create_terminology_dictionary(admin.id, &input)
        .await
        .map_err(map_write_error)?;
    Ok((StatusCode::CREATED, Json(value)))
}

async fn update_dictionary(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
    Json(input): Json<TerminologyDictionaryInput>,
) -> Result<Json<TerminologyDictionary>, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    database(&state)?
        .update_terminology_dictionary(id, admin.id, &validate_dictionary(input)?)
        .await
        .map_err(map_write_error)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("行业词库不存在"))
}

async fn delete_dictionary(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    if !database(&state)?
        .delete_terminology_dictionary(id, admin.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::not_found("行业词库不存在"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_entries(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<TerminologyEntry>>, ApiError> {
    require_admin(&state, &cookies).await?;
    Ok(Json(
        database(&state)?
            .list_terminology_entries(id, true)
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn create_entry(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
    Json(input): Json<TerminologyEntryInput>,
) -> Result<(StatusCode, Json<TerminologyEntry>), ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    let value = database(&state)?
        .create_terminology_entry(id, admin.id, &validate_entry(input)?)
        .await
        .map_err(map_write_error)?;
    Ok((StatusCode::CREATED, Json(value)))
}

async fn update_entry(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
    Json(input): Json<TerminologyEntryInput>,
) -> Result<Json<TerminologyEntry>, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    database(&state)?
        .update_terminology_entry(id, admin.id, &validate_entry(input)?)
        .await
        .map_err(map_write_error)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("术语不存在"))
}

async fn delete_entry(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    if !database(&state)?
        .delete_terminology_entry(id, admin.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::not_found("术语不存在"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_blocked_words(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Json<Vec<BlockedWord>>, ApiError> {
    require_admin(&state, &cookies).await?;
    Ok(Json(
        database(&state)?
            .list_blocked_words(true)
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn create_blocked_word(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<BlockedWordInput>,
) -> Result<(StatusCode, Json<BlockedWord>), ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    let value = database(&state)?
        .create_blocked_word(admin.id, &validate_blocked(input)?)
        .await
        .map_err(map_write_error)?;
    Ok((StatusCode::CREATED, Json(value)))
}

async fn update_blocked_word(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
    Json(input): Json<BlockedWordInput>,
) -> Result<Json<BlockedWord>, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    database(&state)?
        .update_blocked_word(id, admin.id, &validate_blocked(input)?)
        .await
        .map_err(map_write_error)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("屏蔽词不存在"))
}

async fn delete_blocked_word(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    if !database(&state)?
        .delete_blocked_word(id, admin.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::not_found("屏蔽词不存在"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ImportInput {
    content: String,
}

#[derive(Serialize)]
struct ImportResult {
    imported: usize,
    errors: Vec<String>,
}

async fn import_entries(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(id): Path<Uuid>,
    Json(input): Json<ImportInput>,
) -> Result<Json<ImportResult>, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    validate_import_size(&input.content)?;
    let mut result = ImportResult {
        imported: 0,
        errors: Vec::new(),
    };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(input.content.as_bytes());
    for (index, record) in reader.records().enumerate() {
        let values = match record {
            Ok(record) => record,
            Err(error) => {
                result
                    .errors
                    .push(format!("第 {} 行：CSV 格式错误：{}", index + 1, error));
                continue;
            }
        };
        if values.iter().all(|value| value.trim().is_empty())
            || (index == 0
                && values
                    .get(0)
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("source_term")))
        {
            continue;
        }
        if values.len() < 2 {
            result
                .errors
                .push(format!("第 {} 行至少需要原词和目标词", index + 1));
            continue;
        }
        let entry = TerminologyEntryInput {
            source_term: values[0].trim().to_owned(),
            target_term: values[1].trim().to_owned(),
            aliases: values
                .get(2)
                .map(|value| {
                    value
                        .split('|')
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            priority: values
                .get(3)
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(100),
            status: "active".to_owned(),
        };
        match validate_entry(entry) {
            Ok(entry) => match database(&state)?
                .create_terminology_entry(id, admin.id, &entry)
                .await
            {
                Ok(_) => result.imported += 1,
                Err(error) => result.errors.push(format!(
                    "第 {} 行：{}",
                    index + 1,
                    friendly_write_error(&error)
                )),
            },
            Err(error) => result
                .errors
                .push(format!("第 {} 行：{}", index + 1, error.message)),
        }
    }
    Ok(Json(result))
}

async fn import_blocked_words(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(input): Json<ImportInput>,
) -> Result<Json<ImportResult>, ApiError> {
    let admin = require_admin(&state, &cookies).await?;
    validate_import_size(&input.content)?;
    let mut result = ImportResult {
        imported: 0,
        errors: Vec::new(),
    };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(input.content.as_bytes());
    for (index, record) in reader.records().enumerate() {
        let values = match record {
            Ok(record) => record,
            Err(error) => {
                result
                    .errors
                    .push(format!("第 {} 行：CSV 格式错误：{}", index + 1, error));
                continue;
            }
        };
        if values.iter().all(|value| value.trim().is_empty())
            || (index == 0
                && values
                    .get(0)
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("word")))
        {
            continue;
        }
        let word = BlockedWordInput {
            word: values[0].trim().to_owned(),
            replacement: values
                .get(1)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("***")
                .to_owned(),
            match_mode: values
                .get(2)
                .map(str::trim)
                .filter(|value| *value == "word")
                .unwrap_or("substring")
                .to_owned(),
            case_sensitive: values
                .get(3)
                .is_some_and(|value| matches!(value.trim(), "true" | "1")),
            status: "active".to_owned(),
            note: values.get(4).map(str::trim).unwrap_or("").to_owned(),
        };
        match validate_blocked(word) {
            Ok(word) => match database(&state)?.create_blocked_word(admin.id, &word).await {
                Ok(_) => result.imported += 1,
                Err(error) => result.errors.push(format!(
                    "第 {} 行：{}",
                    index + 1,
                    friendly_write_error(&error)
                )),
            },
            Err(error) => result
                .errors
                .push(format!("第 {} 行：{}", index + 1, error.message)),
        }
    }
    Ok(Json(result))
}

#[derive(Deserialize)]
struct BindingInput {
    dictionary_id: Option<Uuid>,
}

async fn room_binding(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(room_id): Path<Uuid>,
) -> Result<Json<RoomTerminologyBinding>, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    database(&state)?
        .get_room(room_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("房间不存在"))?;
    if !database(&state)?
        .can_view_room(room_id, user.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::forbidden("无权查看该房间设置"));
    }
    Ok(Json(
        database(&state)?
            .room_terminology_binding(room_id)
            .await
            .map_err(ApiError::internal)?,
    ))
}

async fn update_room_binding(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(room_id): Path<Uuid>,
    Json(input): Json<BindingInput>,
) -> Result<Json<RoomTerminologyBinding>, ApiError> {
    let user = authenticate(&state, &cookies).await?;
    let room = database(&state)?
        .get_room(room_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("房间不存在"))?;
    if room.owner_id != user.id {
        return Err(ApiError::forbidden("只有房主可以修改行业词库"));
    }
    if let Some(id) = input.dictionary_id {
        let exists = database(&state)?
            .list_terminology_dictionaries(false)
            .await
            .map_err(ApiError::internal)?
            .iter()
            .any(|value| value.id == id);
        if !exists {
            return Err(ApiError::bad_request("所选行业词库不可用"));
        }
    }
    database(&state)?
        .set_room_terminology_binding(room_id, input.dictionary_id, user.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(
        database(&state)?
            .room_terminology_binding(room_id)
            .await
            .map_err(ApiError::internal)?,
    ))
}

fn validate_dictionary(
    mut input: TerminologyDictionaryInput,
) -> Result<TerminologyDictionaryInput, ApiError> {
    input.name = input.name.trim().to_owned();
    input.industry = input.industry.trim().to_owned();
    input.description = input.description.trim().to_owned();
    if input.name.is_empty() || input.name.chars().count() > 120 {
        return Err(ApiError::bad_request("词库名称长度必须为 1 到 120 个字符"));
    }
    if input.industry.is_empty() || input.industry.chars().count() > 80 {
        return Err(ApiError::bad_request("行业名称长度必须为 1 到 80 个字符"));
    }
    if input.description.chars().count() > 2000 {
        return Err(ApiError::bad_request("词库说明不能超过 2000 个字符"));
    }
    const LANGUAGES: &[&str] = &[
        "auto", "zh", "en", "ja", "ko", "fr", "de", "es", "it", "pt", "ru",
    ];
    input.source_language = input.source_language.trim().to_ascii_lowercase();
    input.target_language = input.target_language.trim().to_ascii_lowercase();
    if !LANGUAGES.contains(&input.source_language.as_str())
        || input.target_language == "auto"
        || !LANGUAGES.contains(&input.target_language.as_str())
    {
        return Err(ApiError::bad_request("词库语言方向无效"));
    }
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request("词库状态无效"));
    }
    Ok(input)
}

fn validate_entry(mut input: TerminologyEntryInput) -> Result<TerminologyEntryInput, ApiError> {
    input.source_term = input.source_term.trim().to_owned();
    input.target_term = input.target_term.trim().to_owned();
    input.aliases = input
        .aliases
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .take(20)
        .collect();
    if input
        .aliases
        .iter()
        .any(|value| value.chars().count() > 240)
    {
        return Err(ApiError::bad_request("术语别名不能超过 240 个字符"));
    }
    if input.source_term.is_empty()
        || input.target_term.is_empty()
        || input.source_term.chars().count() > 240
        || input.target_term.chars().count() > 240
    {
        return Err(ApiError::bad_request("原词和目标词必须为 1 到 240 个字符"));
    }
    if !(0..=1000).contains(&input.priority)
        || !matches!(input.status.as_str(), "active" | "disabled")
    {
        return Err(ApiError::bad_request("术语优先级或状态无效"));
    }
    Ok(input)
}

fn validate_blocked(mut input: BlockedWordInput) -> Result<BlockedWordInput, ApiError> {
    input.word = input.word.trim().to_owned();
    input.replacement = input.replacement.trim().to_owned();
    input.note = input.note.trim().to_owned();
    if input.word.is_empty()
        || input.word.chars().count() > 240
        || input.replacement.chars().count() > 240
    {
        return Err(ApiError::bad_request("屏蔽词或替换文本长度无效"));
    }
    if input.note.chars().count() > 1000 {
        return Err(ApiError::bad_request("屏蔽词备注不能超过 1000 个字符"));
    }
    if !matches!(input.match_mode.as_str(), "substring" | "word")
        || !matches!(input.status.as_str(), "active" | "disabled")
    {
        return Err(ApiError::bad_request("匹配方式或状态无效"));
    }
    Ok(input)
}

fn validate_import_size(content: &str) -> Result<(), ApiError> {
    if content.len() > 1024 * 1024 {
        return Err(ApiError::bad_request("导入文件不能超过 1 MB"));
    }
    if content.lines().count() > 5001 {
        return Err(ApiError::bad_request("单次最多导入 5000 条记录"));
    }
    Ok(())
}

fn map_write_error(error: anyhow::Error) -> ApiError {
    ApiError::conflict(friendly_write_error(&error))
}
fn friendly_write_error(error: &anyhow::Error) -> String {
    if error.to_string().contains("duplicate key") {
        "记录已存在".to_owned()
    } else {
        "无法保存词库记录".to_owned()
    }
}
