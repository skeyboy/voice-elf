use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::extract::ws::Message;
use tokio::{
    sync::{broadcast, mpsc},
    time::{self, Duration, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{
    backends::AppServices,
    media::MediaStore,
    pipeline::{PipelineIdentity, PipelineInput, run_pipeline},
    protocol::{
        ClientEvent, ClientVadEnd, ClientVadStart, RoomMemberState, ServerEvent, SessionConfig,
        SpeakerIdentity, VadEndReason,
    },
    storage::{Database, RoomMemberRecord, RoomRecord, UserRecord},
};

const ROOM_MESSAGE_CAPACITY: usize = 2_048;
const AUDIO_COMMAND_CAPACITY: usize = 512;
const MIX_FRAME_SAMPLES: usize = 512;
const MIX_FRAME_MS: u64 = 32;
const SEGMENT_CLOSE_TICKS: usize = 12;
const MAX_QUEUED_FRAMES_PER_SPEAKER: usize = 48;

#[derive(Clone, Default)]
pub(crate) struct RoomHub {
    rooms: Arc<Mutex<HashMap<Uuid, RoomRuntime>>>,
}

struct RoomRuntime {
    channel: broadcast::Sender<Message>,
    members: HashMap<Uuid, RuntimeMember>,
    audio_tx: Option<mpsc::Sender<RoomAudioCommand>>,
    backend: Option<&'static str>,
}

struct RuntimeMember {
    username: String,
    is_owner: bool,
    is_muted: bool,
    connections: usize,
    is_speaking: bool,
    disconnect: broadcast::Sender<()>,
}

pub(crate) struct RoomConnection {
    pub events: broadcast::Receiver<Message>,
    pub revoked: broadcast::Receiver<()>,
    pub can_publish: bool,
    pub backend: &'static str,
}

#[derive(Debug)]
enum RoomAudioCommand {
    Configure(SessionConfig),
    Start { speaker: SpeakerIdentity },
    Audio { user_id: Uuid, bytes: Vec<u8> },
    End { user_id: Uuid },
    RemoveSpeaker { user_id: Uuid },
    Shutdown,
}

struct SpeakerStream {
    identity: SpeakerIdentity,
    active: bool,
    frames: VecDeque<Vec<i16>>,
}

struct MixedSegment {
    id: Uuid,
    config: SessionConfig,
    started_at: Instant,
    sample_count: usize,
    idle_ticks: usize,
    speakers: HashMap<Uuid, SpeakerIdentity>,
}

impl RoomHub {
    pub(crate) fn connect(
        &self,
        room: &RoomRecord,
        user: &UserRecord,
        members: &[RoomMemberRecord],
        services: Arc<AppServices>,
        database: Option<Database>,
        media: MediaStore,
    ) -> RoomConnection {
        let mut rooms = self
            .rooms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runtime = rooms.entry(room.id).or_insert_with(|| RoomRuntime {
            channel: broadcast::channel(ROOM_MESSAGE_CAPACITY).0,
            members: HashMap::new(),
            audio_tx: None,
            backend: None,
        });
        for member in members {
            let current = runtime
                .members
                .entry(member.user_id)
                .or_insert_with(|| RuntimeMember {
                    username: member.username.clone(),
                    is_owner: member.is_owner,
                    is_muted: member.is_muted,
                    connections: 0,
                    is_speaking: false,
                    disconnect: broadcast::channel(8).0,
                });
            current.username.clone_from(&member.username);
            current.is_owner = member.is_owner;
            current.is_muted = member.is_muted;
        }
        let current = runtime
            .members
            .entry(user.id)
            .or_insert_with(|| RuntimeMember {
                username: user.username.clone(),
                is_owner: room.owner_id == user.id,
                is_muted: false,
                connections: 0,
                is_speaking: false,
                disconnect: broadcast::channel(8).0,
            });
        current.connections += 1;
        let can_publish = current.is_owner || !current.is_muted;
        let revoked = current.disconnect.subscribe();
        let events = runtime.channel.subscribe();

        if runtime.audio_tx.is_none() {
            runtime.backend = Some(services.backend_name);
            runtime.audio_tx = Some(self.start_audio_runtime(
                room,
                runtime.channel.clone(),
                services,
                database,
                media,
            ));
        }
        let runtime_backend = runtime
            .backend
            .expect("room runtime backend is set with its audio pipeline");
        drop(rooms);
        self.broadcast_members(room.id);
        RoomConnection {
            events,
            revoked,
            can_publish,
            backend: runtime_backend,
        }
    }

    pub(crate) async fn disconnect_user(&self, user_id: Uuid) {
        let affected = {
            let mut rooms = self
                .rooms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rooms
                .iter_mut()
                .filter_map(|(room_id, runtime)| {
                    let member = runtime.members.get_mut(&user_id)?;
                    member.connections = 0;
                    member.is_speaking = false;
                    let _ = member.disconnect.send(());
                    Some((*room_id, runtime.audio_tx.clone()))
                })
                .collect::<Vec<_>>()
        };
        for (room_id, audio_tx) in affected {
            if let Some(sender) = audio_tx {
                let _ = sender
                    .send(RoomAudioCommand::RemoveSpeaker { user_id })
                    .await;
            }
            self.broadcast_members(room_id);
        }
    }

    pub(crate) async fn close_room(&self, room_id: Uuid) {
        let runtime = self
            .rooms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&room_id);
        if let Some(sender) = runtime.and_then(|room| room.audio_tx) {
            let _ = sender.send(RoomAudioCommand::Shutdown).await;
        }
    }

    pub(crate) async fn disconnect(&self, room_id: Uuid, user_id: Uuid) {
        let (audio_tx, shutdown) = {
            let mut rooms = self
                .rooms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(runtime) = rooms.get_mut(&room_id) else {
                return;
            };
            let mut remove_speaker = false;
            if let Some(member) = runtime.members.get_mut(&user_id) {
                member.connections = member.connections.saturating_sub(1);
                if member.connections == 0 {
                    member.is_speaking = false;
                    remove_speaker = true;
                }
            }
            let shutdown = runtime
                .members
                .values()
                .all(|member| member.connections == 0);
            let audio_tx = if shutdown {
                runtime.audio_tx.take()
            } else if remove_speaker {
                runtime.audio_tx.clone()
            } else {
                None
            };
            (audio_tx, shutdown)
        };
        if let Some(sender) = audio_tx {
            let command = if shutdown {
                RoomAudioCommand::Shutdown
            } else {
                RoomAudioCommand::RemoveSpeaker { user_id }
            };
            let _ = sender.send(command).await;
        }
        self.broadcast_members(room_id);
    }

    pub(crate) fn member_states(
        &self,
        room_id: Uuid,
        members: &[RoomMemberRecord],
    ) -> Vec<RoomMemberState> {
        let rooms = self
            .rooms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runtime = rooms.get(&room_id);
        members
            .iter()
            .map(|member| {
                let live = runtime.and_then(|room| room.members.get(&member.user_id));
                RoomMemberState {
                    user_id: member.user_id,
                    username: member.username.clone(),
                    is_owner: member.is_owner,
                    is_muted: live.map_or(member.is_muted, |state| state.is_muted),
                    is_online: live.is_some_and(|state| state.connections > 0),
                    is_speaking: live.is_some_and(|state| state.is_speaking),
                }
            })
            .collect()
    }

    pub(crate) async fn set_muted(&self, room_id: Uuid, user_id: Uuid, is_muted: bool) {
        let audio_tx = {
            let mut rooms = self
                .rooms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(runtime) = rooms.get_mut(&room_id) else {
                return;
            };
            let Some(member) = runtime.members.get_mut(&user_id) else {
                return;
            };
            member.is_muted = is_muted;
            if is_muted {
                member.is_speaking = false;
            }
            runtime.audio_tx.clone()
        };
        if is_muted && let Some(sender) = audio_tx {
            let _ = sender
                .send(RoomAudioCommand::RemoveSpeaker { user_id })
                .await;
        }
        self.broadcast_members(room_id);
    }

    pub(crate) async fn send_event(
        &self,
        room_id: Uuid,
        user_id: Uuid,
        event: ClientEvent,
    ) -> bool {
        let (sender, member) = {
            let rooms = self
                .rooms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(runtime) = rooms.get(&room_id) else {
                return false;
            };
            let Some(member) = runtime.members.get(&user_id) else {
                return false;
            };
            if member.is_muted && !member.is_owner {
                return false;
            }
            (
                runtime.audio_tx.clone(),
                (member.username.clone(), member.is_owner),
            )
        };
        let Some(sender) = sender else {
            return false;
        };
        let command = match event {
            ClientEvent::Configure(config) if member.1 => RoomAudioCommand::Configure(config),
            ClientEvent::Configure(_) => return true,
            ClientEvent::Start { .. } => RoomAudioCommand::Start {
                speaker: SpeakerIdentity {
                    user_id: Some(user_id),
                    username: member.0,
                },
            },
            ClientEvent::End { .. } | ClientEvent::Flush => RoomAudioCommand::End { user_id },
        };
        sender.send(command).await.is_ok()
    }

    pub(crate) async fn send_audio(&self, room_id: Uuid, user_id: Uuid, bytes: Vec<u8>) -> bool {
        let sender = {
            let rooms = self
                .rooms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(runtime) = rooms.get(&room_id) else {
                return false;
            };
            let Some(member) = runtime.members.get(&user_id) else {
                return false;
            };
            if member.is_muted && !member.is_owner {
                return false;
            }
            runtime.audio_tx.clone()
        };
        let Some(sender) = sender else {
            return false;
        };
        sender
            .send(RoomAudioCommand::Audio { user_id, bytes })
            .await
            .is_ok()
    }

    fn start_audio_runtime(
        &self,
        room: &RoomRecord,
        publisher: broadcast::Sender<Message>,
        services: Arc<AppServices>,
        database: Option<Database>,
        media: MediaStore,
    ) -> mpsc::Sender<RoomAudioCommand> {
        let (pipeline_output_tx, mut pipeline_output_rx) = mpsc::channel::<Message>(256);
        let (pipeline_input_tx, pipeline_input_rx) = mpsc::channel::<PipelineInput>(256);
        let (audio_tx, audio_rx) = mpsc::channel(AUDIO_COMMAND_CAPACITY);
        let room_id = room.id;
        let identity = PipelineIdentity {
            user_id: room.owner_id,
            room_id,
        };
        let forward_publisher = publisher.clone();
        tokio::spawn(async move {
            while let Some(message) = pipeline_output_rx.recv().await {
                if matches!(message, Message::Text(_) | Message::Binary(_)) {
                    let _ = forward_publisher.send(message);
                }
            }
        });
        tokio::spawn(async move {
            match run_pipeline(
                services,
                database,
                media,
                identity,
                pipeline_input_rx,
                pipeline_output_tx,
            )
            .await
            {
                Ok(()) => tracing::info!(%room_id, "room audio pipeline stopped"),
                Err(error) => tracing::warn!(%error, %room_id, "room audio pipeline failed"),
            }
        });
        let hub = self.clone();
        let mut config = SessionConfig::default();
        config.source_language.clone_from(&room.source_language);
        config.target_language.clone_from(&room.target_language);
        config.max_utterance_seconds = room.max_utterance_seconds as u32;
        tokio::spawn(run_room_mixer(
            hub,
            room_id,
            config,
            audio_rx,
            pipeline_input_tx,
        ));
        audio_tx
    }

    fn set_speaking(&self, room_id: Uuid, user_id: Uuid, speaking: bool) {
        let changed = {
            let mut rooms = self
                .rooms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(member) = rooms
                .get_mut(&room_id)
                .and_then(|room| room.members.get_mut(&user_id))
            else {
                return;
            };
            if member.is_speaking == speaking {
                false
            } else {
                member.is_speaking = speaking;
                true
            }
        };
        if changed {
            self.broadcast_members(room_id);
        }
    }

    fn broadcast_members(&self, room_id: Uuid) {
        let (publisher, members) = {
            let rooms = self
                .rooms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(runtime) = rooms.get(&room_id) else {
                return;
            };
            let mut members = runtime
                .members
                .iter()
                .map(|(user_id, member)| RoomMemberState {
                    user_id: *user_id,
                    username: member.username.clone(),
                    is_owner: member.is_owner,
                    is_muted: member.is_muted,
                    is_online: member.connections > 0,
                    is_speaking: member.is_speaking,
                })
                .collect::<Vec<_>>();
            members.sort_by_key(|member| (!member.is_owner, member.username.to_lowercase()));
            (runtime.channel.clone(), members)
        };
        let event = ServerEvent::RoomMembers { members };
        if let Ok(text) = serde_json::to_string(&event) {
            let _ = publisher.send(Message::Text(text.into()));
        }
    }
}

async fn run_room_mixer(
    hub: RoomHub,
    room_id: Uuid,
    mut config: SessionConfig,
    mut commands: mpsc::Receiver<RoomAudioCommand>,
    pipeline: mpsc::Sender<PipelineInput>,
) {
    let mut speakers = HashMap::<Uuid, SpeakerStream>::new();
    let mut segment: Option<MixedSegment> = None;
    let mut ticker = time::interval(Duration::from_millis(MIX_FRAME_MS));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    RoomAudioCommand::Configure(next) => {
                        config = next.clone();
                        if pipeline.send(PipelineInput::Event(ClientEvent::Configure(next))).await.is_err() {
                            break;
                        }
                    }
                    RoomAudioCommand::Start { speaker } => {
                        let Some(user_id) = speaker.user_id else { continue; };
                        let stream = speakers.entry(user_id).or_insert_with(|| SpeakerStream {
                            identity: speaker.clone(),
                            active: false,
                            frames: VecDeque::new(),
                        });
                        stream.identity = speaker;
                        stream.active = true;
                        hub.set_speaking(room_id, user_id, true);
                    }
                    RoomAudioCommand::Audio { user_id, bytes } => {
                        let Some(frame) = decode_frame(&bytes) else { continue; };
                        if let Some(stream) = speakers.get_mut(&user_id)
                            && stream.active
                            && stream.frames.len() < MAX_QUEUED_FRAMES_PER_SPEAKER
                        {
                            stream.frames.push_back(frame);
                        }
                    }
                    RoomAudioCommand::End { user_id } => {
                        if let Some(stream) = speakers.get_mut(&user_id) {
                            stream.active = false;
                        }
                        hub.set_speaking(room_id, user_id, false);
                    }
                    RoomAudioCommand::RemoveSpeaker { user_id } => {
                        speakers.remove(&user_id);
                        hub.set_speaking(room_id, user_id, false);
                    }
                    RoomAudioCommand::Shutdown => break,
                }
            }
            _ = ticker.tick() => {
                let mut frames = Vec::new();
                for (user_id, speaker) in &mut speakers {
                    if let Some(frame) = speaker.frames.pop_front() {
                        frames.push((*user_id, speaker.identity.clone(), frame));
                    }
                }
                if !frames.is_empty() {
                    if segment.is_none() {
                        let id = Uuid::new_v4();
                        let start = ClientEvent::Start {
                            tc_id: id,
                            vad: Some(ClientVadStart {
                                engine: "room-pcm-mixer".to_owned(),
                                sample_rate: 16_000,
                                frame_samples: MIX_FRAME_SAMPLES,
                                pre_roll_samples: 0,
                            }),
                            config: config.clone(),
                        };
                        if pipeline.send(PipelineInput::Event(start)).await.is_err() {
                            break;
                        }
                        segment = Some(MixedSegment {
                            id,
                            config: config.clone(),
                            started_at: Instant::now(),
                            sample_count: 0,
                            idle_ticks: 0,
                            speakers: HashMap::new(),
                        });
                    }
                    let current = segment.as_mut().expect("mixed segment must exist");
                    let previous_count = current.speakers.len();
                    for (user_id, identity, _) in &frames {
                        current.speakers.insert(*user_id, identity.clone());
                    }
                    if current.speakers.len() != previous_count {
                        let mut participants = current.speakers.values().cloned().collect::<Vec<_>>();
                        participants.sort_by(|left, right| left.username.cmp(&right.username));
                        if pipeline.send(PipelineInput::Speakers {
                            utterance_id: current.id,
                            speakers: participants,
                        }).await.is_err() {
                            break;
                        }
                    }
                    let mixed = mix_frames(&frames);
                    current.sample_count += mixed.len();
                    current.idle_ticks = 0;
                    if pipeline.send(PipelineInput::Audio(encode_frame(&mixed))).await.is_err() {
                        break;
                    }
                } else if let Some(current) = segment.as_mut() {
                    if speakers.values().any(|speaker| speaker.active || !speaker.frames.is_empty()) {
                        current.idle_ticks = 0;
                    } else {
                        current.idle_ticks += 1;
                    }
                }

                let should_close = segment.as_ref().is_some_and(|current| {
                    current.idle_ticks >= SEGMENT_CLOSE_TICKS
                        || current.started_at.elapsed()
                            >= Duration::from_secs(current.config.max_utterance_seconds as u64)
                });
                if should_close {
                    let reason = if segment.as_ref().is_some_and(|current| {
                        current.started_at.elapsed()
                            >= Duration::from_secs(current.config.max_utterance_seconds as u64)
                    }) {
                        VadEndReason::MaxDuration
                    } else {
                        VadEndReason::Silence
                    };
                    if !finish_mixed_segment(&pipeline, segment.take().unwrap(), reason).await {
                        break;
                    }
                }
            }
        }
    }

    for user_id in speakers.keys().copied().collect::<Vec<_>>() {
        hub.set_speaking(room_id, user_id, false);
    }
    if let Some(segment) = segment {
        let _ = finish_mixed_segment(&pipeline, segment, VadEndReason::Manual).await;
    }
}

