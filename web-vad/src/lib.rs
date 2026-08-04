use std::{collections::VecDeque, ffi::c_void};

use lele::tensor::TensorView;

#[allow(clippy::get_first, clippy::type_complexity)]
#[path = "generated/silerovad.rs"]
mod silerovad;

static SILERO_WEIGHTS: &[u8] = include_bytes!("generated/silerovad_weights.bin");

pub const SAMPLE_RATE: usize = 16_000;
pub const FRAME_SAMPLES: usize = 512;
pub const MAX_INPUT_SAMPLES: usize = 2_048;
pub const FLAG_SPEECH_STARTED: u32 = 1 << 0;
pub const FLAG_SPEECH_ACTIVE: u32 = 1 << 1;
pub const FLAG_SPEECH_ENDED: u32 = 1 << 2;
pub const FLAG_VOICED: u32 = 1 << 3;
pub const FLAG_FORCED_END: u32 = 1 << 4;
pub const FLAG_FRAME_READY: u32 = 1 << 30;
pub const FLAG_INVALID_INPUT: u32 = 1 << 31;

// Silero probability, signal level, and the learned noise floor jointly guard each start.
const START_TRIGGER_FRAMES: u16 = 1;
const ENHANCED_START_TRIGGER_FRAMES: u16 = 1;
// Keep longer conversational pauses inside one utterance. 88 * 32 ms = 2.816 s.
const END_SILENCE_FRAMES: u16 = 88;
// Keep enough audio to cover Silero's onset latency without clipping short initials.
const PRE_ROLL_FRAMES: usize = 16;
const ENHANCED_PRE_ROLL_FRAMES: usize = 32;
const ENHANCED_CALIBRATION_FRAMES: u16 = 32;
const FORCED_CONTINUATION_FRAMES: u16 = 6;
// Energy may bridge brief model dropouts, but must not hold a segment open on AGC noise.
const ENERGY_HANGOVER_FRAMES: u16 = 6;
// Lele's wasm32 path produces lower Silero probabilities than its native path
// for the same input, so this threshold is calibrated against the final WASM.
const SPEECH_THRESHOLD: f32 = 0.03;
const START_LEVEL_THRESHOLD: f32 = 0.005;
const ENHANCED_START_LEVEL_THRESHOLD: f32 = 0.008;
const ACTIVE_LEVEL_THRESHOLD: f32 = 0.008;
const INITIAL_NOISE_FLOOR: f32 = 0.002;
const MAX_NOISE_FLOOR: f32 = 0.012;
const START_SIGNAL_TO_NOISE: f32 = 1.35;
const ACTIVE_SIGNAL_TO_NOISE: f32 = 1.2;
// Broadband noise crosses zero far more often than conversational speech. Keep
// this deliberately loose so fricatives still pass when Silero confirms them.
const MAX_SPEECH_ZERO_CROSSING_RATE: f32 = 0.32;
// Lock out sustained sub-100 Hz hum until several clearly non-tonal frames arrive.
const ENHANCED_HUM_LEVEL: f32 = 0.03;
const ENHANCED_HUM_MAX_ZERO_CROSSING_RATE: f32 = 0.012;
const ENHANCED_HUM_LOCK_FRAMES: u16 = 8;
const ENHANCED_HUM_CLEAR_ZERO_CROSSING_RATE: f32 = 0.02;
const ENHANCED_HUM_CLEAR_FRAMES: u16 = 3;

pub struct SpeechGate {
    speaking: bool,
    voiced_run: u16,
    silence_run: u16,
    active_frames: u32,
    max_active_frames: u32,
    continuation_frames: u16,
    start_trigger_frames: u16,
}

impl SpeechGate {
    pub fn new(max_utterance_seconds: u32, enhanced_voice_filter: bool) -> Self {
        Self {
            speaking: false,
            voiced_run: 0,
            silence_run: 0,
            active_frames: 0,
            max_active_frames: max_utterance_seconds.clamp(5, 20) * 1_000 / 32,
            continuation_frames: 0,
            start_trigger_frames: if enhanced_voice_filter {
                ENHANCED_START_TRIGGER_FRAMES
            } else {
                START_TRIGGER_FRAMES
            },
        }
    }

