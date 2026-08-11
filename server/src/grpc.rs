use std::{collections::HashMap, pin::Pin, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ws::Message,
    http::{HeaderName, HeaderValue, Method, Request as HttpRequest, Uri},
};
use futures_util::Stream;
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, metadata::MetadataValue};
use tower::{Layer, ServiceExt};
use uuid::Uuid;

use crate::{
    AppState,
    protocol::{ClientEvent, ServerEvent},
};

pub(crate) mod pb {
    tonic::include_proto!("voice_elf.v1");
}

use pb::{
    ApiRequest, ApiResponse, Header, RealtimeAck, RealtimeInput, RealtimeOutput,
    RealtimeSubscribeRequest,
    api_service_server::{ApiService, ApiServiceServer},
    realtime_input, realtime_output,
};

const MAX_RPC_BODY_BYTES: usize = 20 * 1024 * 1024;

#[derive(Clone)]
struct GrpcApi {
    router: Router,
    state: AppState,
    sessions: Arc<RwLock<HashMap<Uuid, RealtimeSession>>>,
    realtime_enabled: bool,
}

#[derive(Clone)]
struct RealtimeSession {
    room_id: Uuid,
    user_id: Uuid,
    auth_hash: String,
}

#[tonic::async_trait]
impl ApiService for GrpcApi {
    type SubscribeRealtimeStream =
        Pin<Box<dyn Stream<Item = Result<RealtimeOutput, Status>> + Send + 'static>>;

