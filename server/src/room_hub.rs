use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::extract::ws::Message;
use tokio::sync::broadcast;
use uuid::Uuid;

const ROOM_MESSAGE_CAPACITY: usize = 2_048;

#[derive(Clone, Default)]
pub(crate) struct RoomHub {
    channels: Arc<Mutex<HashMap<Uuid, broadcast::Sender<Message>>>>,
}

impl RoomHub {
    pub(crate) fn subscribe(&self, room_id: Uuid) -> broadcast::Receiver<Message> {
        self.channel(room_id).subscribe()
    }

    pub(crate) fn publisher(&self, room_id: Uuid) -> broadcast::Sender<Message> {
        self.channel(room_id)
    }

    fn channel(&self, room_id: Uuid) -> broadcast::Sender<Message> {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        channels.retain(|_, sender| sender.receiver_count() > 0 || sender.strong_count() > 1);
        channels
            .entry(room_id)
            .or_insert_with(|| broadcast::channel(ROOM_MESSAGE_CAPACITY).0)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcasts_ordered_room_messages_to_every_subscriber() {
        let hub = RoomHub::default();
        let room_id = Uuid::new_v4();
        let mut first = hub.subscribe(room_id);
        let mut second = hub.subscribe(room_id);
        let publisher = hub.publisher(room_id);

        let text = Message::Text("transcript".into());
        let audio = Message::Binary(vec![1, 2, 3].into());
        publisher.send(text.clone()).unwrap();
        publisher.send(audio.clone()).unwrap();

        assert_eq!(first.recv().await.unwrap(), text);
        assert_eq!(first.recv().await.unwrap(), audio);
        assert_eq!(
            second.recv().await.unwrap(),
            Message::Text("transcript".into())
        );
        assert_eq!(
            second.recv().await.unwrap(),
            Message::Binary(vec![1, 2, 3].into())
        );
    }

    #[tokio::test]
    async fn isolates_messages_between_rooms() {
        let hub = RoomHub::default();
        let first_room = Uuid::new_v4();
        let second_room = Uuid::new_v4();
        let mut first = hub.subscribe(first_room);
        let mut second = hub.subscribe(second_room);

        hub.publisher(first_room)
            .send(Message::Text("first-room".into()))
            .unwrap();

        assert_eq!(
            first.recv().await.unwrap(),
            Message::Text("first-room".into())
        );
        assert!(matches!(
            second.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }
}
