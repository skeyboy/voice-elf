mod api;
mod asr_manager;
mod audio;
mod authority;
mod backends;
mod config;
mod control;
mod grpc;
mod index_tts_runtime;
mod language_policy;
mod mailer;
mod media;
mod pipeline;
mod protocol;
mod room_hub;
mod schema;
mod storage;
mod tts_manager;

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
use tokio::process::Command;
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
    asr_manager::AsrManager,
    authority::AuthorityService,
    backends::{AppServices, AsrBackendRegistry, TtsBackendRegistry},
    config::AppConfig,
    mailer::MailService,
    media::MediaStore,
    protocol::{ClientEvent, ServerEvent},
    room_hub::RoomHub,
    storage::Database,
    tts_manager::TtsManager,
};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) services: Arc<AppServices>,
    pub(crate) database: Option<Database>,
    pub(crate) media: MediaStore,
    pub(crate) rooms: RoomHub,
    pub(crate) authority: AuthorityService,
    pub(crate) asr: AsrManager,
    pub(crate) tts: TtsManager,
    pub(crate) setup_token_hash: Arc<str>,
    pub(crate) mail: MailService,
    pub(crate) control: control::ControlPlaneClient,
    pub(crate) commands: control::RuntimeCommandBus,
}

pub async fn run_combined() -> Result<()> {
    initialize_process();
    let config = AppConfig::from_env()?;
    let (state, media) =
        build_state(&config, true, control::ControlPlaneClient::disabled()).await?;
    let database_enabled = state.database.is_some();
    let app = combined_app(state, &config, &media);
    serve_http(
        config.bind,
        app,
        "voice-elf-server",
        &config,
        database_enabled,
    )
    .await
}

pub async fn run_public() -> Result<()> {
    initialize_process();
    let config = AppConfig::from_env()?;
    let control = control::ControlPlaneClient::from_env()?;
    let (state, media) = build_state(&config, true, control).await?;
    let snapshot = state.control.runtime_snapshot().await;
    log_dependency_snapshot(&snapshot);
    let database_enabled = state.database.is_some();
    let command_control = state.control.clone();
    let command_rooms = state.rooms.clone();
    let app = public_app(state, &config, &media);
    tokio::try_join!(
        serve_http(
            config.bind,
            app,
            "voice-elf-public",
            &config,
            database_enabled,
        ),
        command_control.run_command_listener(command_rooms),
    )?;
    Ok(())
}

pub async fn run_admin() -> Result<()> {
    initialize_process();
    let config = AppConfig::from_env()?;
    let (state, _) = build_state(&config, false, control::ControlPlaneClient::disabled()).await?;
    let http_bind = control::admin_http_bind_from_env()?;
    let grpc_bind = control::admin_grpc_bind_from_env()?;
    let database_enabled = state.database.is_some();
    let app = admin_app(state.clone(), &config);
    tokio::try_join!(
        serve_http(http_bind, app, "voice-elf-admin", &config, database_enabled,),
        control::serve(grpc_bind, state),
    )?;
    Ok(())
}

fn initialize_process() {
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "voice_elf_server=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
}

