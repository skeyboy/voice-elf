use std::{
    io::Write,
    path::{Path as FilePath, PathBuf},
};

use anyhow::{Context, Result};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{
        HeaderValue, StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Utc;
use futures_util::stream;
use serde::Deserialize;
use tempfile::TempPath;
use tokio::io::AsyncReadExt;
use tower_cookies::Cookies;
use uuid::Uuid;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
    AppState,
    storage::{RoomRecord, UtteranceExport},
};

use super::{ApiError, authenticate, database};

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/rooms/{room_id}/export", get(export_room))
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct ExportQuery {
    #[serde(default)]
    source_text: bool,
    #[serde(default)]
    translated_text: bool,
    #[serde(default)]
    source_audio: bool,
    #[serde(default)]
    translated_audio: bool,
    #[serde(default)]
    archive: bool,
}

impl ExportQuery {
    fn has_text(self) -> bool {
        self.source_text || self.translated_text
    }

    fn has_audio(self) -> bool {
        self.source_audio || self.translated_audio
    }

    fn validate(self) -> Result<Self, ApiError> {
        if self.has_text() || self.has_audio() {
            Ok(self)
        } else {
            Err(ApiError::bad_request("请至少选择一种会议记录内容"))
        }
    }
}

struct ExportAudio {
    archive_name: String,
    download_name: String,
    path: PathBuf,
}

async fn export_room(
    State(state): State<AppState>,
    cookies: Cookies,
    Path(room_id): Path<Uuid>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let query = query.validate()?;
    let user = authenticate(&state, &cookies).await?;
    let database = database(&state)?;
    let room = database
        .get_room(room_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("会议不存在"))?;
    if room.status == "archived" {
        return Err(ApiError::not_found("会议不存在"));
    }
    if !database
        .can_view_room(room_id, user.id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::forbidden("无权下载该会议记录"));
    }
    let utterances = database
        .list_utterances_for_export(room_id)
        .await
        .map_err(ApiError::internal)?;
    let text = query
        .has_text()
        .then(|| render_transcript(&room, &utterances, query));
    let audio = collect_audio(&state, &utterances, query)
        .await
        .map_err(ApiError::internal)?;

    if query.has_audio() && audio.is_empty() && text.is_none() {
        return Err(ApiError::not_found("该会议暂无可下载的音频记录"));
    }
    if !query.has_audio() {
        return text_response(room.id, text.unwrap_or_default());
    }
    if !query.archive && text.is_none() && audio.len() == 1 {
        let audio = audio.into_iter().next().expect("one audio file exists");
        return stream_path(&audio.path, None, "audio/wav", &audio.download_name).await;
    }
    zip_response(room.id, text, audio).await
}

async fn collect_audio(
    state: &AppState,
    utterances: &[UtteranceExport],
    query: ExportQuery,
) -> Result<Vec<ExportAudio>> {
    let mut files = Vec::new();
    for (index, utterance) in utterances.iter().enumerate() {
        let prefix = format!(
            "{:04}-{}",
            index + 1,
            utterance.created_at.format("%Y%m%d-%H%M%S")
        );
        if query.source_audio {
            if let Some(path) =
                validated_media_path(state.media.root(), utterance.source_audio_path.as_deref())
                    .await?
            {
                files.push(ExportAudio {
                    archive_name: format!("source-audio/{prefix}-source.wav"),
                    download_name: format!("{prefix}-source.wav"),
                    path,
                });
            }
        }
        if query.translated_audio {
            if let Some(path) = validated_media_path(
                state.media.root(),
                utterance.translated_audio_path.as_deref(),
            )
            .await?
            {
                files.push(ExportAudio {
                    archive_name: format!("translated-audio/{prefix}-translated.wav"),
                    download_name: format!("{prefix}-translated.wav"),
                    path,
                });
            }
        }
    }
    Ok(files)
}

async fn validated_media_path(root: &FilePath, stored: Option<&str>) -> Result<Option<PathBuf>> {
    let Some(stored) = stored else {
        return Ok(None);
    };
    let path = match tokio::fs::canonicalize(stored).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to resolve export audio path"),
    };
    if !path.starts_with(root) {
        anyhow::bail!("refusing to export an audio file outside the media store");
    }
    Ok(Some(path))
}

fn render_transcript(
    room: &RoomRecord,
    utterances: &[UtteranceExport],
    query: ExportQuery,
) -> String {
    let mut output = String::from("\u{feff}");
    let selected = match (query.source_text, query.translated_text) {
        (true, true) => "原文、译文",
        (true, false) => "原文",
        (false, true) => "译文",
        _ => "",
    };
    output.push_str(&format!(
        "会议：{}\n会议 ID：{}\n导出时间：{}\n内容：{}\n\n",
        room.name,
        room.id,
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
        selected,
    ));
    if utterances.is_empty() {
        output.push_str("暂无会议记录。\n");
        return output;
    }
    for utterance in utterances {
        let speakers = if utterance.speakers.is_empty() {
            "未知发言人".to_owned()
        } else {
            utterance
                .speakers
                .iter()
                .map(|speaker| speaker.username.as_str())
                .collect::<Vec<_>>()
                .join("、")
        };
        output.push_str(&format!(
            "[{}] {}\n",
            utterance.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
            speakers,
        ));
        if query.source_text {
            output.push_str(&format!(
                "原文（{}）：{}\n",
                utterance.source_language,
                value_or_placeholder(&utterance.source_text),
            ));
        }
        if query.translated_text {
            output.push_str(&format!(
                "译文（{}）：{}\n",
                utterance.target_language,
                value_or_placeholder(&utterance.translated_text),
            ));
        }
        output.push('\n');
    }
    output
}

