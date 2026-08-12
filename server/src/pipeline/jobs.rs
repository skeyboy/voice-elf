use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{Duration, sleep},
};
use uuid::Uuid;

use crate::{
    backends::{LiveTranscription, Transcriber, Transcription, Translator},
    protocol::{PipelinePhase, ServerEvent, SessionConfig, SpeakerIdentity},
};

use super::{
    PipelineContext,
    events::{send_event, send_state},
    latency::{LatencyObserver, SpeechStart},
    refinement::run_refinement_worker,
    synthesis::run_synthesis_worker,
    transcription::run_transcription_worker,
    translation::run_translation_worker,
};

const TEXT_QUEUE_CAPACITY: usize = 64;
const LIVE_TRANSLATION_DEBOUNCE: Duration = Duration::from_millis(180);

pub(super) struct UtteranceJob {
    pub id: Uuid,
    pub audio: Vec<i16>,
    pub config: SessionConfig,
    pub latency: LatencyObserver,
    pub live: Option<LivePreview>,
    pub speakers: Vec<SpeakerIdentity>,
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
    translation: Option<JoinHandle<()>>,
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
        if let Some(translation) = self.translation.take() {
            translation.abort();
            let _ = translation.await;
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
        if let Some(translation) = self.translation.take() {
            translation.abort();
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

pub(super) struct RefinementJob {
    pub utterance_id: Uuid,
    pub audio: Vec<i16>,
    pub source_language: String,
}

pub(super) struct SynthesisJob {
    pub utterance: UtteranceJob,
    pub translated_text: String,
}

pub(super) struct PipelineWorkers {
    transcription_tx: mpsc::Sender<UtteranceJob>,
    transcriber: Arc<dyn Transcriber>,
    translator: Arc<dyn Translator>,
    language_policy: crate::language_policy::LanguagePolicy,
    handles: Vec<JoinHandle<()>>,
}

impl PipelineWorkers {
    pub(super) fn start(context: PipelineContext) -> Self {
        let transcriber = context.services.transcriber.clone();
        let translator = context.services.translator.clone();
        let language_policy = context.language_policy.clone();
        let (transcription_tx, transcription_rx) = mpsc::channel(TEXT_QUEUE_CAPACITY);
        let (translation_tx, translation_rx) = mpsc::channel(TEXT_QUEUE_CAPACITY);
        let (synthesis_tx, synthesis_rx) = mpsc::channel(TEXT_QUEUE_CAPACITY);
        let (refinement_tx, refinement_rx) = mpsc::channel(TEXT_QUEUE_CAPACITY);

        let handles = vec![
            tokio::spawn(run_transcription_worker(
                context.clone(),
                transcription_rx,
                translation_tx,
                refinement_tx,
            )),
            tokio::spawn(run_translation_worker(
                context.clone(),
                translation_rx,
                synthesis_tx,
            )),
            tokio::spawn(run_refinement_worker(context.clone(), refinement_rx)),
            tokio::spawn(run_synthesis_worker(context, synthesis_rx)),
        ];

        Self {
            transcription_tx,
            transcriber,
            translator,
            language_policy,
            handles,
        }
    }

    pub(super) async fn begin_live(
        &self,
        id: Uuid,
        started: SpeechStart,
        config: &SessionConfig,
        output: &mpsc::Sender<axum::extract::ws::Message>,
    ) -> Result<LiveUtterance> {
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
                let (preview_tx, preview_rx) = watch::channel(String::new());
                let translation = tokio::spawn(run_live_translation_preview(
                    self.translator.clone(),
                    event_output.clone(),
                    event_id.clone(),
                    language.clone(),
                    config.target_language.clone(),
                    self.language_policy.clone(),
                    preview_rx,
                ));
                let updates = tokio::spawn(async move {
                    let mut text = String::new();
                    while let Some(delta) = updates_rx.recv().await {
                        text.push_str(&delta);
                        preview_tx.send_replace(text.clone());
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
                    translation: Some(translation),
                })
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, %utterance_id, "failed to start live ASR; using completed utterance fallback");
                None
            }
        };
        send_event(
            output,
            ServerEvent::UtteranceQueued {
                utterance_id: utterance_id.clone(),
                tc_id: utterance_id.clone(),
            },
        )
        .await?;
        send_state(output, PipelinePhase::Transcribing, Some(&utterance_id)).await?;
        Ok(LiveUtterance {
            id,
            config: config.clone(),
            latency: LatencyObserver::new(started),
            preview,
        })
    }

    pub(super) async fn finish_live(
        &self,
        audio: Vec<i16>,
        mut live: LiveUtterance,
        speakers: Vec<SpeakerIdentity>,
    ) -> Result<()> {
        live.latency.mark_vad_complete();
        self.transcription_tx
            .send(UtteranceJob {
                id: live.id,
                audio,
                config: live.config,
                latency: live.latency,
                live: live.preview.take(),
                speakers,
            })
            .await
            .context("transcription worker stopped")
    }

    #[cfg(test)]
    pub(super) async fn enqueue(
        &self,
        id: Uuid,
        audio: Vec<i16>,
        started: SpeechStart,
        config: &SessionConfig,
        output: &mpsc::Sender<axum::extract::ws::Message>,
    ) -> Result<()> {
        let utterance_id = id.to_string();
        let mut latency = LatencyObserver::new(started);
        latency.mark_vad_complete();
        send_event(
            output,
            ServerEvent::UtteranceQueued {
                utterance_id: utterance_id.clone(),
                tc_id: utterance_id.clone(),
            },
        )
        .await?;
        send_state(output, PipelinePhase::Transcribing, Some(&utterance_id)).await?;
        if let Err(error) = self
            .transcription_tx
            .send(UtteranceJob {
                id,
                audio,
                config: config.clone(),
                latency,
                live: None,
                speakers: Vec::new(),
            })
            .await
        {
            return Err(error).context("transcription worker stopped");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn abort(self) {
        drop(self.transcription_tx);
        for handle in &self.handles {
            handle.abort();
        }
        for handle in self.handles {
            let _ = handle.await;
        }
    }

    pub(super) async fn finish(self) {
        drop(self.transcription_tx);
        for handle in self.handles {
            if let Err(error) = handle.await {
                tracing::warn!(%error, "pipeline worker stopped before its queue drained");
            }
        }
    }
}

async fn run_live_translation_preview(
    translator: Arc<dyn Translator>,
    output: mpsc::Sender<axum::extract::ws::Message>,
    utterance_id: String,
    source_language: String,
    target_language: String,
    policy: crate::language_policy::LanguagePolicy,
    mut source: watch::Receiver<String>,
) {
    let mut source_ready = false;
    'source_updates: loop {
        if !source_ready {
            if source.changed().await.is_err() {
                return;
            }
        }
        source_ready = false;
        loop {
            let debounce = sleep(LIVE_TRANSLATION_DEBOUNCE);
            tokio::pin!(debounce);
            tokio::select! {
                _ = &mut debounce => break,
                changed = source.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
        let source_text = policy.sanitize(source.borrow_and_update().trim());
        if source_text.is_empty() {
            continue;
        }

        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
        let terminology = policy.translation_terms();
        let translation = translator.translate_streaming(
            &source_text,
            &source_language,
            &target_language,
            &terminology,
            updates_tx,
        );
        tokio::pin!(translation);
        let mut translated_text = String::new();
        loop {
            tokio::select! {
                changed = source.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    source_ready = true;
                    continue 'source_updates;
                }
                Some(delta) = updates_rx.recv() => {
                    translated_text.push_str(&delta);
                    let visible_text = policy.normalize_translation(&translated_text);
                    if send_live_translation_delta(
                        &output,
                        &utterance_id,
                        &target_language,
                        String::new(),
                        &visible_text,
                    ).await.is_err() {
                        return;
                    }
                }
                result = &mut translation => {
                    match result {
                        Ok(final_text) => {
                            while let Ok(delta) = updates_rx.try_recv() {
                                translated_text.push_str(&delta);
                                let visible_text = policy.normalize_translation(&translated_text);
                                if send_live_translation_delta(
                                    &output,
                                    &utterance_id,
                                    &target_language,
                                    String::new(),
                                    &visible_text,
                                ).await.is_err() {
                                    return;
                                }
                            }
                            if translated_text.is_empty() && !final_text.trim().is_empty()
                                && send_live_translation_delta(
                                    &output,
                                    &utterance_id,
                                    &target_language,
                                    String::new(),
                                    &policy.normalize_translation(&final_text),
                                ).await.is_err()
                            {
                                return;
                            }
                        }
                        Err(error) => {
                            tracing::debug!(%error, %utterance_id, "live translation preview failed");
                        }
                    }
                    continue 'source_updates;
                }
            }
        }
    }
}

async fn send_live_translation_delta(
    output: &mpsc::Sender<axum::extract::ws::Message>,
    utterance_id: &str,
    target_language: &str,
    delta: String,
    text: &str,
) -> Result<()> {
    send_event(
        output,
        ServerEvent::TranslationDelta {
            utterance_id: utterance_id.to_owned(),
            delta,
            text: text.to_owned(),
            target_language: target_language.to_owned(),
            done: false,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::extract::ws::Message;
    use serde_json::Value;

    use crate::{
        backends::{
            AppServices, CompletedTranscriptionEngine, DemoSynthesizer, DemoTranscriber,
            DemoTranslator, NoSpeechDetected, Transcriber, Transcription,
        },
        media::MediaStore,
        protocol::SessionConfig,
    };

    use super::*;

    struct RestartingTranslator;

    #[async_trait::async_trait]
    impl Translator for RestartingTranslator {
        async fn translate_streaming(
            &self,
            text: &str,
            _source_language: &str,
            _target_language: &str,
            _terminology: &[crate::backends::TranslationTerm],
            updates: mpsc::UnboundedSender<String>,
        ) -> anyhow::Result<String> {
            if text == "first" {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let translated = format!("translated:{text}");
            let _ = updates.send(translated.clone());
            Ok(translated)
        }
    }

    #[tokio::test]
    async fn live_translation_restarts_with_the_latest_source_revision() {
        let (output, mut events) = mpsc::channel(16);
        let (source, source_updates) = watch::channel(String::new());
        let task = tokio::spawn(run_live_translation_preview(
            Arc::new(RestartingTranslator),
            output,
            "utterance-1".to_owned(),
            "en".to_owned(),
            "zh".to_owned(),
            crate::language_policy::LanguagePolicy::default(),
            source_updates,
        ));

        source.send_replace("first".to_owned());
        tokio::time::sleep(LIVE_TRANSLATION_DEBOUNCE + Duration::from_millis(40)).await;
        source.send_replace("second".to_owned());

        let event = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let Some(Message::Text(text)) = events.recv().await else {
                    continue;
                };
                let event: Value = serde_json::from_str(&text).unwrap();
                if event["type"] == "translation_delta" {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(event["text"], "translated:second");

        drop(source);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn completes_all_pending_text_and_server_audio() {
        let directory = tempfile::tempdir().unwrap();
        let media = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let services = Arc::new(AppServices {
            transcriber: Arc::new(DemoTranscriber::new()),
            refinement_engines: Vec::new(),
            translator: Arc::new(DemoTranslator::new()),
            synthesizer: Arc::new(DemoSynthesizer::new()),
            backend_name: "demo",
        });
        let (output, mut events) = mpsc::channel(256);
        let context = PipelineContext {
            services,
            database: None,
            language_policy: crate::language_policy::LanguagePolicy::default(),
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
                    Uuid::new_v4(),
                    vec![1_000; 4_800],
                    SpeechStart::now(),
                    &SessionConfig::default(),
                    &output,
                )
                .await
                .unwrap();
        }

        let mut translations = 0;
        let mut audio_ends = 0;
        tokio::time::timeout(Duration::from_secs(8), async {
            while let Some(message) = events.recv().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let event: Value = serde_json::from_str(&text).unwrap();
                match event["type"].as_str() {
                    Some("translation") => translations += 1,
                    Some("audio_end") => {
                        audio_ends += 1;
                        if audio_ends == 2 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(translations, 2);
        assert_eq!(audio_ends, 2);
        workers.abort().await;
    }

    struct EmptyTranscriber;

    #[async_trait::async_trait]
    impl CompletedTranscriptionEngine for EmptyTranscriber {
        fn name(&self) -> &'static str {
            "empty"
        }

        async fn transcribe_completed(
            &self,
            _pcm: &[i16],
            _source_language: &str,
        ) -> anyhow::Result<Transcription> {
            Err(NoSpeechDetected.into())
        }
    }

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

    struct SlowRefinement;

    #[async_trait::async_trait]
    impl CompletedTranscriptionEngine for SlowRefinement {
        fn name(&self) -> &'static str {
            "slow-refinement"
        }

        async fn transcribe_completed(
            &self,
            _pcm: &[i16],
            source_language: &str,
        ) -> anyhow::Result<Transcription> {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok(Transcription::plain("accurate transcript", source_language))
        }
    }

    #[tokio::test]
    async fn accurate_transcription_does_not_block_realtime_translation() {
        let directory = tempfile::tempdir().unwrap();
        let media = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let services = Arc::new(AppServices {
            transcriber: Arc::new(DemoTranscriber::new()),
            refinement_engines: vec![Arc::new(SlowRefinement)],
            translator: Arc::new(DemoTranslator::new()),
            synthesizer: Arc::new(DemoSynthesizer::new()),
            backend_name: "test",
        });
        let (output, mut events) = mpsc::channel(256);
        let context = PipelineContext {
            services,
            database: None,
            language_policy: crate::language_policy::LanguagePolicy::default(),
            media,
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            room_id: Uuid::new_v4(),
            output: output.clone(),
        };
        let workers = PipelineWorkers::start(context);
        workers
            .enqueue(
                Uuid::new_v4(),
                vec![1_000; 4_800],
                SpeechStart::now(),
                &SessionConfig::default(),
                &output,
            )
            .await
            .unwrap();

        let mut event_types = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(message) = events.recv().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let event: Value = serde_json::from_str(&text).unwrap();
                let Some(event_type) = event["type"].as_str() else {
                    continue;
                };
                event_types.push(event_type.to_owned());
                if event_type == "transcript_refinement"
                    && event["status"].as_str() == Some("completed")
                {
                    break;
                }
            }
        })
        .await
        .unwrap();

        let translation = event_types
            .iter()
            .position(|event| event == "translation")
            .unwrap();
        let refinement = event_types
            .iter()
            .rposition(|event| event == "transcript_refinement")
            .unwrap();
        assert!(translation < refinement, "event order: {event_types:?}");
        workers.abort().await;
    }

    #[tokio::test]
    async fn recognition_failure_stays_attached_to_its_utterance() {
        let directory = tempfile::tempdir().unwrap();
        let media = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let services = Arc::new(AppServices {
            transcriber: Arc::new(EmptyTranscriber),
            refinement_engines: Vec::new(),
            translator: Arc::new(DemoTranslator::new()),
            synthesizer: Arc::new(DemoSynthesizer::new()),
            backend_name: "test",
        });
        let (output, mut events) = mpsc::channel(64);
        let context = PipelineContext {
            services,
            database: None,
            language_policy: crate::language_policy::LanguagePolicy::default(),
            media,
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            room_id: Uuid::new_v4(),
            output: output.clone(),
        };
        let workers = PipelineWorkers::start(context);
        workers
            .enqueue(
                Uuid::new_v4(),
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