async fn build_state(
    config: &AppConfig,
    start_index_tts: bool,
    control: control::ControlPlaneClient,
) -> Result<(AppState, MediaStore)> {
    if start_index_tts {
        if config.fun_asr.enabled {
            start_managed_sidecar("FunASR", &config.fun_asr.manager_script).await;
        }
        if config.tts.qwen_tts.enabled {
            start_managed_sidecar("Qwen3-TTS", &config.tts.qwen_tts.manager_script).await;
        }
    }
    let services = Arc::new(AppServices::from_config(&config)?);
    let asr_registry = AsrBackendRegistry::from_config(&config)?;
    let tts_registry = TtsBackendRegistry::from_config(&config, services.synthesizer.clone())?;
    let database = match &config.database_url {
        Some(url) => Some(Database::connect(url).await?),
        None => None,
    };
    if let Some(database) = &database {
        database
            .ensure_asr_system_setting(asr_registry.default_backend_id())
            .await?;
        database
            .ensure_tts_system_setting(tts_registry.default_backend_id())
            .await?;
    }
    let initialized = match &database {
        Some(database) => database.system_installation().await?.is_some(),
        None => false,
    };
    let configured_setup_token = std::env::var("VOICE_ELF_SETUP_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if configured_setup_token
        .as_ref()
        .is_some_and(|token| token.chars().count() < 16)
    {
        anyhow::bail!("VOICE_ELF_SETUP_TOKEN must contain at least 16 characters");
    }
    let generated_setup_token = configured_setup_token.is_none().then(|| {
        format!(
            "vesetup_{}",
            &uuid::Uuid::new_v4().simple().to_string()[..20]
        )
    });
    if !initialized {
        if let Some(token) = &generated_setup_token {
            tracing::warn!(setup_token = %token, "system initialization token generated");
        } else {
            tracing::info!("system initialization requires VOICE_ELF_SETUP_TOKEN");
        }
    }
    let setup_token = configured_setup_token
        .or(generated_setup_token)
        .expect("a setup token is always available");
    let media = MediaStore::new(config.media_dir.clone()).await?;
    let mail_config = match &database {
        Some(database) => database
            .email_setting()
            .await?
            .map(|setting| setting.mail_config())
            .unwrap_or_else(|| config.mail.clone()),
        None => config.mail.clone(),
    };
    let mail = MailService::new(mail_config)?;
    let authority = AuthorityService::new(config.authority.clone());
    let asr = AsrManager::new(asr_registry, database.clone(), authority.clone());
    let tts = TtsManager::new(
        tts_registry,
        database.clone(),
        authority.clone(),
        config.tts.index_tts.clone(),
        config.tts.qwen_tts.clone(),
    )?;
    if start_index_tts && config.tts.index_tts.enabled {
        tts.start_index_if_installed().await;
    }
    let state = AppState {
        services: services.clone(),
        database,
        media: media.clone(),
        rooms: RoomHub::default(),
        authority: authority.clone(),
        asr,
        tts,
        setup_token_hash: Arc::from(api::token_hash(&setup_token)),
        mail,
        control,
        commands: control::RuntimeCommandBus::default(),
    };
    authority.start();
    Ok((state, media))
}

async fn start_managed_sidecar(name: &str, manager_script: &std::path::Path) {
    if !manager_script.is_file() {
        tracing::warn!(sidecar = name, script = %manager_script.display(), "managed sidecar script is missing");
        return;
    }
    match Command::new(manager_script).arg("start").output().await {
        Ok(output) if output.status.success() => {
            tracing::info!(sidecar = name, message = %String::from_utf8_lossy(&output.stdout).trim(), "managed sidecar start requested");
        }
        Ok(output) => {
            tracing::warn!(
                sidecar = name,
                status = %output.status,
                message = %String::from_utf8_lossy(&output.stderr).trim(),
                "managed sidecar could not be started; run its setup command first"
            );
        }
        Err(error) => {
            tracing::warn!(sidecar = name, %error, "managed sidecar start command failed");
        }
    }
}

fn combined_app(state: AppState, config: &AppConfig, media: &MediaStore) -> Router {
    let grpc = grpc::router(state.clone(), api::router(), true);
    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/runtime/dependencies", get(runtime_dependencies))
        .nest("/api", api::file_router())
        .route("/ws", get(websocket))
        .merge(grpc);
    with_public_assets(app, state, config, media, true)
}

fn public_app(state: AppState, config: &AppConfig, media: &MediaStore) -> Router {
    let grpc = grpc::router(state.clone(), api::public_router(), true);
    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/runtime/dependencies", get(runtime_dependencies))
        .nest("/api", api::file_router())
        .route("/ws", get(websocket))
        .merge(grpc);
    with_public_assets(app, state, config, media, false)
}

fn with_public_assets(
    app: Router<AppState>,
    state: AppState,
    config: &AppConfig,
    media: &MediaStore,
    include_admin_pages: bool,
) -> Router {
    let static_files = ServeDir::new(&config.web_dist).append_index_html_on_directories(true);
    let index_file = config.web_dist.join("index.html");
    let media_files = Router::new()
        .fallback_service(ServeDir::new(media.root()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authorize_media,
        ));
    let app = app
        .nest("/media", media_files)
        .route_service("/login", ServeFile::new(&index_file))
        .route_service("/setup", ServeFile::new(&index_file))
        .route_service("/reset-password", ServeFile::new(&index_file))
        .route_service("/rooms", ServeFile::new(&index_file))
        .route_service("/rooms/{room_id}", ServeFile::new(&index_file))
        .route_service("/rooms/{room_id}/subtitles", ServeFile::new(&index_file))
        .route_service("/me", ServeFile::new(&index_file))
        .route_service("/settings", ServeFile::new(&index_file))
        .fallback_service(static_files);
    let app = if include_admin_pages {
        app.route_service("/admin", ServeFile::new(&index_file))
            .route_service("/admin/dependencies", ServeFile::new(&index_file))
            .route_service("/admin/lexicons", ServeFile::new(&index_file))
    } else {
        app
    };
    app.layer(middleware::from_fn(cache_vad_assets))
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-embedder-policy"),
            HeaderValue::from_static("require-corp"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("microphone=(self), display-capture=(self)"),
        ))
        .layer(CookieManagerLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn admin_app(state: AppState, config: &AppConfig) -> Router {
    let index_file = config.web_dist.join("index.html");
    let grpc = grpc::router(state.clone(), api::admin_http_router(), false);
    Router::new()
        .route("/api/health", get(admin_health))
        .route("/api/runtime/dependencies", get(local_runtime_dependencies))
        .nest("/api", api::admin_http_router())
        .merge(grpc)
        .route_service("/setup", ServeFile::new(&index_file))
        .route_service("/admin", ServeFile::new(&index_file))
        .route_service("/admin/dependencies", ServeFile::new(&index_file))
        .route_service("/admin/lexicons", ServeFile::new(&index_file))
        .fallback_service(ServeDir::new(&config.web_dist).append_index_html_on_directories(true))
        .layer(CompressionLayer::new())
        .layer(CookieManagerLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn serve_http(
    bind: std::net::SocketAddr,
    app: Router,
    service: &'static str,
    config: &AppConfig,
    database_enabled: bool,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(
        address = %bind,
        service,
        database = database_enabled,
        web_dist = %config.web_dist.display(),
        authority_mode = config.authority.mode.as_str(),
        "voice elf HTTP service listening"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

fn log_dependency_snapshot(snapshot: &control::RuntimeSnapshotView) {
    tracing::info!(
        service = %snapshot.service,
        status = %snapshot.overall_status,
        "admin control plane startup check completed"
    );
    for dependency in &snapshot.dependencies {
        tracing::info!(
            dependency = %dependency.name,
            kind = %dependency.kind,
            required = dependency.required,
            status = %dependency.status,
            message = %dependency.message,
            "runtime dependency check"
        );
    }
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
    if user.is_admin() {
        return next.run(request).await;
    }
    match database.can_view_room(room_id, user.id).await {
        Ok(true) => next.run(request).await,
        Ok(false) => api::ApiError::forbidden("无权访问该房间音频").into_response(),
        Err(error) => api::ApiError::internal(error).into_response(),
    }
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let authority = state.authority.snapshot().await;
    let asr = state.asr.effective_selection().await.ok();
    let tts = state.tts.effective_selection().await.ok();
    let control = state.control.runtime_snapshot().await;
    let status = if control.overall_status == "ready" {
        "ok"
    } else {
        "degraded"
    };
    (
        StatusCode::OK,
        Json(json!({
            "status": status,
            "backend": state.services.backend_name,
            "asr_backend": asr.as_ref().map(|selection| &selection.backend_id),
            "asr_config_source": asr.as_ref().map(|selection| &selection.source),
            "tts_backend": tts.as_ref().map(|selection| &selection.backend_id),
            "tts_config_source": tts.as_ref().map(|selection| &selection.source),
            "database": state.database.is_some(),
            "media": true,
            "authority": authority,
            "admin_control_plane": control,
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
}

async fn runtime_dependencies(State(state): State<AppState>) -> Json<control::RuntimeSnapshotView> {
    Json(state.control.runtime_snapshot().await)
}

async fn local_runtime_dependencies(
    State(state): State<AppState>,
) -> Json<control::RuntimeSnapshotView> {
    Json(control::admin_snapshot_view(&state).await)
}

async fn admin_health(State(state): State<AppState>) -> impl IntoResponse {
    let snapshot = control::admin_snapshot_view(&state).await;
    (
        StatusCode::OK,
        Json(json!({
            "status": snapshot.overall_status,
            "service": snapshot.service,
            "initialized": snapshot.initialized,
            "authorized": snapshot.authorized,
            "dependencies": snapshot.dependencies,
            "version": snapshot.version,
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
    if room.status != "active" {
        return Err(api::ApiError::forbidden("会议已停止实时语音接入"));
    }
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
    let (services, _) = state
        .asr
        .services_for_session(&state.services)
        .await
        .map_err(|error| api::ApiError::unavailable(format!("ASR 服务不可用: {error}")))?;
    let (services, _) = state
        .tts
        .services_for_session(&services)
        .await
        .map_err(|error| api::ApiError::unavailable(format!("TTS 服务不可用: {error}")))?;
    Ok(ws
        .max_message_size(256 * 1024)
        .max_frame_size(256 * 1024)
        .on_upgrade(move |socket| handle_socket(socket, state, room, user, members, services))
        .into_response())
}

async fn handle_socket(
    socket: axum::extract::ws::WebSocket,
    state: AppState,
    room: storage::RoomRecord,
    user: storage::UserRecord,
    members: Vec<storage::RoomMemberRecord>,
    services: Arc<AppServices>,
) {
    let room_id = room.id;
    let user_id = user.id;
    let connection = state.rooms.connect(
        &room,
        &user,
        &members,
        services,
        state.database.clone(),
        state.media.clone(),
    );
    let rooms = state.rooms;
    let can_publish = connection.can_publish;
    let mut room_events = connection.events;
    let mut revoked = connection.revoked;
    let (mut socket_writer, mut socket_reader) = socket.split();
    let subscribed = ServerEvent::RoomSubscribed {
        room_id: room_id.to_string(),
        can_publish,
        user_id,
        backend: connection.backend.to_owned(),
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
            _ = revoked.recv() => break,
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
