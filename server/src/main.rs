mod api;
mod audio;
mod backends;
mod config;
mod media;
mod pipeline;
mod protocol;
mod schema;
mod storage;

use std::sync::Arc;

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, Query, State, WebSocketUpgrade, ws::Message},
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tower_cookies::{CookieManagerLayer, Cookies};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    backends::AppServices,
    config::AppConfig,
    media::MediaStore,
    pipeline::{PipelineIdentity, PipelineInput, run_pipeline},
    protocol::ClientEvent,
    storage::Database,
};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) services: Arc<AppServices>,
    pub(crate) database: Option<Database>,
    pub(crate) media: MediaStore,
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
    if room.owner_id != user.id {
        return Err(api::ApiError::forbidden("只有房主可以控制实时翻译"));
    }
    let identity = PipelineIdentity {
        user_id: user.id,
        room_id: room.id,
    };
    Ok(ws
        .max_message_size(256 * 1024)
        .max_frame_size(256 * 1024)
        .on_upgrade(move |socket| {
            handle_socket(
                socket,
                state.services,
                state.database,
                state.media,
                identity,
            )
        })
        .into_response())
}

async fn handle_socket(
    socket: axum::extract::ws::WebSocket,
    services: Arc<AppServices>,
    database: Option<Database>,
    media: MediaStore,
    identity: PipelineIdentity,
) {
    let (mut socket_writer, mut socket_reader) = socket.split();
    let (output_tx, mut output_rx) = mpsc::channel::<Message>(256);
    let (input_tx, input_rx) = mpsc::channel::<PipelineInput>(192);

    let writer = tokio::spawn(async move {
        while let Some(message) = output_rx.recv().await {
            if socket_writer.send(message).await.is_err() {
                break;
            }
        }
    });
    let pipeline = tokio::spawn(run_pipeline(
        services, database, media, identity, input_rx, output_tx,
    ));

    while let Some(result) = socket_reader.next().await {
        let input = match result {
            Ok(Message::Binary(bytes)) => PipelineInput::Audio(bytes.to_vec()),
            Ok(Message::Text(text)) => match serde_json::from_str::<ClientEvent>(&text) {
                Ok(event) => PipelineInput::Event(event),
                Err(error) => PipelineInput::Invalid(format!("Invalid client message: {error}")),
            },
            Ok(Message::Ping(payload)) => PipelineInput::Ping(payload.to_vec()),
            Ok(Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Err(_) => break,
        };
        if input_tx.send(input).await.is_err() {
            break;
        }
    }

    drop(input_tx);
    let _ = pipeline.await;
    let _ = writer.await;
}
