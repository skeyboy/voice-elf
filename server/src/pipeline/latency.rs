use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::protocol::{INPUT_SAMPLE_RATE, LatencyReport};

#[derive(Clone, Copy)]
pub(super) struct SpeechStart {
    instant: Instant,
    unix_ms: u64,
}

impl SpeechStart {
    pub(super) fn now() -> Self {
        Self {
            instant: Instant::now(),
            unix_ms: unix_ms(),
        }
    }
}

pub(super) struct LatencyObserver {
    start: SpeechStart,
    t1: Option<(Instant, u64)>,
    t2: Option<(Instant, u64)>,
    t3: Option<(Instant, u64)>,
}

impl LatencyObserver {
    pub(super) fn new(start: SpeechStart) -> Self {
        Self {
            start,
            t1: None,
            t2: None,
            t3: None,
        }
    }

    pub(super) fn mark_vad_complete(&mut self) {
        self.t1 = Some(mark());
    }

    pub(super) fn mark_stt_complete(&mut self) {
        self.t2 = Some(mark());
    }

    pub(super) fn mark_translation_complete(&mut self) {
        self.t3 = Some(mark());
    }

    pub(super) fn text_report(&self, input_samples: usize) -> LatencyReport {
        self.report_with_t4(
            input_samples,
            self.t3.expect("translation timestamp must be set"),
        )
    }

    pub(super) fn queued_report(&self, input_samples: usize) -> LatencyReport {
        self.report_with_t4(input_samples, self.t1.expect("VAD timestamp must be set"))
    }

    pub(super) fn transcription_report(&self, input_samples: usize) -> LatencyReport {
        self.report_with_t4(input_samples, self.t2.expect("STT timestamp must be set"))
    }

    fn report_with_t4(&self, input_samples: usize, t4: (Instant, u64)) -> LatencyReport {
        let t1 = self.t1.expect("VAD timestamp must be set");
        let t2 = self.t2.unwrap_or(t1);
        let t3 = self.t3.unwrap_or(t2);
        LatencyReport {
            vad_ms: elapsed_ms(self.start.instant, t1.0),
            stt_ms: elapsed_ms(t1.0, t2.0),
            translation_ms: elapsed_ms(t2.0, t3.0),
            tts_ms: elapsed_ms(t3.0, t4.0),
            total_ms: elapsed_ms(self.start.instant, t4.0),
            audio_ms: input_samples as u64 * 1_000 / INPUT_SAMPLE_RATE as u64,
            t0_unix_ms: self.start.unix_ms,
            t1_unix_ms: t1.1,
            t2_unix_ms: t2.1,
            t3_unix_ms: t3.1,
            t4_unix_ms: t4.1,
        }
    }
}

fn mark() -> (Instant, u64) {
    (Instant::now(), unix_ms())
}

fn elapsed_ms(start: Instant, end: Instant) -> u64 {
    end.saturating_duration_since(start).as_millis() as u64
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}