    pub fn process(&mut self, voiced: bool) -> u32 {
        let mut flags = if voiced { FLAG_VOICED } else { 0 };
        if !self.speaking {
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            if self.voiced_run >= self.start_trigger_frames {
                self.speaking = true;
                self.silence_run = 0;
                self.active_frames = 1;
                self.continuation_frames = 0;
                flags |= FLAG_SPEECH_STARTED | FLAG_SPEECH_ACTIVE;
            } else {
                self.continuation_frames = self.continuation_frames.saturating_sub(1);
            }
            return flags;
        }

        self.active_frames += 1;
        self.silence_run = if voiced { 0 } else { self.silence_run + 1 };
        let forced = self.active_frames >= self.max_active_frames;
        if self.silence_run >= END_SILENCE_FRAMES || forced {
            self.speaking = false;
            self.voiced_run = 0;
            self.silence_run = 0;
            self.active_frames = 0;
            self.continuation_frames = if forced && voiced {
                FORCED_CONTINUATION_FRAMES
            } else {
                0
            };
            flags |= FLAG_SPEECH_ENDED;
            if forced {
                flags |= FLAG_FORCED_END;
            }
        } else {
            flags |= FLAG_SPEECH_ACTIVE;
        }
        flags
    }

    pub fn reset(&mut self) {
        self.speaking = false;
        self.voiced_run = 0;
        self.silence_run = 0;
        self.active_frames = 0;
        self.continuation_frames = 0;
    }

    fn accepts_active_energy(&self) -> bool {
        self.speaking || self.continuation_frames > 0
    }
}

struct AudioFrame {
    samples: [i16; FRAME_SAMPLES],
    flags: u32,
    level: f32,
}

struct BrowserAudioVad {
    model: silerovad::SileroVad<'static>,
    workspace: silerovad::SileroVadWorkspace,
    model_state: Vec<f32>,
    gate: SpeechGate,
    noise_floor: f32,
    model_silence_run: u16,
    segment_model_frames: u32,
    completed_model_frames: u32,
    enhanced_voice_filter: bool,
    enhanced_calibration_frames: u16,
    enhanced_hum_locked: bool,
    enhanced_hum_run: u16,
    enhanced_hum_clear_run: u16,
    enhanced_recovered_from_hum: bool,
    enhanced_acoustic_override: bool,
    enhanced_acoustic_voice: bool,
    input: [f32; MAX_INPUT_SAMPLES],
    output: [i16; FRAME_SAMPLES],
    output_level: f32,
    input_level: f32,
    resample_source: Vec<f32>,
    resample_position: f64,
    resample_ratio: f64,
    frame: [i16; FRAME_SAMPLES],
    frame_offset: usize,
    pre_roll: VecDeque<([i16; FRAME_SAMPLES], f32)>,
    pre_roll_frames: usize,
    ready: VecDeque<AudioFrame>,
    segment_active: bool,
}

impl BrowserAudioVad {
    fn new(
        max_utterance_seconds: u32,
        input_sample_rate: u32,
        enhanced_voice_filter: bool,
    ) -> Option<Self> {
        if !(8_000..=192_000).contains(&input_sample_rate) {
            return None;
        }
        Some(Self {
            model: silerovad::SileroVad::new(SILERO_WEIGHTS),
            workspace: silerovad::SileroVadWorkspace::new(),
            model_state: vec![0.0; 2 * 128],
            gate: SpeechGate::new(max_utterance_seconds, enhanced_voice_filter),
            noise_floor: INITIAL_NOISE_FLOOR,
            model_silence_run: 0,
            segment_model_frames: 0,
            completed_model_frames: 0,
            enhanced_voice_filter,
            enhanced_calibration_frames: if enhanced_voice_filter {
                ENHANCED_CALIBRATION_FRAMES
            } else {
                0
            },
            enhanced_hum_locked: false,
            enhanced_hum_run: 0,
            enhanced_hum_clear_run: 0,
            enhanced_recovered_from_hum: false,
            enhanced_acoustic_override: false,
            enhanced_acoustic_voice: false,
            input: [0.0; MAX_INPUT_SAMPLES],
            output: [0; FRAME_SAMPLES],
            output_level: 0.0,
            input_level: 0.0,
            resample_source: Vec::with_capacity(MAX_INPUT_SAMPLES + 8),
            resample_position: 0.0,
            resample_ratio: input_sample_rate as f64 / SAMPLE_RATE as f64,
            frame: [0; FRAME_SAMPLES],
            frame_offset: 0,
            pre_roll: VecDeque::with_capacity(
                if enhanced_voice_filter {
                    ENHANCED_PRE_ROLL_FRAMES
                } else {
                    PRE_ROLL_FRAMES
                } + 1,
            ),
            pre_roll_frames: if enhanced_voice_filter {
                ENHANCED_PRE_ROLL_FRAMES
            } else {
                PRE_ROLL_FRAMES
            },
            ready: VecDeque::new(),
            segment_active: false,
        })
    }

