use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Context, Result};
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    backends::{LiveTranscription, Transcriber, Transcription},
    protocol::{PipelinePhase, ServerEvent, SessionConfig},
};

use super::{
    PipelineContext,
    events::{send_event, send_state},
    latency::{LatencyObserver, SpeechStart},
    synthesis::run_synthesis_worker,
    transcription::run_transcription_worker,
    translation::run_translation_worker,
};

const TEXT_QUEUE_CAPACITY: usize = 64;
const SYNTHESIS_QUEUE_CAPACITY: usize = 32;

pub(super) struct UtteranceJob {
    pub id: Uuid,
    pub audio: Vec<i16>,
    pub config: SessionConfig,
    pub latency: LatencyObserver,
    pub live: Option<LivePreview>,
}

pub(super) struct LiveUtterance {
    id: Uuid,
    config: SessionConfig,
    latency: LatencyObserver,
    preview: Option<LivePreview>,
}

pub(super) struct LivePreview {
    transcription: Option<LiveTranscription>,
    updates: Option<JoinHandle<String>>,
}

impl LivePreview {
    pub(super) async fn finish(mut self) -> Result<Transcription> {
        let transcription = self
            .transcription
            .take()
            .context("live transcription handle is missing")?
            .finish()
            .await;
        if let Some(updates) = self.updates.take() {
            let _ = updates.await;
        }
        transcription
    }

    fn push(&self, pcm: &[i16]) -> Result<()> {
        self.transcription
            .as_ref()
            .context("live transcription handle is missing")?
            .push(pcm)
    }
}

impl Drop for LivePreview {
    fn drop(&mut self) {
        if let Some(updates) = self.updates.take() {
            updates.abort();
        }
    }
}

impl LiveUtterance {
    pub(super) fn push(&self, pcm: &[i16]) -> Result<()> {
        if let Some(preview) = &self.preview {
            preview.push(pcm)?;
        }
        Ok(())
    }
}

pub(super) struct TranslationJob {
    pub utterance: UtteranceJob,
    pub transcription: Transcription,
}

pub(super) struct SynthesisJob {
    pub utterance: UtteranceJob,
    pub translated: String,
}

#[derive(Clone, Default)]
pub(super) struct TextWorkload {
    pending: Arc<AtomicUsize>,
    idle: Arc<Notify>,
    work: Arc<Notify>,
}

impl TextWorkload {
    pub(super) fn add(&self) {
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.work.notify_waiters();
    }

