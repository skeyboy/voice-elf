use std::{
    collections::{HashSet, VecDeque},
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::Stream;
use serde::Serialize;
use tokio::sync::{Notify, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    Request, Response, Status,
    metadata::MetadataValue,
    transport::{Channel, Endpoint, Server},
};
use uuid::Uuid;

use crate::{AppState, room_hub::RoomHub};

pub(crate) mod proto {
    tonic::include_proto!("voiceelf.control.v1");
}

use proto::{
    CloseRoom, DependencyCheck, ReadinessRequest, ReadinessResponse, RevokeUserSessions,
    RuntimeCommand, RuntimeCommandStreamRequest, RuntimeSnapshotRequest, RuntimeSnapshotResponse,
    control_plane_client::ControlPlaneClient as GrpcControlPlaneClient,
    control_plane_server::{ControlPlane, ControlPlaneServer},
    runtime_command,
};

const CONTROL_TOKEN_HEADER: &str = "x-voice-elf-control-token";
const COMMAND_QUEUE_CAPACITY: usize = 2_048;
const PROCESSED_COMMAND_CAPACITY: usize = 4_096;

#[derive(Clone)]
pub(crate) struct RuntimeCommandBus {
    inner: Arc<RuntimeCommandBusInner>,
}

struct RuntimeCommandBusInner {
    next_sequence: AtomicU64,
    commands: Mutex<VecDeque<RuntimeCommand>>,
    changed: Notify,
    active_subscribers: AtomicU64,
    last_subscriber: Mutex<Option<String>>,
}

impl Default for RuntimeCommandBus {
    fn default() -> Self {
        Self {
            inner: Arc::new(RuntimeCommandBusInner {
                next_sequence: AtomicU64::new(1),
                commands: Mutex::new(VecDeque::with_capacity(COMMAND_QUEUE_CAPACITY)),
                changed: Notify::new(),
                active_subscribers: AtomicU64::new(0),
                last_subscriber: Mutex::new(None),
            }),
        }
    }
}

impl RuntimeCommandBus {
    pub(crate) fn revoke_user_sessions(&self, user_id: Uuid) -> String {
        self.publish(runtime_command::Action::RevokeUserSessions(
            RevokeUserSessions {
                user_id: user_id.to_string(),
            },
        ))
    }

    pub(crate) fn close_room(&self, room_id: Uuid) -> String {
        self.publish(runtime_command::Action::CloseRoom(CloseRoom {
            room_id: room_id.to_string(),
        }))
    }

    fn publish(&self, action: runtime_command::Action) -> String {
        let command_id = Uuid::new_v4().to_string();
        let command = RuntimeCommand {
            command_id: command_id.clone(),
            sequence: self.inner.next_sequence.fetch_add(1, Ordering::Relaxed),
            issued_at: Utc::now().to_rfc3339(),
            action: Some(action),
        };
        let mut commands = self
            .inner
            .commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if commands.len() == COMMAND_QUEUE_CAPACITY {
            commands.pop_front();
        }
        commands.push_back(command);
        drop(commands);
        self.inner.changed.notify_waiters();
        command_id
    }

    fn commands_after(&self, sequence: u64) -> Vec<RuntimeCommand> {
        self.inner
            .commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|command| command.sequence > sequence)
            .cloned()
            .collect()
    }

    fn subscribe(&self, caller: &str) -> RuntimeCommandSubscriber {
        self.inner
            .active_subscribers
            .fetch_add(1, Ordering::Relaxed);
        *self
            .inner
            .last_subscriber
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(format!(
            "{} at {}",
            if caller.trim().is_empty() {
                "unknown"
            } else {
                caller
            },
            Utc::now().to_rfc3339()
        ));
        RuntimeCommandSubscriber { bus: self.clone() }
    }

    fn subscriber_status(&self) -> (u64, Option<String>) {
        (
            self.inner.active_subscribers.load(Ordering::Relaxed),
            self.inner
                .last_subscriber
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }

    async fn stream_after(
        &self,
        mut sequence: u64,
        sender: mpsc::Sender<Result<RuntimeCommand, Status>>,
    ) {
        let latest_sequence = self
            .inner
            .next_sequence
            .load(Ordering::Relaxed)
            .saturating_sub(1);
        if sequence > latest_sequence {
            sequence = 0;
        }
        loop {
            let changed = self.inner.changed.notified();
            let commands = self.commands_after(sequence);
            if commands.is_empty() {
                changed.await;
                continue;
            }
            for command in commands {
                sequence = command.sequence;
                if sender.send(Ok(command)).await.is_err() {
                    return;
                }
            }
        }
    }
}

struct RuntimeCommandSubscriber {
    bus: RuntimeCommandBus,
}

impl Drop for RuntimeCommandSubscriber {
    fn drop(&mut self) {
        self.bus
            .inner
            .active_subscribers
            .fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DependencyView {
    pub name: String,
    pub kind: String,
    pub required: bool,
    pub status: String,
    pub message: String,
    pub checked_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeSnapshotView {
    pub service: String,
    pub overall_status: String,
    pub generated_at: String,
    pub initialized: bool,
    pub authorized: bool,
    pub dependencies: Vec<DependencyView>,
    pub version: String,
}

impl From<DependencyCheck> for DependencyView {
    fn from(value: DependencyCheck) -> Self {
        Self {
            name: value.name,
            kind: value.kind,
            required: value.required,
            status: value.status,
            message: value.message,
            checked_at: value.checked_at,
        }
    }
}

impl From<RuntimeSnapshotResponse> for RuntimeSnapshotView {
    fn from(value: RuntimeSnapshotResponse) -> Self {
        Self {
            service: value.service,
            overall_status: value.overall_status,
            generated_at: value.generated_at,
            initialized: value.initialized,
            authorized: value.authorized,
            dependencies: value.dependencies.into_iter().map(Into::into).collect(),
            version: value.version,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ControlPlaneClient {
    endpoint: Option<Arc<str>>,
    channel: Option<Channel>,
    token: Option<Arc<str>>,
}

impl ControlPlaneClient {
    pub(crate) fn disabled() -> Self {
        Self {
            endpoint: None,
            channel: None,
            token: None,
        }
    }

    pub(crate) fn from_env() -> Result<Self> {
        let endpoint = std::env::var("VOICE_ELF_ADMIN_GRPC_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:50051".to_owned());
        let channel = Endpoint::from_shared(endpoint.clone())
            .context("VOICE_ELF_ADMIN_GRPC_URL must be a valid tonic endpoint")?
            .connect_lazy();
        let token = control_token_from_env()?;
        Ok(Self {
            endpoint: Some(Arc::from(endpoint)),
            channel: Some(channel),
            token: token.map(Arc::from),
        })
    }

    pub(crate) async fn runtime_snapshot(&self) -> RuntimeSnapshotView {
        let checked_at = Utc::now().to_rfc3339();
        let Some(_) = &self.endpoint else {
            return RuntimeSnapshotView {
                service: "embedded".to_owned(),
                overall_status: "ready".to_owned(),
                generated_at: checked_at.clone(),
                initialized: true,
                authorized: true,
                dependencies: vec![DependencyView {
                    name: "admin_control_plane".to_owned(),
                    kind: "grpc".to_owned(),
                    required: false,
                    status: "ready".to_owned(),
                    message: "兼容单体模式使用进程内控制面".to_owned(),
                    checked_at,
                }],
                version: env!("CARGO_PKG_VERSION").to_owned(),
            };
        };

        match self.fetch_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => RuntimeSnapshotView {
                service: "voice-elf-admin".to_owned(),
                overall_status: "unavailable".to_owned(),
                generated_at: checked_at.clone(),
                initialized: false,
                authorized: false,
                dependencies: vec![DependencyView {
                    name: "admin_control_plane".to_owned(),
                    kind: "grpc".to_owned(),
                    required: true,
                    status: "unavailable".to_owned(),
                    message: format!("无法连接管理控制面: {error}"),
                    checked_at,
                }],
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        }
    }

    async fn fetch_snapshot(&self) -> Result<RuntimeSnapshotView> {
        let mut client = self.grpc_client()?;
        let mut request = Request::new(RuntimeSnapshotRequest {
            caller: "voice-elf-public".to_owned(),
        });
        if let Some(token) = &self.token {
            request.metadata_mut().insert(
                CONTROL_TOKEN_HEADER,
                MetadataValue::try_from(token.as_ref()).context("invalid control token")?,
            );
        }
        Ok(client
            .get_runtime_snapshot(request)
            .await
            .context("control plane request failed")?
            .into_inner()
            .into())
    }

    fn grpc_client(&self) -> Result<GrpcControlPlaneClient<Channel>> {
        self.channel
            .clone()
            .map(GrpcControlPlaneClient::new)
            .context("control plane channel is disabled")
    }

    pub(crate) async fn run_command_listener(&self, rooms: RoomHub) -> Result<()> {
        let Some(endpoint) = &self.endpoint else {
            return Ok(());
        };
        let mut after_sequence = 0;
        let mut processed = ProcessedCommands::default();
        let mut retry_delay = Duration::from_secs(1);
        loop {
            let result = self
                .consume_command_stream(endpoint, &rooms, &mut after_sequence, &mut processed)
                .await;
            match result {
                Ok(()) => tracing::warn!("admin runtime command stream closed; reconnecting"),
                Err(error) => {
                    tracing::warn!(%error, "admin runtime command stream unavailable; reconnecting")
                }
            }
            tokio::time::sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
        }
    }

    async fn consume_command_stream(
        &self,
        endpoint: &str,
        rooms: &RoomHub,
        after_sequence: &mut u64,
        processed: &mut ProcessedCommands,
    ) -> Result<()> {
        let mut client = self.grpc_client()?;
        let mut request = Request::new(RuntimeCommandStreamRequest {
            caller: "voice-elf-public".to_owned(),
            after_sequence: *after_sequence,
        });
        if let Some(token) = &self.token {
            request.metadata_mut().insert(
                CONTROL_TOKEN_HEADER,
                MetadataValue::try_from(token.as_ref()).context("invalid control token")?,
            );
        }
        let mut stream = client
            .watch_runtime_commands(request)
            .await
            .context("runtime command stream request failed")?
            .into_inner();
        tracing::info!(
            endpoint,
            after_sequence = *after_sequence,
            "runtime command stream connected"
        );
        while let Some(command) = stream
            .message()
            .await
            .context("runtime command stream failed")?
        {
            *after_sequence = (*after_sequence).max(command.sequence);
            if processed.contains(&command.command_id) {
                continue;
            }
            let command_id = command.command_id.clone();
            match apply_runtime_command(rooms, command).await {
                Ok(action) => {
                    processed.insert(command_id.clone());
                    tracing::info!(%command_id, action, "runtime command applied");
                }
                Err(error) => {
                    tracing::error!(%command_id, %error, "invalid runtime command skipped");
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct ProcessedCommands {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl ProcessedCommands {
    fn contains(&self, command_id: &str) -> bool {
        self.ids.contains(command_id)
    }

    fn insert(&mut self, command_id: String) {
        if !self.ids.insert(command_id.clone()) {
            return;
        }
        self.order.push_back(command_id);
        if self.order.len() > PROCESSED_COMMAND_CAPACITY
            && let Some(expired) = self.order.pop_front()
        {
            self.ids.remove(&expired);
        }
    }
}

async fn apply_runtime_command(rooms: &RoomHub, command: RuntimeCommand) -> Result<&'static str> {
    match command.action.context("runtime command has no action")? {
        runtime_command::Action::RevokeUserSessions(action) => {
            let user_id = Uuid::parse_str(&action.user_id).context("invalid user_id")?;
            rooms.disconnect_user(user_id).await;
            Ok("revoke_user_sessions")
        }
        runtime_command::Action::CloseRoom(action) => {
            let room_id = Uuid::parse_str(&action.room_id).context("invalid room_id")?;
            rooms.close_room(room_id).await;
            Ok("close_room")
        }
    }
}

#[derive(Clone)]
struct ControlPlaneService {
    state: AppState,
    token: Option<Arc<str>>,
}

impl ControlPlaneService {
    fn authorize<T>(&self, request: &Request<T>) -> Result<(), Status> {
        let Some(expected) = &self.token else {
            return Ok(());
        };
        let actual = request
            .metadata()
            .get(CONTROL_TOKEN_HEADER)
            .and_then(|value| value.to_str().ok());
        if actual == Some(expected.as_ref()) {
            Ok(())
        } else {
            Err(Status::unauthenticated("invalid control plane credential"))
        }
    }
}

#[tonic::async_trait]
impl ControlPlane for ControlPlaneService {
    type WatchRuntimeCommandsStream =
        Pin<Box<dyn Stream<Item = Result<RuntimeCommand, Status>> + Send + 'static>>;

    async fn get_runtime_snapshot(
        &self,
        request: Request<RuntimeSnapshotRequest>,
    ) -> Result<Response<RuntimeSnapshotResponse>, Status> {
        self.authorize(&request)?;
        Ok(Response::new(build_admin_snapshot(&self.state).await))
    }

    async fn check_readiness(
        &self,
        request: Request<ReadinessRequest>,
    ) -> Result<Response<ReadinessResponse>, Status> {
        self.authorize(&request)?;
        let snapshot = build_admin_snapshot(&self.state).await;
        Ok(Response::new(ReadinessResponse {
            ready: snapshot.overall_status == "ready",
            snapshot: Some(snapshot),
        }))
    }

    async fn watch_runtime_commands(
        &self,
        request: Request<RuntimeCommandStreamRequest>,
    ) -> Result<Response<Self::WatchRuntimeCommandsStream>, Status> {
        self.authorize(&request)?;
        let request = request.into_inner();
        let (sender, receiver) = mpsc::channel(64);
        let commands = self.state.commands.clone();
        let subscriber = commands.subscribe(&request.caller);
        tokio::spawn(async move {
            let _subscriber = subscriber;
            commands.stream_after(request.after_sequence, sender).await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
    }
}

pub(crate) async fn serve(bind: SocketAddr, state: AppState) -> Result<()> {
    let token = control_token_from_env()?.map(Arc::from);
    if !bind.ip().is_loopback() && token.is_none() {
        anyhow::bail!("VOICE_ELF_CONTROL_TOKEN is required for a non-loopback gRPC bind");
    }
    tracing::info!(address = %bind, authenticated = token.is_some(), "admin gRPC control plane listening");
    Server::builder()
        .add_service(ControlPlaneServer::new(ControlPlaneService {
            state,
            token,
        }))
        .serve(bind)
        .await?;
    Ok(())
}

pub(crate) fn admin_grpc_bind_from_env() -> Result<SocketAddr> {
    std::env::var("VOICE_ELF_ADMIN_GRPC_BIND")
        .unwrap_or_else(|_| "127.0.0.1:50051".to_owned())
        .parse()
        .context("VOICE_ELF_ADMIN_GRPC_BIND must be a socket address")
}

pub(crate) fn admin_http_bind_from_env() -> Result<SocketAddr> {
    std::env::var("VOICE_ELF_ADMIN_BIND")
        .unwrap_or_else(|_| "127.0.0.1:3002".to_owned())
        .parse()
        .context("VOICE_ELF_ADMIN_BIND must be a socket address")
}

fn control_token_from_env() -> Result<Option<String>> {
    let token = std::env::var("VOICE_ELF_CONTROL_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if token.as_ref().is_some_and(|value| value.len() < 32) {
        anyhow::bail!("VOICE_ELF_CONTROL_TOKEN must contain at least 32 bytes");
    }
    Ok(token)
}

async fn build_admin_snapshot(state: &AppState) -> RuntimeSnapshotResponse {
    let checked_at = Utc::now().to_rfc3339();
    let (database_status, database_message, initialized) = match &state.database {
        Some(database) => match database.system_installation().await {
            Ok(profile) => (
                "ready",
                "PostgreSQL 连接和管理 schema 可用".to_owned(),
                profile.is_some(),
            ),
            Err(error) => (
                "unavailable",
                format!("PostgreSQL 检测失败: {error}"),
                false,
            ),
        },
        None => ("unavailable", "未配置 DATABASE_URL".to_owned(), false),
    };
    let authorization = state.authority.snapshot().await;
    let asr = state.asr.effective_selection().await;
    let fun_asr = state.asr.fun_asr_runtime_status().await;
    let tts = state.tts.effective_selection().await;
    let qwen_tts = state.tts.qwen_runtime_status().await;
    let (asr_status, asr_message) = match asr {
        Ok(selection) if !state.asr.provider_available(&selection.backend_id) => (
            "unavailable",
            format!(
                "生效 ASR provider 未在当前实例配置: {}",
                selection.backend_id
            ),
        ),
        Ok(selection)
            if selection.backend_id == crate::backends::FUN_ASR_ID && !fun_asr.healthy =>
        {
            (
                "unavailable",
                format!(
                    "生效 provider funasr-streaming 不可连接: {}",
                    fun_asr.message
                ),
            )
        }
        Ok(selection) => ("ready", format!("生效 provider: {}", selection.backend_id)),
        Err(error) => ("unavailable", format!("ASR 配置不可用: {error}")),
    };
    let mut dependencies = vec![
        dependency(
            "postgresql",
            "database",
            true,
            database_status,
            database_message,
            &checked_at,
        ),
        dependency(
            "system_installation",
            "deployment",
            true,
            if initialized { "ready" } else { "unavailable" },
            if initialized {
                "系统已完成初始化"
            } else {
                "系统尚未初始化，请访问 admin /setup"
            },
            &checked_at,
        ),
        dependency(
            "instance_authorization",
            "authorization",
            true,
            if authorization.allowed {
                "ready"
            } else {
                "unavailable"
            },
            authorization.message.clone(),
            &checked_at,
        ),
        dependency(
            "asr_provider",
            "runtime",
            true,
            asr_status,
            asr_message,
            &checked_at,
        ),
        dependency(
            "tts_provider",
            "runtime",
            true,
            if tts.is_ok() { "ready" } else { "unavailable" },
            tts.map(|value| format!("生效 provider: {}", value.backend_id))
                .unwrap_or_else(|error| format!("TTS 配置不可用: {error}")),
            &checked_at,
        ),
    ];
    let (active_command_streams, last_command_stream) = state.commands.subscriber_status();
    dependencies.push(dependency(
        "public_command_stream",
        "grpc",
        false,
        if active_command_streams > 0 {
            "ready"
        } else {
            "degraded"
        },
        match (active_command_streams, last_command_stream) {
            (active, _) if active > 0 => format!("{active} 个 public 实例已连接运行时命令流"),
            (_, Some(last)) => format!("当前无 public 命令流连接；最近连接: {last}"),
            _ => "尚无 public 实例连接运行时命令流".to_owned(),
        },
        &checked_at,
    ));
    dependencies.push(dependency(
        "smtp",
        "optional",
        false,
        if state.mail.configured() {
            "ready"
        } else {
            "degraded"
        },
        if state.mail.configured() {
            "SMTP 已配置"
        } else {
            "SMTP 未完整配置，密码重置邮件不可用"
        },
        &checked_at,
    ));
    if qwen_tts.enabled {
        dependencies.push(dependency(
            "qwen_tts",
            "runtime",
            false,
            if qwen_tts.healthy {
                "ready"
            } else {
                "unavailable"
            },
            qwen_tts.message,
            &checked_at,
        ));
    }
    if fun_asr.enabled {
        dependencies.push(dependency(
            "funasr_streaming",
            "runtime",
            false,
            if fun_asr.healthy {
                "ready"
            } else {
                "unavailable"
            },
            fun_asr.message,
            &checked_at,
        ));
    }
    let overall_status = if dependencies
        .iter()
        .any(|check| check.required && check.status != "ready")
    {
        "unavailable"
    } else if dependencies.iter().any(|check| check.status == "degraded") {
        "degraded"
    } else {
        "ready"
    };
    RuntimeSnapshotResponse {
        service: "voice-elf-admin".to_owned(),
        overall_status: overall_status.to_owned(),
        generated_at: checked_at,
        initialized,
        authorized: authorization.allowed,
        dependencies,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

pub(crate) async fn admin_snapshot_view(state: &AppState) -> RuntimeSnapshotView {
    build_admin_snapshot(state).await.into()
}

fn dependency(
    name: impl Into<String>,
    kind: impl Into<String>,
    required: bool,
    status: impl Into<String>,
    message: impl Into<String>,
    checked_at: &str,
) -> DependencyCheck {
    DependencyCheck {
        name: name.into(),
        kind: kind.into(),
        required,
        status: status.into(),
        message: message.into(),
        checked_at: checked_at.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runtime_command_bus_streams_ordered_commands() {
        let bus = RuntimeCommandBus::default();
        let user_id = Uuid::new_v4();
        let room_id = Uuid::new_v4();
        let revoke_id = bus.revoke_user_sessions(user_id);
        let close_id = bus.close_room(room_id);
        let (sender, mut receiver) = mpsc::channel(4);
        let stream = tokio::spawn({
            let bus = bus.clone();
            async move { bus.stream_after(0, sender).await }
        });

        let revoke = receiver.recv().await.unwrap().unwrap();
        let close = receiver.recv().await.unwrap().unwrap();
        assert_eq!(revoke.sequence, 1);
        assert_eq!(revoke.command_id, revoke_id);
        assert!(matches!(
            revoke.action,
            Some(runtime_command::Action::RevokeUserSessions(action))
                if action.user_id == user_id.to_string()
        ));
        assert_eq!(close.sequence, 2);
        assert_eq!(close.command_id, close_id);
        assert!(matches!(
            close.action,
            Some(runtime_command::Action::CloseRoom(action))
                if action.room_id == room_id.to_string()
        ));
        stream.abort();
    }

    #[tokio::test]
    async fn runtime_command_stream_recovers_after_admin_sequence_reset() {
        let bus = RuntimeCommandBus::default();
        bus.close_room(Uuid::new_v4());
        let (sender, mut receiver) = mpsc::channel(2);
        let stream = tokio::spawn({
            let bus = bus.clone();
            async move { bus.stream_after(99, sender).await }
        });

        let command = receiver.recv().await.unwrap().unwrap();
        assert_eq!(command.sequence, 1);
        stream.abort();
    }

    #[test]
    fn processed_commands_deduplicates_command_ids() {
        let mut processed = ProcessedCommands::default();
        processed.insert("command-1".to_owned());
        processed.insert("command-1".to_owned());
        assert!(processed.contains("command-1"));
        assert_eq!(processed.ids.len(), 1);
        assert_eq!(processed.order.len(), 1);
    }

    #[test]
    fn runtime_command_bus_tracks_active_subscribers() {
        let bus = RuntimeCommandBus::default();
        let subscriber = bus.subscribe("voice-elf-public-test");
        let (active, last) = bus.subscriber_status();
        assert_eq!(active, 1);
        assert!(last.unwrap().contains("voice-elf-public-test"));
        drop(subscriber);
        assert_eq!(bus.subscriber_status().0, 0);
    }
}