async fn finish_mixed_segment(
    pipeline: &mpsc::Sender<PipelineInput>,
    segment: MixedSegment,
    reason: VadEndReason,
) -> bool {
    pipeline
        .send(PipelineInput::Event(ClientEvent::End {
            tc_id: segment.id,
            is_silent_vad: segment.sample_count < MIX_FRAME_SAMPLES * 6,
            vad: Some(ClientVadEnd {
                reason,
                sample_count: segment.sample_count,
                speech_frames: Some(segment.sample_count / MIX_FRAME_SAMPLES),
            }),
        }))
        .await
        .is_ok()
}

fn decode_frame(bytes: &[u8]) -> Option<Vec<i16>> {
    if bytes.len() != MIX_FRAME_SAMPLES * size_of::<i16>() {
        return None;
    }
    Some(
        bytes
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect(),
    )
}

fn encode_frame(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

fn mix_frames(frames: &[(Uuid, SpeakerIdentity, Vec<i16>)]) -> Vec<i16> {
    let divisor = frames.len().max(1) as i32;
    (0..MIX_FRAME_SAMPLES)
        .map(|index| {
            let sum = frames
                .iter()
                .map(|(_, _, frame)| frame[index] as i32)
                .sum::<i32>();
            (sum / divisor).clamp(i16::MIN as i32, i16::MAX as i32) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_simultaneous_frames_without_clipping() {
        let speaker = SpeakerIdentity {
            user_id: Some(Uuid::new_v4()),
            username: "speaker".to_owned(),
        };
        let frames = vec![
            (
                Uuid::new_v4(),
                speaker.clone(),
                vec![30_000; MIX_FRAME_SAMPLES],
            ),
            (Uuid::new_v4(), speaker, vec![10_000; MIX_FRAME_SAMPLES]),
        ];
        let mixed = mix_frames(&frames);
        assert_eq!(mixed.len(), MIX_FRAME_SAMPLES);
        assert!(mixed.iter().all(|sample| *sample == 20_000));
    }

    #[test]
    fn rejects_non_vad_pcm_frames() {
        assert!(decode_frame(&vec![0; MIX_FRAME_SAMPLES * 2]).is_some());
        assert!(decode_frame(&vec![0; MIX_FRAME_SAMPLES]).is_none());
    }
}
