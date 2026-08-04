mod config;
mod events;
mod jobs;
mod latency;
mod session;
mod synthesis;
mod transcription;
mod translation;

use std::sync::Arc;

use axum::extract::ws::Message;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{backends::AppServices, media::MediaStore, storage::Database};

pub use session::run_pipeline;

#[derive(Debug)]
pub enum PipelineInput {
    Event(crate::protocol::ClientEvent),
    Audio(Vec<u8>),
    Invalid(String),
    Ping(Vec<u8>),
}

#[derive(Clone, Copy, Debug)]
pub struct PipelineIdentity {
    pub user_id: Uuid,
    pub room_id: Uuid,
}

#[derive(Clone)]
struct PipelineContext {
    services: Arc<AppServices>,
    database: Option<Database>,
    media: MediaStore,
    session_id: Uuid,
    user_id: Uuid,
    room_id: Uuid,
    output: mpsc::Sender<Message>,
}