    async fn call(&self, request: Request<ApiRequest>) -> Result<Response<ApiResponse>, Status> {
        let metadata = request.metadata().clone();
        let request = request.into_inner();
        if !request.path.starts_with("/api/") || request.path.contains("//") {
            return Err(Status::invalid_argument("RPC path must target /api/"));
        }
        let method = Method::from_bytes(request.method.as_bytes())
            .map_err(|_| Status::invalid_argument("invalid HTTP method"))?;
        let uri = request
            .path
            .parse::<Uri>()
            .map_err(|_| Status::invalid_argument("invalid RPC path"))?;
        let mut builder = HttpRequest::builder().method(method).uri(uri);
        let headers = builder
            .headers_mut()
            .ok_or_else(|| Status::internal("failed to create request headers"))?;
        for header in request.headers {
            let Ok(name) = HeaderName::from_bytes(header.name.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_bytes(&header.value) else {
                continue;
            };
            headers.append(name, value);
        }
        if let Some(cookie) = metadata.get("cookie")
            && let Ok(value) = HeaderValue::from_bytes(cookie.as_encoded_bytes())
        {
            headers.insert(axum::http::header::COOKIE, value);
        }
        let http_request = builder
            .body(Body::from(request.body))
            .map_err(|_| Status::internal("failed to build internal request"))?;
        let response = self
            .router
            .clone()
            .oneshot(http_request)
            .await
            .map_err(|error| Status::internal(format!("API routing failed: {error}")))?;
        let (parts, body) = response.into_parts();
        let body = to_bytes(body, MAX_RPC_BODY_BYTES).await.map_err(|error| {
            Status::resource_exhausted(format!("API response too large: {error}"))
        })?;
        let response_headers = parts
            .headers
            .iter()
            .filter(|(name, _)| !is_transport_header(name))
            .map(|(name, value)| Header {
                name: name.as_str().to_owned(),
                value: value.as_bytes().to_vec(),
            })
            .collect();
        let mut rpc_response = Response::new(ApiResponse {
            status: parts.status.as_u16().into(),
            headers: response_headers,
            body: body.to_vec(),
        });
        for value in parts.headers.get_all(axum::http::header::SET_COOKIE) {
            if let Ok(value) = MetadataValue::try_from(value.as_bytes()) {
                rpc_response.metadata_mut().append("set-cookie", value);
            }
        }
        Ok(rpc_response)
    }

    async fn subscribe_realtime(
        &self,
        request: Request<RealtimeSubscribeRequest>,
    ) -> Result<Response<Self::SubscribeRealtimeStream>, Status> {
        if !self.realtime_enabled {
            return Err(Status::unimplemented(
                "the admin service does not expose realtime room streams",
            ));
        }
        let token =
            auth_token(request.metadata()).ok_or_else(|| Status::unauthenticated("请先登录"))?;
        let user = crate::api::authenticate_token(&self.state, &token)
            .await
            .map_err(|_| Status::unauthenticated("登录状态已失效"))?;
        let room_id = Uuid::parse_str(&request.get_ref().room_id)
            .map_err(|_| Status::invalid_argument("房间 ID 无效"))?;
        let database = self
            .state
            .database
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("账号功能需要 PostgreSQL"))?;
        let room = database
            .get_room(room_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| Status::not_found("房间不存在"))?;
        if room.status != "active" {
            return Err(Status::permission_denied("会议已停止实时语音接入"));
        }
        if !database
            .can_view_room(room.id, user.id)
            .await
            .map_err(internal)?
        {
            return Err(Status::permission_denied("请先加入房间"));
        }
        let members = database
            .list_room_members(room.id)
            .await
            .map_err(internal)?;
        let (services, _) = self
            .state
            .asr
            .services_for_session(&self.state.services)
            .await
            .map_err(|error| Status::unavailable(format!("ASR 服务不可用: {error}")))?;
        let (services, _) = self
            .state
            .tts
            .services_for_session(&services)
            .await
            .map_err(|error| Status::unavailable(format!("TTS 服务不可用: {error}")))?;
        let connection = self.state.rooms.connect(
            &room,
            &user,
            &members,
            services,
            self.state.database.clone(),
            self.state.media.clone(),
        );
        let session_id = Uuid::new_v4();
        self.sessions.write().await.insert(
            session_id,
            RealtimeSession {
                room_id,
                user_id: user.id,
                auth_hash: crate::api::token_hash(&token),
            },
        );
        let sessions = self.sessions.clone();
        let rooms = self.state.rooms.clone();
        let (output, input) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut events = connection.events;
            let mut revoked = connection.revoked;
            let subscribed = ServerEvent::RoomSubscribed {
                room_id: room_id.to_string(),
                can_publish: connection.can_publish,
                user_id: user.id,
                backend: connection.backend.to_owned(),
            };
            let initial =
                serde_json::to_string(&subscribed)
                    .ok()
                    .map(|event_json| RealtimeOutput {
                        session_id: session_id.to_string(),
                        payload: Some(realtime_output::Payload::EventJson(event_json)),
                    });
            if let Some(initial) = initial
                && output.send(Ok(initial)).await.is_ok()
            {
                loop {
                    let next = tokio::select! {
                        _ = revoked.recv() => None,
                        event = events.recv() => event.ok(),
                    };
                    let Some(message) = next else { break };
                    let payload = match message {
                        Message::Text(text) => {
                            realtime_output::Payload::EventJson(text.to_string())
                        }
                        Message::Binary(bytes) => realtime_output::Payload::Audio(bytes.to_vec()),
                        _ => continue,
                    };
                    if output
                        .send(Ok(RealtimeOutput {
                            session_id: session_id.to_string(),
                            payload: Some(payload),
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            sessions.write().await.remove(&session_id);
            rooms.disconnect(room_id, user.id).await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(input))))
    }

    async fn send_realtime(
        &self,
        request: Request<RealtimeInput>,
    ) -> Result<Response<RealtimeAck>, Status> {
        if !self.realtime_enabled {
            return Err(Status::unimplemented(
                "the admin service does not expose realtime room streams",
            ));
        }
        let auth_hash = auth_token(request.metadata())
            .map(|token| crate::api::token_hash(&token))
            .ok_or_else(|| Status::unauthenticated("请先登录"))?;
        let input = request.into_inner();
        let session_id = Uuid::parse_str(&input.session_id)
            .map_err(|_| Status::invalid_argument("实时会话 ID 无效"))?;
        let session = self
            .sessions
            .read()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| Status::not_found("实时会话已关闭"))?;
        if session.auth_hash != auth_hash {
            return Err(Status::permission_denied("实时会话与当前账号不匹配"));
        }
        let accepted = match input.payload {
            Some(realtime_input::Payload::EventJson(value)) => {
                let event = serde_json::from_str::<ClientEvent>(&value)
                    .map_err(|error| Status::invalid_argument(format!("事件格式无效: {error}")))?;
                self.state
                    .rooms
                    .send_event(session.room_id, session.user_id, event)
                    .await
            }
            Some(realtime_input::Payload::Audio(value)) => {
                self.state
                    .rooms
                    .send_audio(session.room_id, session.user_id, value)
                    .await
            }
            None => return Err(Status::invalid_argument("实时消息不能为空")),
        };
        Ok(Response::new(RealtimeAck { accepted }))
    }
}

fn auth_token(metadata: &tonic::metadata::MetadataMap) -> Option<String> {
    let cookies = metadata.get("cookie")?.to_str().ok()?;
    cookies.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == crate::api::AUTH_COOKIE).then(|| value.to_owned())
    })
}

fn internal(error: impl std::fmt::Display) -> Status {
    Status::internal(error.to_string())
}

fn is_transport_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "content-length" | "connection" | "transfer-encoding" | "set-cookie"
    )
}

pub(crate) fn router(state: AppState, routes: Router<AppState>, realtime_enabled: bool) -> Router {
    let api_router = Router::new()
        .route("/api/health", axum::routing::get(crate::health))
        .nest("/api", routes)
        .layer(tower_cookies::CookieManagerLayer::new())
        .with_state(state.clone());
    let service = ApiServiceServer::new(GrpcApi {
        router: api_router,
        state,
        sessions: Arc::default(),
        realtime_enabled,
    });
    tonic::service::Routes::new(tonic_web::GrpcWebLayer::new().layer(service)).into_axum_router()
}
