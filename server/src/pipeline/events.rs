use anyhow::{Context, Result};
use axum::extract::ws::Message;
use tokio::sync::mpsc;

use crate::protocol::{PipelinePhase, ServerEvent};

pub(super) async fn send_state(
    output: &mpsc::Sender<Message>,
    phase: PipelinePhase,
    utterance_id: Option<&str>,
) -> Result<()> {
    send_event(
        output,
        ServerEvent::State {
            phase,
            utterance_id: utterance_id.map(str::to_owned),
        },
    )
    .await
}

pub(super) async fn send_event(output: &mpsc::Sender<Message>, event: ServerEvent) -> Result<()> {
    let json = serde_json::to_string(&event)?;
    output
        .send(Message::Text(json.into()))
        .await
        .context("WebSocket writer closed")
}
