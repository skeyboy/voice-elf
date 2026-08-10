use std::{
    fs,
    net::TcpListener as StdTcpListener,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{OriginalUri, State, WebSocketUpgrade, ws},
    http::{
        HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode,
        header::{CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, TRANSFER_ENCODING, UPGRADE},
    },
    response::IntoResponse,
    routing::{any, get, post},
};
use futures_util::{SinkExt, StreamExt};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
#[cfg(desktop)]
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::RwLock;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as UpstreamMessage, client::IntoClientRequest},
};
use tower_http::{compression::CompressionLayer, set_header::SetResponseHeaderLayer};
use url::Url;

const DEFAULT_UPSTREAM: &str = "http://192.168.0.63:3001";
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PROXY_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const SETTINGS_FILE: &str = "app-settings.json";

#[derive(RustEmbed)]
#[folder = "../dist/"]
#[include = "**/*"]
struct WebAssets;

#[derive(Clone)]
struct ServerState {
    client: reqwest::Client,
    upstream: Arc<RwLock<Url>>,
    settings_path: PathBuf,
    shell: Option<ShellWindows>,
}

#[derive(Clone)]
struct ShellWindows {
    app: AppHandle,
    #[cfg(desktop)]
    origin: Url,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AppConfig {
    api_url: String,
}

#[derive(Debug, Deserialize)]
struct SubtitleWindowRequest {
    room_id: String,
}

#[derive(Debug, Deserialize)]
struct SubtitleWindowActionRequest {
    room_id: String,
    action: SubtitleWindowAction,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SubtitleWindowAction {
    State,
    ToggleFullscreen,
    Minimize,
    Hide,
}

#[cfg(desktop)]
#[derive(Debug, Serialize)]
struct SubtitleWindowState {
    fullscreen: bool,
}

pub(crate) struct ServerHandle {
    pub(crate) origin: String,
    pub(crate) task: tauri::async_runtime::JoinHandle<()>,
}

pub(crate) fn start(app_handle: AppHandle, config_dir: PathBuf) -> Result<ServerHandle> {
    let settings_path = config_dir.join(SETTINGS_FILE);
    let upstream = configured_upstream(&settings_path)?;
    let upstream_description = upstream.to_string();
    let listener = StdTcpListener::bind(("127.0.0.1", 0))
        .context("failed to bind the embedded Axum server")?;
    listener
        .set_nonblocking(true)
        .context("failed to configure the embedded Axum listener")?;
    let address = listener
        .local_addr()
        .context("failed to read the embedded Axum address")?;
    let origin = format!("http://{address}");
    let state = ServerState {
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
            .build()
            .context("failed to create the application proxy client")?,
        upstream: Arc::new(RwLock::new(upstream)),
        settings_path,
        shell: Some(ShellWindows {
            app: app_handle,
            #[cfg(desktop)]
            origin: Url::parse(&origin).context("failed to parse the embedded app origin")?,
        }),
    };
    let app = router(state);
    eprintln!("Voice Elf app shell listening at {origin}; upstream: {upstream_description}");
    let task = tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("failed to adopt the embedded Axum listener: {error}");
                return;
            }
        };
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("embedded Axum server stopped: {error}");
        }
    });
    Ok(ServerHandle { origin, task })
}

fn configured_upstream(settings_path: &Path) -> Result<Url> {
    if let Ok(configured) = std::env::var("VOICE_ELF_APP_SERVER_URL") {
        return normalize_upstream(&configured)
            .with_context(|| format!("invalid VOICE_ELF_APP_SERVER_URL: {configured}"));
    }
    if let Some(upstream) = saved_upstream(settings_path) {
        return Ok(upstream);
    }
    let configured = option_env!("VOICE_ELF_APP_SERVER_URL").unwrap_or(DEFAULT_UPSTREAM);
    normalize_upstream(configured)
        .with_context(|| format!("invalid VOICE_ELF_APP_SERVER_URL: {configured}"))
}