    fn process_input(&mut self, sample_count: usize) -> bool {
        if sample_count == 0 || sample_count > MAX_INPUT_SAMPLES {
            return false;
        }
        self.resample_source
            .extend_from_slice(&self.input[..sample_count]);

        while self.resample_position + 1.0 < self.resample_source.len() as f64 {
            let left = self.resample_position.floor() as usize;
            let fraction = (self.resample_position - left as f64) as f32;
            let sample = self.resample_source[left]
                + (self.resample_source[left + 1] - self.resample_source[left]) * fraction;
            self.push_resampled(float_to_pcm16(sample));
            self.resample_position += self.resample_ratio;
        }

        let consumed = (self.resample_position.floor() as usize).min(self.resample_source.len());
        self.resample_source.drain(..consumed);
        self.resample_position -= consumed as f64;
        true
    }

    fn push_resampled(&mut self, sample: i16) {
        self.frame[self.frame_offset] = sample;
        self.frame_offset += 1;
        if self.frame_offset != FRAME_SAMPLES {
            return;
        }

        let frame = self.frame;
        self.frame_offset = 0;
        let level = rms_level(&frame);
        self.input_level = level;
        let probability = self.predict_speech(&frame);
        if !self.gate.accepts_active_energy() && probability < SPEECH_THRESHOLD {
            self.update_noise_floor(level);
        }
        let speech_like = self.frame_is_speech_like(&frame, level);
        let acoustic_override = self.enhanced_acoustic_override;
        let acoustic_voice = self.enhanced_acoustic_voice;
        let (voiced, model_voiced) =
            self.classify_voiced(probability, level, speech_like, acoustic_voice);
        let flags = self.gate.process(voiced);
        if flags & FLAG_SPEECH_STARTED != 0 {
            self.segment_model_frames = if acoustic_override {
                ENHANCED_HUM_CLEAR_FRAMES as u32
            } else {
                0
            };
            self.completed_model_frames = 0;
        }
        if model_voiced && (self.segment_active || flags & FLAG_SPEECH_STARTED != 0) {
            self.segment_model_frames = self.segment_model_frames.saturating_add(1);
        }
        if flags & FLAG_SPEECH_ENDED != 0 {
            self.completed_model_frames = self.segment_model_frames;
        }
        self.enhanced_acoustic_override = false;
        self.enhanced_acoustic_voice = false;
        self.route_frame(frame, level, flags);
    }

    fn classify_voiced(
        &mut self,
        probability: f32,
        level: f32,
        speech_like: bool,
        acoustic_override: bool,
    ) -> (bool, bool) {
        let configured_start_level = if self.enhanced_voice_filter {
            ENHANCED_START_LEVEL_THRESHOLD
        } else {
            START_LEVEL_THRESHOLD
        };
        let start_level = configured_start_level.max(self.noise_floor * START_SIGNAL_TO_NOISE);
        let active_level = ACTIVE_LEVEL_THRESHOLD.max(self.noise_floor * ACTIVE_SIGNAL_TO_NOISE);
        let model_voiced = (probability >= SPEECH_THRESHOLD || acoustic_override)
            && level >= start_level
            && speech_like;
        self.model_silence_run = if model_voiced {
            0
        } else {
            self.model_silence_run.saturating_add(1)
        };
        let energy_hangover = self.gate.accepts_active_energy()
            && self.model_silence_run <= ENERGY_HANGOVER_FRAMES
            && level >= active_level;
        (model_voiced || energy_hangover, model_voiced)
    }

