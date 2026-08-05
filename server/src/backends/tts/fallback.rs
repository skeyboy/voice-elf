use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;

use super::{TtsChunkSink, TtsEngine, TtsRequest, canonical_language};

pub struct FallbackTtsEngine {
    engines: Vec<Arc<dyn TtsEngine>>,
}

impl FallbackTtsEngine {
    pub fn new(engines: Vec<Arc<dyn TtsEngine>>) -> Result<Self> {
        if engines.is_empty() {
            bail!("at least one TTS engine is required");
        }
        Ok(Self { engines })
    }
}

#[async_trait]
impl TtsEngine for FallbackTtsEngine {
    fn name(&self) -> &'static str {
        "fallback"
    }

    fn supports(&self, language: &str) -> bool {
        self.engines.iter().any(|engine| engine.supports(language))
    }

    fn supports_voice_clone(&self) -> bool {
        self.engines
            .iter()
            .any(|engine| engine.supports_voice_clone())
    }

    async fn synthesize(&self, request: &TtsRequest, output: TtsChunkSink) -> Result<()> {
        let language = canonical_language(&request.language);
        let mut failures = Vec::new();
        let mut candidates = 0;
        for engine in &self.engines {
            if !engine.supports(&language)
                || (request.reference_audio_path.is_some() && !engine.supports_voice_clone())
            {
                continue;
            }
            candidates += 1;
            let engine_output = output.for_engine(engine.name());
            match engine.synthesize(request, engine_output.clone()).await {
                Ok(()) if engine_output.emitted_chunks() > 0 => return Ok(()),
                Ok(()) => failures.push(format!("{}: empty audio", engine.name())),
                Err(error) if engine_output.emitted_chunks() == 0 => {
                    tracing::warn!(engine = engine.name(), %error, "TTS engine failed before streaming; trying fallback");
                    failures.push(format!("{}: {error}", engine.name()));
                }
                Err(error) => {
                    return Err(error).map_err(|error| {
                        anyhow!(
                            "{} failed after audio streaming started; fallback was suppressed: {error}",
                            engine.name()
                        )
                    });
                }
            }
        }
        if candidates == 0 {
            bail!("no TTS engine supports language '{language}'");
        }
        Err(anyhow!("all TTS engines failed: {}", failures.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use tokio::sync::mpsc;

    struct FakeEngine {
        name: &'static str,
        fail: bool,
        voice_clone: bool,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TtsEngine for FakeEngine {
        fn name(&self) -> &'static str {
            self.name
        }

        fn supports(&self, _language: &str) -> bool {
            true
        }

        fn supports_voice_clone(&self) -> bool {
            self.voice_clone
        }

        async fn synthesize(&self, _request: &TtsRequest, output: TtsChunkSink) -> Result<()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                bail!("expected failure");
            }
            output.send(vec![1, -1], 24_000, 1).await
        }
    }

    #[tokio::test]
    async fn falls_back_when_primary_fails_before_emitting_audio() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let engine = FallbackTtsEngine::new(vec![
            Arc::new(FakeEngine {
                name: "primary",
                fail: true,
                voice_clone: false,
                calls: primary_calls.clone(),
            }),
            Arc::new(FakeEngine {
                name: "fallback",
                fail: false,
                voice_clone: false,
                calls: fallback_calls.clone(),
            }),
        ])
        .unwrap();
        let (tx, mut rx) = mpsc::channel(2);
        engine
            .synthesize(
                &TtsRequest {
                    text: "hello".to_owned(),
                    language: "en".to_owned(),
                    voice: "F1".to_owned(),
                    reference_audio_path: None,
                },
                TtsChunkSink::new(tx),
            )
            .await
            .unwrap();
        assert_eq!(primary_calls.load(Ordering::Relaxed), 1);
        assert_eq!(fallback_calls.load(Ordering::Relaxed), 1);
        assert_eq!(rx.recv().await.unwrap().engine, "fallback");
    }

    #[tokio::test]
    async fn never_replaces_a_custom_voice_with_a_generic_fallback() {
        let clone_calls = Arc::new(AtomicUsize::new(0));
        let generic_calls = Arc::new(AtomicUsize::new(0));
        let engine = FallbackTtsEngine::new(vec![
            Arc::new(FakeEngine {
                name: "clone",
                fail: true,
                voice_clone: true,
                calls: clone_calls.clone(),
            }),
            Arc::new(FakeEngine {
                name: "generic",
                fail: false,
                voice_clone: false,
                calls: generic_calls.clone(),
            }),
        ])
        .unwrap();
        let (tx, _rx) = mpsc::channel(2);
        let result = engine
            .synthesize(
                &TtsRequest {
                    text: "hello".to_owned(),
                    language: "en".to_owned(),
                    voice: "custom:test".to_owned(),
                    reference_audio_path: Some("reference.wav".into()),
                },
                TtsChunkSink::new(tx),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(clone_calls.load(Ordering::Relaxed), 1);
        assert_eq!(generic_calls.load(Ordering::Relaxed), 0);
    }
}