fn saved_upstream(settings_path: &Path) -> Option<Url> {
    let contents = match fs::read(settings_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("failed to read saved application settings: {error}");
            return None;
        }
    };
    let config = match serde_json::from_slice::<AppConfig>(&contents) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("failed to parse saved application settings: {error}");
            return None;
        }
    };
    match normalize_upstream(&config.api_url) {
        Ok(upstream) => Some(upstream),
        Err(error) => {
            eprintln!("ignored invalid saved API URL: {error:#}");
            None
        }
    }
}

fn normalize_upstream(configured: &str) -> Result<Url> {
    let configured = configured.trim();
    let mut upstream =
        Url::parse(configured).with_context(|| format!("API 地址不是有效 URL: {configured}"))?;
    if !matches!(upstream.scheme(), "http" | "https") || upstream.host_str().is_none() {
        bail!("API 地址必须是完整的 HTTP 或 HTTPS URL");
    }
    if !upstream.username().is_empty() || upstream.password().is_some() {
        bail!("API 地址不能包含用户名或密码");
    }
    if upstream.query().is_some() || upstream.fragment().is_some() {
        bail!("API 地址不能包含查询参数或片段");
    }
    if !upstream.path().ends_with('/') {
        upstream.set_path(&format!("{}/", upstream.path()));
    }
    Ok(upstream)
}

fn router(state: ServerState) -> Router {
    Router::new()
        .route("/__voice_elf/health", get(shell_health))
        .route(
            "/__voice_elf/config",
            get(get_app_config).put(update_app_config),
        )
        .route("/__voice_elf/subtitle-window", post(open_subtitle_window))
        .route(
            "/__voice_elf/subtitle-window/close",
            post(close_subtitle_window),
        )
        .route(
            "/__voice_elf/subtitle-window/action",
            post(control_subtitle_window),
        )
        .route("/__voice_elf/settings-window", post(open_settings_window))
        .route("/__voice_elf/app/quit", post(confirm_app_quit))
        .route("/__voice_elf/app/quit/cancel", post(cancel_app_quit))
        .route("/__voice_elf/mac-audio/start", post(start_mac_audio))
        .route("/__voice_elf/mac-audio/stop", post(stop_mac_audio))
        .route("/__voice_elf/mac-audio/status", get(mac_audio_status))
        .route("/ws", get(proxy_websocket))
        .route("/voice_elf.v1.ApiService/{*path}", any(proxy_http))
        .route("/api", any(proxy_http))
        .route("/api/{*path}", any(proxy_http))
        .route("/media/{*path}", any(proxy_http))
        .fallback(get(static_asset))
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
        .with_state(state)
}

#[cfg(desktop)]
async fn open_subtitle_window(
    State(state): State<ServerState>,
    Json(request): Json<SubtitleWindowRequest>,
) -> Response<Body> {
    if !valid_room_id(&request.room_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "房间 ID 无效" })),
        )
            .into_response();
    }
    let Some(shell) = state.shell else {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    };
    let label = subtitle_window_label(&request.room_id);
    if let Some(window) = shell.app.get_webview_window(&label) {
        if let Err(error) = crate::show_and_focus(&window) {
            return shell_window_error("无法显示字幕悬浮窗", error);
        }
        return StatusCode::NO_CONTENT.into_response();
    }
    let mut url = shell.origin;
    url.set_path(&format!("/rooms/{}/subtitles", request.room_id));
    match WebviewWindowBuilder::new(&shell.app, label, WebviewUrl::External(url))
        .title("Voice Elf 实时字幕")
        .inner_size(1100.0, 460.0)
        .min_inner_size(420.0, 180.0)
        .resizable(true)
        .maximizable(true)
        .minimizable(true)
        .closable(true)
        .always_on_top(true)
        .center()
        .build()
    {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(error) => shell_window_error("无法创建字幕悬浮窗", error),
    }
}