    pub(super) fn finish(&self) {
        let previous = self.pending.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "text workload underflow");
        if previous == 1 {
            self.idle.notify_waiters();
        }
    }

    pub(super) async fn wait_until_idle(&self) {
        loop {
            let notified = self.idle.notified();
            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn wait_for_work(&self) {
        loop {
            let notified = self.work.notified();
            if self.pending.load(Ordering::Acquire) > 0 {
                return;
            }
            notified.await;
        }
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }
}

pub(super) struct PipelineWorkers {
    transcription_tx: mpsc::Sender<UtteranceJob>,
    transcriber: Arc<dyn Transcriber>,
    workload: TextWorkload,
    handles: Vec<JoinHandle<()>>,
}

impl PipelineWorkers {
    pub(super) fn start(context: PipelineContext) -> Self {
        let transcriber = context.services.transcriber.clone();
        let workload = TextWorkload::default();
        let (transcription_tx, transcription_rx) = mpsc::channel(TEXT_QUEUE_CAPACITY);
        let (translation_tx, translation_rx) = mpsc::channel(TEXT_QUEUE_CAPACITY);
        let (synthesis_tx, synthesis_rx) = mpsc::channel(SYNTHESIS_QUEUE_CAPACITY);

        let handles = vec![
            tokio::spawn(run_transcription_worker(
                context.clone(),
                transcription_rx,
                translation_tx,
                workload.clone(),
            )),
            tokio::spawn(run_translation_worker(
                context.clone(),
                translation_rx,
                synthesis_tx,
                workload.clone(),
            )),
            tokio::spawn(run_synthesis_worker(
                context,
                synthesis_rx,
                workload.clone(),
            )),
        ];

        Self {
            transcription_tx,
            transcriber,
            workload,
            handles,
        }
    }

    pub(super) async fn begin_live(
        &self,
        started: SpeechStart,
        config: &SessionConfig,
        output: &mpsc::Sender<axum::extract::ws::Message>,
    ) -> Result<LiveUtterance> {
        let id = Uuid::new_v4();
        let utterance_id = id.to_string();
        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
        let preview = match self
            .transcriber
            .start_live(&config.source_language, updates_tx)
            .await
        {
            Ok(Some(transcription)) => {
                let event_output = output.clone();
                let event_id = utterance_id.clone();
                let language = config.source_language.clone();
                let started_at = std::time::Instant::now();
                let updates = tokio::spawn(async move {
                    let mut text = String::new();
                    let mut first_delta = true;
                    while let Some(delta) = updates_rx.recv().await {
                        if first_delta && !delta.trim().is_empty() {
                            first_delta = false;
                            tracing::info!(
                                utterance_id = %event_id,
                                first_text_ms = started_at.elapsed().as_millis(),
                                "live ASR emitted its first text"
                            );
                        }
                        text.push_str(&delta);
                        if send_event(
                            &event_output,
                            ServerEvent::TranscriptDelta {
                                utterance_id: event_id.clone(),
                                delta,
                                text: text.clone(),
                                language: language.clone(),
                                done: false,
                            },
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    text
                });
                Some(LivePreview {
                    transcription: Some(transcription),
                    updates: Some(updates),
                })
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, %utterance_id, "failed to start live ASR; using utterance fallback");
                None
            }
        };
        send_event(
            output,
            ServerEvent::UtteranceQueued {
                utterance_id: utterance_id.clone(),
            },
        )
        .await?;
        send_state(output, PipelinePhase::Transcribing, Some(&utterance_id)).await?;
        self.workload.add();
        Ok(LiveUtterance {
            id,
            config: config.clone(),
            latency: LatencyObserver::new(started),
            preview,
        })
    }

    pub(super) async fn finish_live(&self, audio: Vec<i16>, mut live: LiveUtterance) -> Result<()> {
        live.latency.mark_vad_complete();
        if let Err(error) = self
            .transcription_tx
            .send(UtteranceJob {
                id: live.id,
                audio,
                config: live.config,
                latency: live.latency,
                live: live.preview.take(),
            })
            .await
        {
            self.workload.finish();
            return Err(error).context("transcription worker stopped");
        }
        Ok(())
    }

    pub(super) async fn cancel_live(
        &self,
        live: LiveUtterance,
        output: &mpsc::Sender<axum::extract::ws::Message>,
    ) {
        let utterance_id = live.id.to_string();
        drop(live);
        self.workload.finish();
        let _ = send_event(
            output,
            ServerEvent::RecognitionFailed {
                utterance_id,
                message: "语音时间过短，请重试".to_owned(),
            },
        )
        .await;
    }

    #[cfg(test)]
    pub(super) async fn enqueue(
        &self,
        audio: Vec<i16>,
        started: SpeechStart,
        config: &SessionConfig,
        output: &tokio::sync::mpsc::Sender<axum::extract::ws::Message>,
    ) -> Result<()> {
        let id = Uuid::new_v4();
        let mut latency = LatencyObserver::new(started);
        latency.mark_vad_complete();
        send_event(
            output,
            ServerEvent::UtteranceQueued {
                utterance_id: id.to_string(),
            },
        )
        .await?;
        self.workload.add();
        if let Err(error) = self
            .transcription_tx
            .send(UtteranceJob {
                id,
                audio,
                config: config.clone(),
                latency,
                live: None,
            })
            .await
        {
            self.workload.finish();
            return Err(error).context("transcription worker stopped");
        }
        Ok(())
    }

    pub(super) async fn abort(self) {
        drop(self.transcription_tx);
        for handle in &self.handles {
            handle.abort();
        }
        for handle in self.handles {
            let _ = handle.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::extract::ws::Message;
    use serde_json::Value;

    use crate::{
        backends::{
            AppServices, DemoSynthesizer, DemoTranscriber, DemoTranslator, NoSpeechDetected,
            Transcriber, Transcription,
        },
        media::MediaStore,
        protocol::SessionConfig,
    };

    use super::*;

    #[tokio::test]
    async fn workload_reports_new_text_while_tts_waits() {
        let workload = TextWorkload::default();
        workload.add();
        workload.wait_for_work().await;
        assert_eq!(workload.pending(), 1);
        workload.finish();
        workload.wait_until_idle().await;
        assert_eq!(workload.pending(), 0);
    }

    #[tokio::test]
    async fn completes_all_pending_text_before_starting_audio() {
        let directory = tempfile::tempdir().unwrap();
        let media = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let services = Arc::new(AppServices {
            transcriber: Arc::new(DemoTranscriber::new()),
            translator: Arc::new(DemoTranslator::new()),
            synthesizer: Arc::new(DemoSynthesizer::new()),
            backend_name: "demo",
        });
        let (output, mut events) = mpsc::channel(256);
        let context = PipelineContext {
            services,
            database: None,
            media,
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            room_id: Uuid::new_v4(),
            output: output.clone(),
        };
        let workers = PipelineWorkers::start(context);
        for _ in 0..2 {
            workers
                .enqueue(
                    vec![1_000; 4_800],
                    SpeechStart::now(),
                    &SessionConfig::default(),
                    &output,
                )
                .await
                .unwrap();
        }

        let mut translations = 0;
        tokio::time::timeout(Duration::from_secs(8), async {
            while let Some(message) = events.recv().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let event: Value = serde_json::from_str(&text).unwrap();
                match event["type"].as_str() {
                    Some("translation") => translations += 1,
                    Some("audio_start") => break,
                    _ => {}
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(translations, 2);
        workers.abort().await;
    }

    struct EmptyTranscriber;

    #[async_trait::async_trait]
    impl Transcriber for EmptyTranscriber {
        async fn transcribe_streaming(
            &self,
            _pcm: &[i16],
            _source_language: &str,
            _updates: mpsc::UnboundedSender<String>,
        ) -> anyhow::Result<Transcription> {
            Err(NoSpeechDetected.into())
        }
    }

    #[tokio::test]
    async fn recognition_failure_stays_attached_to_its_utterance() {
        let directory = tempfile::tempdir().unwrap();
        let media = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let services = Arc::new(AppServices {
            transcriber: Arc::new(EmptyTranscriber),
            translator: Arc::new(DemoTranslator::new()),
            synthesizer: Arc::new(DemoSynthesizer::new()),
            backend_name: "test",
        });
        let (output, mut events) = mpsc::channel(64);
        let context = PipelineContext {
            services,
            database: None,
            media,
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            room_id: Uuid::new_v4(),
            output: output.clone(),
        };
        let workers = PipelineWorkers::start(context);
        workers
            .enqueue(
                vec![1_000; 4_800],
                SpeechStart::now(),
                &SessionConfig::default(),
                &output,
            )
            .await
            .unwrap();

        let mut media_id = None;
        let mut failure_id = None;
        let mut event_types = Vec::new();
        let mut transcribing_index = None;
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(message) = events.recv().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let event: Value = serde_json::from_str(&text).unwrap();
                if let Some(event_type) = event["type"].as_str() {
                    event_types.push(event_type.to_owned());
                }
                if event["type"] == "state" && event["phase"] == "transcribing" {
                    transcribing_index = Some(event_types.len() - 1);
                }
                match event["type"].as_str() {
                    Some("media") => media_id = event["utterance_id"].as_str().map(str::to_owned),
                    Some("recognition_failed") => {
                        failure_id = event["utterance_id"].as_str().map(str::to_owned);
                        break;
                    }
                    Some("error") => panic!("recognition failure must not become a global error"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(media_id, failure_id);
        assert!(failure_id.is_some());
        let transcribing = transcribing_index.unwrap();
        let media = event_types
            .iter()
            .position(|event| event == "media")
            .unwrap();
        assert!(
            transcribing < media,
            "the frontend row must exist before media arrives"
        );
        workers.abort().await;
    }
}
