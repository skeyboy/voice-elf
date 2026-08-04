use std::{
    fs,
    net::TcpListener as StdTcpListener,
    path::{Path, PathBuf},
    sync::Arc,
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
    routing::{any, get},
};
use futures_util::{SinkExt, StreamExt};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as UpstreamMessage, client::IntoClientRequest},
};
use tower_http::{compression::CompressionLayer, set_header::SetResponseHeaderLayer};
use url::Url;

const DEFAULT_UPSTREAM: &str = "http://192.168.0.63:3001";
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AppConfig {
    api_url: String,
}

pub(crate) struct ServerHandle {
    pub(crate) origin: String,
    pub(crate) task: tauri::async_runtime::JoinHandle<()>,
}

pub(crate) fn start(config_dir: PathBuf) -> Result<ServerHandle> {
    let settings_path = config_dir.join(SETTINGS_FILE);
    let upstream = configured_upstream(&settings_path)?;
    let upstream_description = upstream.to_string();
    let state = ServerState {
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to create the application proxy client")?,
        upstream: Arc::new(RwLock::new(upstream)),
        settings_path,
    };
    let app = router(state);
    let listener = StdTcpListener::bind(("127.0.0.1", 0))
        .context("failed to bind the embedded Axum server")?;
    listener
        .set_nonblocking(true)
        .context("failed to configure the embedded Axum listener")?;
    let address = listener
        .local_addr()
        .context("failed to read the embedded Axum address")?;
    let origin = format!("http://{address}");
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
        .route("/ws", get(proxy_websocket))
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
        .with_state(state)
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
        assert!(WebAssets::get("index.html").is_some());
        assert!(WebAssets::get("wasm/manifest.json").is_some());
    }

    #[tokio::test]
    async fn serves_the_spa_for_client_side_routes() {
        let response = static_asset(OriginalUri("/rooms/test-room".parse().unwrap())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "text/html");
        let body = to_bytes(response.into_body(), 8 * 1024).await.unwrap();
        assert!(body.starts_with(b"<!doctype html>"));
    }

    #[tokio::test]
    async fn returns_not_found_for_missing_versioned_assets() {
        let response =
            static_asset(OriginalUri("/_app/immutable/missing.js".parse().unwrap())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