    fn frame_is_speech_like(&mut self, frame: &[i16; FRAME_SAMPLES], level: f32) -> bool {
        self.enhanced_acoustic_override = false;
        self.enhanced_acoustic_voice = false;
        let crossing_rate = zero_crossing_rate(frame);
        if crossing_rate > MAX_SPEECH_ZERO_CROSSING_RATE {
            return false;
        }
        if !self.enhanced_voice_filter {
            return true;
        }

        let hum_frame =
            level >= ENHANCED_HUM_LEVEL && crossing_rate <= ENHANCED_HUM_MAX_ZERO_CROSSING_RATE;
        self.enhanced_hum_run = if hum_frame {
            self.enhanced_hum_run.saturating_add(1)
        } else {
            0
        };
        if self.enhanced_hum_run >= ENHANCED_HUM_LOCK_FRAMES {
            let recovered_segment = self.enhanced_recovered_from_hum;
            self.enhanced_hum_locked = true;
            self.enhanced_hum_run = 0;
            self.enhanced_hum_clear_run = 0;
            self.enhanced_recovered_from_hum = false;
            if !recovered_segment && self.segment_active {
                self.segment_model_frames = 0;
            }
        }
        if self.enhanced_calibration_frames > 0 {
            self.enhanced_calibration_frames -= 1;
            return false;
        }
        if hum_frame {
            return false;
        }
        if !self.enhanced_hum_locked {
            if self.enhanced_recovered_from_hum
                && level >= ENHANCED_START_LEVEL_THRESHOLD
                && crossing_rate >= ENHANCED_HUM_CLEAR_ZERO_CROSSING_RATE
            {
                self.enhanced_acoustic_voice = true;
            }
            return true;
        }

        if level >= ENHANCED_START_LEVEL_THRESHOLD
            && crossing_rate >= ENHANCED_HUM_CLEAR_ZERO_CROSSING_RATE
        {
            self.enhanced_hum_clear_run = self.enhanced_hum_clear_run.saturating_add(1);
            if self.enhanced_hum_clear_run >= ENHANCED_HUM_CLEAR_FRAMES {
                self.enhanced_hum_locked = false;
                self.enhanced_hum_clear_run = 0;
                self.enhanced_recovered_from_hum = true;
                self.enhanced_acoustic_override = true;
                self.enhanced_acoustic_voice = true;
                return true;
            }
        } else {
            self.enhanced_hum_clear_run = self.enhanced_hum_clear_run.saturating_sub(1);
        }
        !self.enhanced_hum_locked
    }

    fn update_noise_floor(&mut self, level: f32) {
        let observed = level.clamp(0.000_2, MAX_NOISE_FLOOR);
        let smoothing = if observed > self.noise_floor {
            0.97
        } else {
            0.82
        };
        self.noise_floor = self.noise_floor * smoothing + observed * (1.0 - smoothing);
    }

    fn predict_speech(&mut self, frame: &[i16; FRAME_SAMPLES]) -> f32 {
        let input = TensorView::from_owned(
            frame.iter().map(|sample| *sample as f32).collect(),
            vec![1, FRAME_SAMPLES],
        );
        let state = TensorView::from_owned(self.model_state.clone(), vec![2, 1, 128]);
        let sample_rate = TensorView::from_owned(vec![SAMPLE_RATE as i64], vec![1]);
        let (output, next_state) =
            self.model
                .forward_with_workspace(&mut self.workspace, input, state, sample_rate);
        let probability = output.data.first().copied().unwrap_or(0.0);
        self.model_state.clear();
        self.model_state.extend_from_slice(&next_state.data);
        probability
    }

    fn route_frame(&mut self, frame: [i16; FRAME_SAMPLES], level: f32, flags: u32) {
        if flags & FLAG_SPEECH_STARTED != 0 {
            self.segment_active = true;
            self.pre_roll.push_back((frame, level));
            let last = self.pre_roll.len().saturating_sub(1);
            for (index, (buffered, buffered_level)) in self.pre_roll.drain(..).enumerate() {
                let mut output_flags = FLAG_SPEECH_ACTIVE;
                if index == 0 {
                    output_flags |= FLAG_SPEECH_STARTED;
                }
                if index == last {
                    output_flags |= flags & (FLAG_VOICED | FLAG_FORCED_END | FLAG_SPEECH_ENDED);
                }
                self.ready.push_back(AudioFrame {
                    samples: buffered,
                    flags: output_flags,
                    level: buffered_level,
                });
            }
        } else if self.segment_active {
            self.ready.push_back(AudioFrame {
                samples: frame,
                flags,
                level,
            });
        } else {
            if self.pre_roll.len() == self.pre_roll_frames {
                self.pre_roll.pop_front();
            }
            self.pre_roll.push_back((frame, level));
        }

        if flags & FLAG_SPEECH_ENDED != 0 {
            self.segment_active = false;
            self.pre_roll.clear();
            self.model = silerovad::SileroVad::new(SILERO_WEIGHTS);
            self.workspace = silerovad::SileroVadWorkspace::new();
            self.model_state.fill(0.0);
            self.model_silence_run = 0;
            self.noise_floor = INITIAL_NOISE_FLOOR;
            self.enhanced_recovered_from_hum = false;
            self.enhanced_acoustic_override = false;
            self.enhanced_acoustic_voice = false;
        }
    }