#[cfg(mobile)]
async fn open_subtitle_window(Json(request): Json<SubtitleWindowRequest>) -> Response<Body> {
    if !valid_room_id(&request.room_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    StatusCode::NOT_IMPLEMENTED.into_response()
}

#[cfg(desktop)]
async fn close_subtitle_window(
    State(state): State<ServerState>,
    Json(request): Json<SubtitleWindowRequest>,
) -> Response<Body> {
    if !valid_room_id(&request.room_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(shell) = state.shell else {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    };
    let Some(window) = shell
        .app
        .get_webview_window(&subtitle_window_label(&request.room_id))
    else {
        return StatusCode::NO_CONTENT.into_response();
    };
    match hide_subtitle_window(&window) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => shell_window_error("无法关闭字幕悬浮窗", error),
    }
}

#[cfg(mobile)]
async fn close_subtitle_window(Json(request): Json<SubtitleWindowRequest>) -> Response<Body> {
    if !valid_room_id(&request.room_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    StatusCode::NOT_IMPLEMENTED.into_response()
}

#[cfg(desktop)]
async fn control_subtitle_window(
    State(state): State<ServerState>,
    Json(request): Json<SubtitleWindowActionRequest>,
) -> Response<Body> {
    if !valid_room_id(&request.room_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(shell) = state.shell else {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    };
    let Some(window) = shell
        .app
        .get_webview_window(&subtitle_window_label(&request.room_id))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let result = match request.action {
        SubtitleWindowAction::State => window.is_fullscreen(),
        SubtitleWindowAction::ToggleFullscreen => toggle_subtitle_fullscreen(&window),
        SubtitleWindowAction::Minimize => minimize_subtitle_window(&window).map(|_| false),
        SubtitleWindowAction::Hide => hide_subtitle_window(&window).map(|_| false),
    };
    match result {
        Ok(fullscreen) => Json(SubtitleWindowState { fullscreen }).into_response(),
        Err(error) => shell_window_error("无法控制字幕悬浮窗", error),
    }
}

#[cfg(mobile)]
async fn control_subtitle_window(
    Json(request): Json<SubtitleWindowActionRequest>,
) -> Response<Body> {
    if !valid_room_id(&request.room_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _ = request.action;
    StatusCode::NOT_IMPLEMENTED.into_response()
}

#[cfg(desktop)]
fn toggle_subtitle_fullscreen(window: &tauri::WebviewWindow) -> tauri::Result<bool> {
    let fullscreen = window.is_fullscreen()?;
    if fullscreen {
        window.set_fullscreen(false)?;
        window.set_always_on_top(true)?;
        return Ok(false);
    }
    window.set_always_on_top(false)?;
    if let Err(error) = window.set_fullscreen(true) {
        let _ = window.set_always_on_top(true);
        return Err(error);
    }
    Ok(true)
}

#[cfg(desktop)]
fn leave_subtitle_fullscreen(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    if window.is_fullscreen()? {
        window.set_fullscreen(false)?;
    }
    window.set_always_on_top(true)
}

#[cfg(desktop)]
fn minimize_subtitle_window(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    leave_subtitle_fullscreen(window)?;
    window.minimize()
}

#[cfg(desktop)]
fn hide_subtitle_window(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    leave_subtitle_fullscreen(window)?;
    window.hide()
}

#[cfg(desktop)]
async fn open_settings_window(State(state): State<ServerState>) -> Response<Body> {
    let Some(shell) = state.shell else {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    };
    match show_settings_window(&shell.app, &shell.origin) {
        Ok(true) => StatusCode::CREATED.into_response(),
        Ok(false) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => shell_window_error("无法打开字幕大屏设置", error),
    }
}

#[cfg(mobile)]
async fn open_settings_window() -> Response<Body> {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

#[cfg(desktop)]
pub(crate) fn show_settings_window(app: &AppHandle, origin: &Url) -> tauri::Result<bool> {
    if let Some(window) = app.get_webview_window("subtitle-settings") {
        crate::show_and_focus(&window)?;
        return Ok(false);
    }
    let mut url = origin.clone();
    url.set_path("/settings");
    WebviewWindowBuilder::new(app, "subtitle-settings", WebviewUrl::External(url))
        .title("Voice Elf 设置")
        .inner_size(920.0, 800.0)
        .min_inner_size(600.0, 640.0)
        .resizable(true)
        .maximizable(true)
        .minimizable(true)
        .closable(true)
        .center()
        .build()?;
    Ok(true)
}

async fn confirm_app_quit(State(state): State<ServerState>) -> Response<Body> {
    let Some(shell) = state.shell else {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    };
    crate::confirm_app_exit(&shell.app);
    StatusCode::NO_CONTENT.into_response()
}

async fn cancel_app_quit(State(state): State<ServerState>) -> Response<Body> {
    let Some(shell) = state.shell else {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    };
    crate::cancel_app_exit(&shell.app);
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(target_os = "macos")]
async fn start_mac_audio(State(state): State<ServerState>) -> Response<Body> {
    let Some(shell) = state.shell else {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    };
    match crate::macos_audio_capture::start(shell.app) {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(message) => (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
    }
}

#[cfg(not(target_os = "macos"))]
async fn start_mac_audio() -> Response<Body> {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

#[cfg(target_os = "macos")]
async fn stop_mac_audio() -> Response<Body> {
    crate::macos_audio_capture::stop();
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(target_os = "macos")]
async fn mac_audio_status() -> Response<Body> {
    Json(crate::macos_audio_capture::status()).into_response()
}

#[cfg(not(target_os = "macos"))]
async fn mac_audio_status() -> Response<Body> {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

#[cfg(not(target_os = "macos"))]
async fn stop_mac_audio() -> Response<Body> {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

fn valid_room_id(room_id: &str) -> bool {
    !room_id.is_empty()
        && room_id.len() <= 80
        && room_id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_'))
}

#[cfg(desktop)]
fn subtitle_window_label(room_id: &str) -> String {
    format!("subtitles-{room_id}")
}

#[cfg(desktop)]
fn shell_window_error(message: &str, error: tauri::Error) -> Response<Body> {
    eprintln!("{message}: {error}");
    shell_window_message(message)
}

#[cfg(desktop)]
fn shell_window_message(message: &str) -> Response<Body> {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}

async fn shell_health(State(state): State<ServerState>) -> impl IntoResponse {
    let upstream = state.upstream.read().await;
    Json(serde_json::json!({
        "status": "ok",
        "service": "voice-elf-app-shell",
        "upstream": upstream.as_str(),
    }))
}

async fn get_app_config(State(state): State<ServerState>) -> Json<AppConfig> {
    let upstream = state.upstream.read().await;
    Json(AppConfig {
        api_url: upstream.as_str().to_owned(),
    })
}

async fn update_app_config(
    State(state): State<ServerState>,
    Json(config): Json<AppConfig>,
) -> Response<Body> {
    let upstream = match normalize_upstream(&config.api_url) {
        Ok(upstream) => upstream,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    };
    let stored = AppConfig {
        api_url: upstream.as_str().to_owned(),
    };
    if let Err(error) = persist_app_config(&state.settings_path, &stored).await {
        eprintln!("failed to persist application settings: {error:#}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "无法保存应用设置" })),
        )
            .into_response();
    }
    *state.upstream.write().await = upstream;
    Json(stored).into_response()
}

async fn persist_app_config(settings_path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = settings_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create the application config directory")?;
    }
    let contents = serde_json::to_vec_pretty(config).context("failed to serialize app settings")?;
    tokio::fs::write(settings_path, contents)
        .await
        .context("failed to write app settings")
}

async fn static_asset(OriginalUri(uri): OriginalUri) -> Response<Body> {
    let path = uri.path().trim_start_matches('/');
    let asset_path = if path.is_empty() { "index.html" } else { path };
    if let Some(asset) = WebAssets::get(asset_path) {
        return asset_response(asset_path, asset.data.into_owned());
    }

    if !asset_path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .contains('.')
        && let Some(index) = WebAssets::get("index.html")
    {
        return asset_response("index.html", index.data.into_owned());
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from("Not Found"))
        .expect("static 404 response is valid")
}

fn asset_response(path: &str, body: Vec<u8>) -> Response<Body> {
    let content_type = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if path == "wasm/manifest.json" || path == "_app/version.json" {
        "no-cache"
    } else if path.starts_with("_app/immutable/")
        || (path.starts_with("wasm/voice_elf_web_vad.") && path.ends_with(".wasm"))
    {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type.as_ref())
        .header("cache-control", cache_control)
        .body(Body::from(body))
        .expect("embedded asset response is valid")
}

async fn proxy_http(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    request: Request<Body>,
) -> Response<Body> {
    match forward_http(&state, uri, request).await {
        Ok(response) => response,
        Err(error) => {
            eprintln!("application proxy request failed: {error:#}");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": "无法连接 Voice Elf 服务" })),
            )
                .into_response()
        }
    }
}

async fn forward_http(
    state: &ServerState,
    uri: axum::http::Uri,
    request: Request<Body>,
) -> Result<Response<Body>> {
    let configured_upstream = state.upstream.read().await.clone();
    let upstream = upstream_url(&configured_upstream, &uri)?;
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, MAX_PROXY_REQUEST_BYTES)
        .await
        .context("proxy request body exceeded its limit")?;
    let mut forwarded = state.client.request(parts.method, upstream).body(body);
    for (name, value) in &parts.headers {
        if !is_hop_by_hop(name) && *name != HOST && *name != CONTENT_LENGTH {
            forwarded = forwarded.header(name, value);
        }
    }
    let upstream_response = forwarded.send().await.context("upstream request failed")?;
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let mut response = Response::builder().status(status);
    for (name, value) in &headers {
        if !is_hop_by_hop(name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from_stream(upstream_response.bytes_stream()))
        .context("failed to construct the proxy response")
}

async fn proxy_websocket(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response<Body> {
    let upstream = state.upstream.read().await.clone();
    match connect_upstream_websocket(&upstream, &uri, &headers).await {
        Ok(upstream) => upgrade
            .max_message_size(256 * 1024)
            .max_frame_size(256 * 1024)
            .on_upgrade(move |client| bridge_websockets(client, upstream))
            .into_response(),
        Err(error) => {
            eprintln!("application WebSocket proxy failed: {error:#}");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": "无法连接 Voice Elf 实时服务" })),
            )
                .into_response()
        }
    }
}

async fn connect_upstream_websocket(
    upstream: &Url,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let mut url = upstream_url(upstream, uri)?;
    url.set_scheme(if upstream.scheme() == "https" {
        "wss"
    } else {
        "ws"
    })
    .map_err(|_| anyhow::anyhow!("failed to select the upstream WebSocket scheme"))?;
    let mut request = url
        .as_str()
        .into_client_request()
        .context("failed to create the upstream WebSocket request")?;
    if let Some(cookie) = headers.get("cookie") {
        request.headers_mut().insert("cookie", cookie.clone());
    }
    if let Some(user_agent) = headers.get("user-agent") {
        request
            .headers_mut()
            .insert("user-agent", user_agent.clone());
    }
    let (socket, _) = connect_async(request)
        .await
        .context("upstream WebSocket handshake failed")?;
    Ok(socket)
}

async fn bridge_websockets(
    client: ws::WebSocket,
    upstream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let (mut client_writer, mut client_reader) = client.split();
    let (mut upstream_writer, mut upstream_reader) = upstream.split();
    loop {
        tokio::select! {
            message = client_reader.next() => match message {
                Some(Ok(message)) => {
                    let close = matches!(message, ws::Message::Close(_));
                    if upstream_writer.send(to_upstream_message(message)).await.is_err() || close {
                        break;
                    }
                }
                _ => break,
            },
            message = upstream_reader.next() => match message {
                Some(Ok(message)) => {
                    if let Some(message) = to_client_message(message) {
                        let close = matches!(message, ws::Message::Close(_));
                        if client_writer.send(message).await.is_err() || close {
                            break;
                        }
                    }
                }
                _ => break,
            },
        }
    }
    let _ = upstream_writer.close().await;
    let _ = client_writer.close().await;
}

fn to_upstream_message(message: ws::Message) -> UpstreamMessage {
    match message {
        ws::Message::Text(text) => UpstreamMessage::Text(text.to_string().into()),
        ws::Message::Binary(bytes) => UpstreamMessage::Binary(bytes.to_vec().into()),
        ws::Message::Ping(bytes) => UpstreamMessage::Ping(bytes.to_vec().into()),
        ws::Message::Pong(bytes) => UpstreamMessage::Pong(bytes.to_vec().into()),
        ws::Message::Close(_) => UpstreamMessage::Close(None),
    }
}

fn to_client_message(message: UpstreamMessage) -> Option<ws::Message> {
    match message {
        UpstreamMessage::Text(text) => Some(ws::Message::Text(text.to_string().into())),
        UpstreamMessage::Binary(bytes) => Some(ws::Message::Binary(bytes.to_vec().into())),
        UpstreamMessage::Ping(bytes) => Some(ws::Message::Ping(bytes.to_vec().into())),
        UpstreamMessage::Pong(bytes) => Some(ws::Message::Pong(bytes.to_vec().into())),
        UpstreamMessage::Close(_) => Some(ws::Message::Close(None)),
        UpstreamMessage::Frame(_) => None,
    }
}

fn upstream_url(upstream: &Url, uri: &axum::http::Uri) -> Result<Url> {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or_else(|| uri.path());
    upstream
        .join(path_and_query.trim_start_matches('/'))
        .context("failed to construct the upstream URL")
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    *name == CONNECTION || *name == TRANSFER_ENCODING || *name == UPGRADE
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn preserves_upstream_base_paths() {
        let upstream = Url::parse("https://example.com/voice-elf/").unwrap();
        let uri = "/api/health?probe=app".parse().unwrap();
        assert_eq!(
            upstream_url(&upstream, &uri).unwrap().as_str(),
            "https://example.com/voice-elf/api/health?probe=app"
        );
    }

    #[test]
    fn normalizes_and_validates_api_urls() {
        assert_eq!(
            normalize_upstream(" https://example.com/voice-elf ")
                .unwrap()
                .as_str(),
            "https://example.com/voice-elf/"
        );
        assert!(normalize_upstream("ftp://example.com").is_err());
        assert!(normalize_upstream("https://user:secret@example.com").is_err());
        assert!(normalize_upstream("https://example.com?token=secret").is_err());
    }

    #[tokio::test]
    async fn updates_the_proxy_and_persists_the_selected_api_url() {
        let directory = tempdir().unwrap();
        let settings_path = directory.path().join(SETTINGS_FILE);
        let config = AppConfig {
            api_url: "https://api.example.com/voice-elf/".to_owned(),
        };
        let state = ServerState {
            client: reqwest::Client::new(),
            upstream: Arc::new(RwLock::new(Url::parse(DEFAULT_UPSTREAM).unwrap())),
            settings_path: settings_path.clone(),
            shell: None,
        };
        let response = update_app_config(State(state.clone()), Json(config.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(state.upstream.read().await.as_str(), config.api_url);
        assert_eq!(
            saved_upstream(&settings_path).unwrap().as_str(),
            config.api_url
        );
    }

    #[test]
    fn embedded_assets_include_the_spa_and_wasm() {
        let index = WebAssets::get("index.html").expect("embedded web index");
        let index = String::from_utf8_lossy(index.data.as_ref());
        assert!(index.contains("voice-elf-boot"));
        assert!(index.contains("正在启动 Voice Elf"));
        assert!(WebAssets::get("wasm/manifest.json").is_some());
    }

    #[tokio::test]
    async fn serves_the_spa_for_client_side_routes() {
        let response =
            static_asset(OriginalUri("/rooms/test-room/subtitles".parse().unwrap())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "text/html");
        let body = to_bytes(response.into_body(), 8 * 1024).await.unwrap();
        assert!(body.starts_with(b"<!doctype html>"));
    }

    #[test]
    fn accepts_only_safe_room_ids_for_window_labels_and_paths() {
        assert!(valid_room_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(valid_room_id("meeting_room-1"));
        assert!(!valid_room_id("../settings"));
        assert!(!valid_room_id("room?admin=true"));
        assert!(!valid_room_id(""));
    }

    #[tokio::test]
    async fn returns_not_found_for_missing_versioned_assets() {
        let response =
            static_asset(OriginalUri("/_app/immutable/missing.js".parse().unwrap())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