fn value_or_placeholder(value: &str) -> &str {
    let value = value.trim();
    if value.is_empty() {
        "[无内容]"
    } else {
        value
    }
}

fn text_response(room_id: Uuid, text: String) -> Result<Response, ApiError> {
    let file_name = format!("voice-elf-room-{room_id}-transcript.txt");
    attachment_response(
        Body::from(text),
        "text/plain; charset=utf-8",
        &file_name,
        None,
    )
}

async fn zip_response(
    room_id: Uuid,
    transcript: Option<String>,
    audio: Vec<ExportAudio>,
) -> Result<Response, ApiError> {
    let archive = tokio::task::spawn_blocking(move || build_zip(transcript, audio))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    let size = archive
        .as_file()
        .metadata()
        .map_err(ApiError::internal)?
        .len();
    let path = archive.into_temp_path();
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(ApiError::internal)?;
    let file_name = format!("voice-elf-room-{room_id}-records.zip");
    stream_file(file, Some(path), "application/zip", &file_name).map(|mut response| {
        response.headers_mut().insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&size.to_string()).expect("file size is a valid header"),
        );
        response
    })
}

fn build_zip(
    transcript: Option<String>,
    audio: Vec<ExportAudio>,
) -> Result<tempfile::NamedTempFile> {
    let archive = tempfile::Builder::new()
        .prefix("voice-elf-export-")
        .suffix(".zip")
        .tempfile()?;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let mut zip = ZipWriter::new(archive);
    if let Some(transcript) = transcript {
        zip.start_file("transcript.txt", options)?;
        zip.write_all(transcript.as_bytes())?;
    }
    for item in audio {
        zip.start_file(item.archive_name, options)?;
        let mut source = std::fs::File::open(&item.path)
            .with_context(|| format!("failed to open export audio: {}", item.path.display()))?;
        std::io::copy(&mut source, &mut zip)?;
    }
    Ok(zip.finish()?)
}

async fn stream_path(
    path: &FilePath,
    cleanup: Option<TempPath>,
    content_type: &'static str,
    file_name: &str,
) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(ApiError::internal)?;
    stream_file(file, cleanup, content_type, file_name)
}

fn stream_file(
    file: tokio::fs::File,
    cleanup: Option<TempPath>,
    content_type: &'static str,
    file_name: &str,
) -> Result<Response, ApiError> {
    let stream = stream::unfold((file, cleanup), |(mut file, cleanup)| async move {
        let mut buffer = vec![0_u8; 64 * 1024];
        match file.read(&mut buffer).await {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                Some((
                    Ok::<_, std::io::Error>(Bytes::from(buffer)),
                    (file, cleanup),
                ))
            }
            Err(error) => Some((Err(error), (file, cleanup))),
        }
    });
    attachment_response(Body::from_stream(stream), content_type, file_name, None)
}

fn attachment_response(
    body: Body,
    content_type: &'static str,
    file_name: &str,
    status: Option<StatusCode>,
) -> Result<Response, ApiError> {
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{file_name}\""))
        .map_err(ApiError::internal)?;
    let mut response = (status.unwrap_or(StatusCode::OK), body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(CONTENT_DISPOSITION, disposition);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SpeakerIdentity;
    use chrono::TimeZone;
    use std::io::Read;

    fn room() -> RoomRecord {
        RoomRecord {
            id: Uuid::nil(),
            owner_id: Uuid::nil(),
            name: "产品周会".to_owned(),
            source_language: "zh".to_owned(),
            target_language: "en".to_owned(),
            max_utterance_seconds: 20,
            status: "ended".to_owned(),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
            updated_at: Utc.timestamp_opt(0, 0).unwrap(),
        }
    }

    fn utterance() -> UtteranceExport {
        UtteranceExport {
            source_text: "你好".to_owned(),
            translated_text: "Hello".to_owned(),
            source_language: "zh".to_owned(),
            target_language: "en".to_owned(),
            source_audio_path: None,
            translated_audio_path: None,
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            speakers: vec![SpeakerIdentity {
                user_id: None,
                username: "Alice".to_owned(),
            }],
        }
    }

    #[test]
    fn transcript_respects_selected_text_fields() {
        let text = render_transcript(
            &room(),
            &[utterance()],
            ExportQuery {
                source_text: true,
                translated_text: false,
                ..ExportQuery::default()
            },
        );
        assert!(text.contains("原文（zh）：你好"));
        assert!(!text.contains("译文（en）"));
        assert!(text.contains("Alice"));
    }

    #[test]
    fn zip_contains_transcript_and_audio_categories() {
        let audio_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(audio_file.path(), b"RIFF-audio").unwrap();
        let archive = build_zip(
            Some("meeting transcript".to_owned()),
            vec![ExportAudio {
                archive_name: "source-audio/0001-source.wav".to_owned(),
                download_name: "0001-source.wav".to_owned(),
                path: audio_file.path().to_owned(),
            }],
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(archive).unwrap();
        let mut transcript = String::new();
        zip.by_name("transcript.txt")
            .unwrap()
            .read_to_string(&mut transcript)
            .unwrap();
        assert_eq!(transcript, "meeting transcript");
        assert_eq!(
            zip.by_name("source-audio/0001-source.wav").unwrap().size(),
            10
        );
    }
}