    fn next_frame(&mut self) -> u32 {
        let Some(frame) = self.ready.pop_front() else {
            return 0;
        };
        self.output = frame.samples;
        self.output_level = frame.level;
        FLAG_FRAME_READY | frame.flags
    }

    fn reset(&mut self) {
        self.gate.reset();
        self.model = silerovad::SileroVad::new(SILERO_WEIGHTS);
        self.workspace = silerovad::SileroVadWorkspace::new();
        self.model_state.fill(0.0);
        self.noise_floor = INITIAL_NOISE_FLOOR;
        self.model_silence_run = 0;
        self.segment_model_frames = 0;
        self.completed_model_frames = 0;
        self.enhanced_calibration_frames = if self.enhanced_voice_filter {
            ENHANCED_CALIBRATION_FRAMES
        } else {
            0
        };
        self.enhanced_hum_locked = false;
        self.enhanced_hum_run = 0;
        self.enhanced_hum_clear_run = 0;
        self.enhanced_recovered_from_hum = false;
        self.enhanced_acoustic_override = false;
        self.enhanced_acoustic_voice = false;
        self.output.fill(0);
        self.output_level = 0.0;
        self.input_level = 0.0;
        self.resample_source.clear();
        self.resample_position = 0.0;
        self.frame.fill(0);
        self.frame_offset = 0;
        self.pre_roll.clear();
        self.ready.clear();
        self.segment_active = false;
    }
}

fn float_to_pcm16(sample: f32) -> i16 {
    let sample = sample.clamp(-1.0, 1.0);
    if sample < 0.0 {
        (sample * 32_768.0) as i16
    } else {
        (sample * 32_767.0) as i16
    }
}

fn rms_level(frame: &[i16]) -> f32 {
    let mean = frame.iter().map(|sample| *sample as f64).sum::<f64>() / frame.len() as f64;
    let sum = frame
        .iter()
        .map(|sample| {
            let normalized = (*sample as f64 - mean) / i16::MAX as f64;
            normalized * normalized
        })
        .sum::<f64>();
    (sum / frame.len() as f64).sqrt() as f32
}

fn zero_crossing_rate(frame: &[i16]) -> f32 {
    if frame.len() < 2 {
        return 0.0;
    }
    let mean = frame.iter().map(|sample| *sample as f64).sum::<f64>() / frame.len() as f64;
    let mut crossings = 0_usize;
    let mut previous = frame[0] as f64 - mean;
    for sample in &frame[1..] {
        let current = *sample as f64 - mean;
        if (previous < 0.0 && current >= 0.0) || (previous >= 0.0 && current < 0.0) {
            crossings += 1;
        }
        previous = current;
    }
    crossings as f32 / (frame.len() - 1) as f32
}

#[unsafe(no_mangle)]
pub extern "C" fn voice_elf_audio_create(
    max_utterance_seconds: u32,
    input_sample_rate: u32,
    enhanced_voice_filter: u32,
) -> *mut c_void {
    BrowserAudioVad::new(
        max_utterance_seconds,
        input_sample_rate,
        enhanced_voice_filter != 0,
    )
    .map(|instance| Box::into_raw(Box::new(instance)).cast())
    .unwrap_or(std::ptr::null_mut())
}

/// # Safety
///
/// `instance` must be a live pointer returned by `voice_elf_audio_create`, and
/// it must not be destroyed more than once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_destroy(instance: *mut c_void) {
    if !instance.is_null() {
        drop(unsafe { Box::from_raw(instance.cast::<BrowserAudioVad>()) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn voice_elf_audio_input_capacity() -> usize {
    MAX_INPUT_SAMPLES
}

/// # Safety
///
/// `instance` must be a live pointer returned by `voice_elf_audio_create`. The
/// returned buffer can hold `voice_elf_audio_input_capacity` f32 samples.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_input_ptr(instance: *mut c_void) -> *mut f32 {
    let Some(instance) = (unsafe { instance.cast::<BrowserAudioVad>().as_mut() }) else {
        return std::ptr::null_mut();
    };
    instance.input.as_mut_ptr()
}

/// # Safety
///
/// `instance` must be live and its input buffer must contain `sample_count`
/// initialized f32 samples.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_process(
    instance: *mut c_void,
    sample_count: usize,
) -> u32 {
    let Some(instance) = (unsafe { instance.cast::<BrowserAudioVad>().as_mut() }) else {
        return FLAG_INVALID_INPUT;
    };
    if instance.process_input(sample_count) {
        0
    } else {
        FLAG_INVALID_INPUT
    }
}

/// # Safety
///
/// `instance` must be a live pointer returned by `voice_elf_audio_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_next(instance: *mut c_void) -> u32 {
    let Some(instance) = (unsafe { instance.cast::<BrowserAudioVad>().as_mut() }) else {
        return FLAG_INVALID_INPUT;
    };
    instance.next_frame()
}

