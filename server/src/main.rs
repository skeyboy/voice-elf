mod api;
mod audio;
mod backends;
mod config;
mod media;
mod pipeline;
mod protocol;
mod room_hub;
mod schema;
mod storage;

use std::sync::Arc;

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, Query, State, WebSocketUpgrade, ws::Message},
    http::{HeaderName, HeaderValue, StatusCode, header::CACHE_CONTROL},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;
use tower_cookies::{CookieManagerLayer, Cookies};
use tower_http::{
    compression::CompressionLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    backends::AppServices,
    config::AppConfig,
    media::MediaStore,
    protocol::{ClientEvent, ServerEvent},
    room_hub::RoomHub,
    storage::Database,
};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) services: Arc<AppServices>,
    pub(crate) database: Option<Database>,
    pub(crate) media: MediaStore,
    pub(crate) rooms: RoomHub,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "voice_elf_server=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env()?;
    let services = Arc::new(AppServices::from_config(&config)?);
    let database = match &config.database_url {
        Some(url) => Some(Database::connect(url).await?),
        None => None,
    };
    let database_enabled = database.is_some();
    let media = MediaStore::new(config.media_dir.clone()).await?;
    let state = AppState {
        services: services.clone(),
        database,
        media: media.clone(),
        rooms: RoomHub::default(),
    };
    let static_files = ServeDir::new(&config.web_dist).append_index_html_on_directories(true);
    let index_file = config.web_dist.join("index.html");
    let media_files = Router::new()
        .fallback_service(ServeDir::new(media.root()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authorize_media,
        ));
    let app = Router::new()
        .route("/api/health", get(health))
        .nest("/api", api::router())
        .route("/ws", get(websocket))
        .nest("/media", media_files)
        .route_service("/login", ServeFile::new(&index_file))
        .route_service("/rooms", ServeFile::new(&index_file))
        .route_service("/rooms/{room_id}", ServeFile::new(&index_file))
        .route_service("/settings", ServeFile::new(&index_file))
        .fallback_service(static_files)
        .layer(middleware::from_fn(cache_vad_assets))
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-embedder-policy"),
            HeaderValue::from_static("require-corp"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(CookieManagerLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(
        address = %config.bind,
        backend = services.backend_name,
        database = database_enabled,
        web_dist = %config.web_dist.display(),
        media_dir = %media.root().display(),
        "voice elf server listening"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn cache_vad_assets(request: axum::http::Request<Body>, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    if response.status().is_success() {
        let value = if path == "/wasm/manifest.json" {
            Some(HeaderValue::from_static("no-cache"))
        } else if path.starts_with("/wasm/voice_elf_web_vad.") && path.ends_with(".wasm") {
            Some(HeaderValue::from_static(
                "public, max-age=31536000, immutable",
            ))
        } else {
            None
        };
        if let Some(value) = value {
            response.headers_mut().insert(CACHE_CONTROL, value);
        }
    }
    response
}

async fn authorize_media(
    State(state): State<AppState>,
    cookies: Cookies,
    request: axum::http::Request<Body>,
    next: Next,
) -> Response {
    let user = match api::authenticate(&state, &cookies).await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let Some(database) = &state.database else {
        return api::ApiError::forbidden("账号功能需要 PostgreSQL").into_response();
    };
    let path = request
        .extensions()
        .get::<OriginalUri>()
        .map(|uri| uri.0.path())
        .unwrap_or_else(|| request.uri().path())
        .to_owned();
    let room_id = match database.media_room_for_url(&path).await {
        Ok(Some(room_id)) => room_id,
        Ok(None) => return api::ApiError::not_found("音频不存在").into_response(),
        Err(error) => return api::ApiError::internal(error).into_response(),
    };
    match database.can_view_room(room_id, user.id).await {
        Ok(true) => next.run(request).await,
        Ok(false) => api::ApiError::forbidden("无权访问该房间音频").into_response(),
        Err(error) => api::ApiError::internal(error).into_response(),
    }
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "backend": state.services.backend_name,
            "database": state.database.is_some(),
            "media": true,
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
}

#[derive(Deserialize)]
struct WebSocketQuery {
    room_id: uuid::Uuid,
}

async fn websocket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<WebSocketQuery>,
    cookies: Cookies,
) -> Result<axum::response::Response, api::ApiError> {
    let user = api::authenticate(&state, &cookies).await?;
    let database = state
        .database
        .as_ref()
        .ok_or_else(|| api::ApiError::forbidden("账号功能需要 PostgreSQL"))?;
    let room = database
        .get_room(query.room_id)
        .await
        .map_err(api::ApiError::internal)?
        .ok_or_else(|| api::ApiError::not_found("房间不存在"))?;
    if !database
        .can_view_room(room.id, user.id)
        .await
        .map_err(api::ApiError::internal)?
    {
        return Err(api::ApiError::forbidden("请先加入房间"));
    }
    let members = database
        .list_room_members(room.id)
        .await
        .map_err(api::ApiError::internal)?;
    Ok(ws
        .max_message_size(256 * 1024)
        .max_frame_size(256 * 1024)
        .on_upgrade(move |socket| handle_socket(socket, state, room, user, members))
        .into_response())
}

async fn handle_socket(
    socket: axum::extract::ws::WebSocket,
    state: AppState,
    room: storage::RoomRecord,
    user: storage::UserRecord,
    members: Vec<storage::RoomMemberRecord>,
) {
    let room_id = room.id;
    let user_id = user.id;
    let connection = state.rooms.connect(
        &room,
        &user,
        &members,
        state.services.clone(),
        state.database.clone(),
        state.media.clone(),
    );
    let rooms = state.rooms;
    let can_publish = connection.can_publish;
    let mut room_events = connection.events;
    let (mut socket_writer, mut socket_reader) = socket.split();
    let subscribed = ServerEvent::RoomSubscribed {
        room_id: room_id.to_string(),
        can_publish,
        user_id,
        backend: state.services.backend_name.to_owned(),
    };
    let Ok(subscribed) = serde_json::to_string(&subscribed) else {
        rooms.disconnect(room_id, user_id).await;
        return;
    };
    if socket_writer
        .send(Message::Text(subscribed.into()))
        .await
        .is_err()
    {
        rooms.disconnect(room_id, user_id).await;
        return;
    }

    loop {
        tokio::select! {
            event = room_events.recv() => match event {
                Ok(message) => {
                    if socket_writer.send(message).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(%room_id, skipped, "disconnecting lagged room subscriber");
                    let warning = ServerEvent::Warning {
                        message: "实时消息积压，正在重新同步房间。".to_owned(),
                    };
                    if let Ok(warning) = serde_json::to_string(&warning) {
                        let _ = socket_writer.send(Message::Text(warning.into())).await;
                    }
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket_reader.next() => {
                let Some(result) = incoming else {
                    break;
                };
                match result {
                    Ok(Message::Binary(bytes)) => {
                        if !rooms.send_audio(room_id, user_id, bytes.to_vec()).await {
                            tracing::debug!(%room_id, %user_id, "ignored audio from muted room member");
                        }
                    }
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<ClientEvent>(&text) {
                            Ok(event) => {
                                if !rooms.send_event(room_id, user_id, event).await {
                                    tracing::debug!(%room_id, %user_id, "ignored event from muted room member");
                                }
                            }
                            Err(error) => {
                                let warning = ServerEvent::Warning {
                                    message: format!("Invalid client message: {error}"),
                                };
                                if let Ok(warning) = serde_json::to_string(&warning) {
                                    let _ = socket_writer.send(Message::Text(warning.into())).await;
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(_) | Message::Pong(_)) => {}
                    Ok(Message::Close(_)) | Err(_) => break,
                };
            }
        }
    }

    drop(room_events);
    rooms.disconnect(room_id, user_id).await;
}