/// # Safety
///
/// `instance` must be live. The returned 512-sample PCM16 buffer contains the
/// frame selected by the last successful `voice_elf_audio_next` call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_output_ptr(instance: *mut c_void) -> *const i16 {
    let Some(instance) = (unsafe { instance.cast::<BrowserAudioVad>().as_ref() }) else {
        return std::ptr::null();
    };
    instance.output.as_ptr()
}

/// # Safety
///
/// `instance` must be a live pointer returned by `voice_elf_audio_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_output_level(instance: *mut c_void) -> f32 {
    unsafe {
        instance
            .cast::<BrowserAudioVad>()
            .as_ref()
            .map(|instance| instance.output_level)
            .unwrap_or(0.0)
    }
}

/// Returns the RMS level of the latest complete microphone frame, including frames that VAD
/// classified as silence. This keeps the capture meter independent from speech decisions.
///
/// # Safety
///
/// `instance` must be a live pointer returned by `voice_elf_audio_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_input_level(instance: *mut c_void) -> f32 {
    unsafe {
        instance
            .cast::<BrowserAudioVad>()
            .as_ref()
            .map(|instance| instance.input_level)
            .unwrap_or(0.0)
    }
}

/// Returns Silero-confirmed speech frames for the active or most recently completed segment.
///
/// # Safety
///
/// `instance` must be a live pointer returned by `voice_elf_audio_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_segment_speech_frames(instance: *mut c_void) -> u32 {
    unsafe {
        instance
            .cast::<BrowserAudioVad>()
            .as_ref()
            .map(|instance| {
                if instance.segment_active {
                    instance.segment_model_frames
                } else {
                    instance.completed_model_frames
                }
            })
            .unwrap_or(0)
    }
}

/// # Safety
///
/// `instance` must be a live pointer returned by `voice_elf_audio_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn voice_elf_audio_reset(instance: *mut c_void) {
    if let Some(instance) = unsafe { instance.cast::<BrowserAudioVad>().as_mut() } {
        instance.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_after_a_stable_voiced_run() {
        let mut gate = SpeechGate::new(20, false);
        for _ in 0..START_TRIGGER_FRAMES - 1 {
            assert_eq!(gate.process(true) & FLAG_SPEECH_STARTED, 0);
        }
        let flags = gate.process(true);
        assert_ne!(flags & FLAG_SPEECH_STARTED, 0);
        assert_ne!(flags & FLAG_SPEECH_ACTIVE, 0);
    }

    #[test]
    fn enhanced_filter_uses_confirmed_model_frames() {
        let mut gate = SpeechGate::new(20, true);
        assert_ne!(gate.process(true) & FLAG_SPEECH_STARTED, 0);
    }

    #[test]
    fn energy_without_silero_probability_cannot_start() {
        let mut vad = BrowserAudioVad::new(20, 16_000, false).unwrap();
        let (voiced, model_voiced) = vad.classify_voiced(0.0, 0.5, true, false);
        assert!(!voiced);
        assert!(!model_voiced);
        assert_eq!(vad.gate.process(voiced) & FLAG_SPEECH_STARTED, 0);
    }

    #[test]
    fn ends_after_silence_hangover() {
        let mut gate = SpeechGate::new(20, false);
        for _ in 0..START_TRIGGER_FRAMES {
            gate.process(true);
        }
        for _ in 0..END_SILENCE_FRAMES - 1 {
            assert_eq!(gate.process(false) & FLAG_SPEECH_ENDED, 0);
        }
        assert_ne!(gate.process(false) & FLAG_SPEECH_ENDED, 0);
    }

    #[test]
    fn force_ends_at_maximum_duration() {
        let mut gate = SpeechGate::new(5, false);
        for _ in 0..START_TRIGGER_FRAMES {
            gate.process(true);
        }
        let mut flags = gate.process(true);
        for _ in 0..313 {
            if flags & FLAG_SPEECH_ENDED != 0 {
                break;
            }
            flags = gate.process(true);
        }
        assert_ne!(flags & FLAG_SPEECH_ENDED, 0);
        assert_ne!(flags & FLAG_FORCED_END, 0);
        assert!(gate.accepts_active_energy());
        for _ in 0..START_TRIGGER_FRAMES - 1 {
            assert_eq!(gate.process(true) & FLAG_SPEECH_STARTED, 0);
        }
        let next = gate.process(true);
        assert_ne!(next & FLAG_SPEECH_STARTED, 0);
        assert_ne!(next & FLAG_SPEECH_ACTIVE, 0);
    }

    #[test]
    fn centered_level_rejects_microphone_dc_offset() {
        assert!(rms_level(&[8_000; FRAME_SAMPLES]) < 0.000_1);
        let mut alternating = [0_i16; FRAME_SAMPLES];
        for (index, sample) in alternating.iter_mut().enumerate() {
            *sample = if index % 2 == 0 { 8_000 } else { -8_000 };
        }
        assert!(rms_level(&alternating) > 0.2);
    }

    #[test]
    fn broadband_noise_is_not_speech_like() {
        let mut state = 0x1234_5678_u32;
        let mut noise = [0_i16; FRAME_SAMPLES];
        for sample in &mut noise {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *sample = ((state >> 16) as i16) / 2;
        }
        assert!(zero_crossing_rate(&noise) > MAX_SPEECH_ZERO_CROSSING_RATE);

        let mut vad = BrowserAudioVad::new(20, 16_000, false).unwrap();
        let (voiced, model_voiced) =
            vad.classify_voiced(SPEECH_THRESHOLD + 0.5, rms_level(&noise), false, false);
        assert!(!voiced);
        assert!(!model_voiced);
    }

    #[test]
    fn ordinary_voiced_waveform_is_speech_like() {
        let mut voiced = [0_i16; FRAME_SAMPLES];
        for (index, sample) in voiced.iter_mut().enumerate() {
            *sample = ((index as f32 * 2.0 * std::f32::consts::PI * 220.0 / SAMPLE_RATE as f32)
                .sin()
                * 8_000.0) as i16;
        }
        assert!(zero_crossing_rate(&voiced) < MAX_SPEECH_ZERO_CROSSING_RATE);
    }

    #[test]
    fn enhanced_filter_rejects_loud_low_frequency_hum() {
        let mut hum = [0_i16; FRAME_SAMPLES];
        for (index, sample) in hum.iter_mut().enumerate() {
            *sample = ((index as f32 * 2.0 * std::f32::consts::PI * 50.0 / SAMPLE_RATE as f32)
                .sin()
                * 12_000.0) as i16;
        }
        let mut vad = BrowserAudioVad::new(20, 16_000, true).unwrap();
        assert!(!vad.frame_is_speech_like(&hum, rms_level(&hum)));
        let (voiced, model_voiced) =
            vad.classify_voiced(SPEECH_THRESHOLD + 0.5, rms_level(&hum), false, false);
        assert!(!voiced);
        assert!(!model_voiced);
    }

    #[test]
    fn enhanced_filter_unlocks_when_speech_structure_returns() {
        let mut hum = [0_i16; FRAME_SAMPLES];
        let mut speech = [0_i16; FRAME_SAMPLES];
        for index in 0..FRAME_SAMPLES {
            hum[index] = ((index as f32 * 2.0 * std::f32::consts::PI * 50.0 / SAMPLE_RATE as f32)
                .sin()
                * 12_000.0) as i16;
            speech[index] =
                ((index as f32 * 2.0 * std::f32::consts::PI * 300.0 / SAMPLE_RATE as f32).sin()
                    * 8_000.0) as i16;
        }
        let mut vad = BrowserAudioVad::new(20, 16_000, true).unwrap();
        for _ in 0..ENHANCED_CALIBRATION_FRAMES {
            assert!(!vad.frame_is_speech_like(&hum, rms_level(&hum)));
        }
        assert!(vad.enhanced_hum_locked);
        for _ in 0..ENHANCED_HUM_CLEAR_FRAMES - 1 {
            assert!(!vad.frame_is_speech_like(&speech, rms_level(&speech)));
        }
        assert!(vad.frame_is_speech_like(&speech, rms_level(&speech)));
        assert!(!vad.enhanced_hum_locked);
        assert!(vad.enhanced_acoustic_override);
        let acoustic_override = vad.enhanced_acoustic_override;
        let (voiced, confirmed) =
            vad.classify_voiced(0.0, rms_level(&speech), true, acoustic_override);
        assert!(voiced);
        assert!(confirmed);
    }

    #[test]
    fn enhanced_filter_ignores_quiet_false_starts() {
        let mut vad = BrowserAudioVad::new(20, 16_000, true).unwrap();
        let (voiced, model_voiced) =
            vad.classify_voiced(SPEECH_THRESHOLD + 0.5, 0.006, true, false);
        assert!(!voiced);
        assert!(!model_voiced);
    }

    #[test]
    fn ambient_noise_raises_the_energy_gate() {
        let mut vad = BrowserAudioVad::new(20, 16_000, false).unwrap();
        for _ in 0..120 {
            vad.update_noise_floor(0.02);
        }
        assert!(vad.noise_floor > 0.011);
        assert!(vad.noise_floor <= MAX_NOISE_FLOOR);
        assert!(vad.noise_floor * START_SIGNAL_TO_NOISE < 0.017);
    }

    #[test]
    fn replays_half_a_second_before_the_start_boundary() {
        let mut vad = BrowserAudioVad::new(20, 16_000, false).unwrap();
        for marker in 1..=PRE_ROLL_FRAMES {
            vad.route_frame([marker as i16; FRAME_SAMPLES], 0.01, 0);
        }
        vad.route_frame(
            [99; FRAME_SAMPLES],
            0.1,
            FLAG_SPEECH_STARTED | FLAG_SPEECH_ACTIVE | FLAG_VOICED,
        );

        assert_eq!(vad.ready.len(), PRE_ROLL_FRAMES + 1);
        assert_eq!(vad.ready.front().unwrap().samples[0], 1);
        assert_ne!(vad.ready.front().unwrap().flags & FLAG_SPEECH_STARTED, 0);
        assert_eq!(vad.ready.back().unwrap().samples[0], 99);
    }

    #[test]
    fn high_background_energy_cannot_block_the_next_segment() {
        let mut vad = BrowserAudioVad::new(20, 16_000, false).unwrap();
        let frame = [1_000; FRAME_SAMPLES];

        for _ in 0..START_TRIGGER_FRAMES {
            let (voiced, _) = vad.classify_voiced(SPEECH_THRESHOLD + 0.1, 0.05, true, false);
            let flags = vad.gate.process(voiced);
            vad.route_frame(frame, 0.05, flags);
        }
        assert!(vad.segment_active);

        let mut ended = false;
        for _ in 0..ENERGY_HANGOVER_FRAMES + END_SILENCE_FRAMES + 1 {
            let (voiced, _) = vad.classify_voiced(0.0, 0.05, true, false);
            let flags = vad.gate.process(voiced);
            ended |= flags & FLAG_SPEECH_ENDED != 0;
            vad.route_frame(frame, 0.05, flags);
        }
        assert!(ended);
        assert!(!vad.segment_active);

        let mut restarted = false;
        for _ in 0..START_TRIGGER_FRAMES {
            let (voiced, _) = vad.classify_voiced(SPEECH_THRESHOLD + 0.1, 0.05, true, false);
            let flags = vad.gate.process(voiced);
            restarted |= flags & FLAG_SPEECH_STARTED != 0;
            vad.route_frame(frame, 0.05, flags);
        }
        assert!(restarted);
        assert!(vad.segment_active);
    }

    #[test]
    fn resamples_native_float_audio_and_builds_fixed_frames() {
        let mut vad = BrowserAudioVad::new(20, 48_000, false).unwrap();
        vad.input.fill(0.25);
        assert!(vad.process_input(1_536));
        assert_eq!(vad.frame_offset, 0);
        assert_eq!(vad.pre_roll.len(), 1);
        assert!(vad.pre_roll.iter().all(|(frame, _)| frame[0] > 8_000));
    }

    #[test]
    fn reports_input_level_before_speech_is_accepted() {
        let mut vad = BrowserAudioVad::new(20, 16_000, false).unwrap();
        for (index, sample) in vad.input[..1_024].iter_mut().enumerate() {
            *sample = if index % 2 == 0 { 0.25 } else { -0.25 };
        }
        assert!(vad.process_input(1_024));
        assert!(vad.input_level > 0.2);
        assert!(!vad.segment_active);
    }

    #[test]
    fn rejects_invalid_input_rates_and_chunk_lengths() {
        assert!(BrowserAudioVad::new(20, 1_000, false).is_none());
        let mut vad = BrowserAudioVad::new(20, 48_000, false).unwrap();
        assert!(!vad.process_input(0));
        assert!(!vad.process_input(MAX_INPUT_SAMPLES + 1));
    }
}
