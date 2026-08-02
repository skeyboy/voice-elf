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

const START_TRIGGER_FRAMES: u16 = 1;
const END_SILENCE_FRAMES: u16 = 14;
const PRE_ROLL_FRAMES: usize = 7;
const FORCED_CONTINUATION_FRAMES: u16 = 6;
// Lele's wasm32 path produces lower Silero probabilities than its native path
// for the same input, so this threshold is calibrated against the final WASM.
const SPEECH_THRESHOLD: f32 = 0.1;
const START_LEVEL_THRESHOLD: f32 = 0.005;
const ACTIVE_LEVEL_THRESHOLD: f32 = 0.008;
const INITIAL_NOISE_FLOOR: f32 = 0.002;
const START_SIGNAL_TO_NOISE: f32 = 2.8;
const ACTIVE_SIGNAL_TO_NOISE: f32 = 1.8;

pub struct SpeechGate {
    speaking: bool,
    voiced_run: u16,
    silence_run: u16,
    active_frames: u32,
    max_active_frames: u32,
    continuation_frames: u16,
}

impl SpeechGate {
    pub fn new(max_utterance_seconds: u32) -> Self {
        Self {
            speaking: false,
            voiced_run: 0,
            silence_run: 0,
            active_frames: 0,
            max_active_frames: max_utterance_seconds.clamp(5, 120) * 1_000 / 32,
            continuation_frames: 0,
        }
    }

    pub fn process(&mut self, voiced: bool) -> u32 {
        let mut flags = if voiced { FLAG_VOICED } else { 0 };
        if !self.speaking {
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            if self.voiced_run >= START_TRIGGER_FRAMES {
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
    input: [f32; MAX_INPUT_SAMPLES],
    output: [i16; FRAME_SAMPLES],
    output_level: f32,
    resample_source: Vec<f32>,
    resample_position: f64,
    resample_ratio: f64,
    frame: [i16; FRAME_SAMPLES],
    frame_offset: usize,
    pre_roll: VecDeque<([i16; FRAME_SAMPLES], f32)>,
    ready: VecDeque<AudioFrame>,
    segment_active: bool,
}

impl BrowserAudioVad {
    fn new(max_utterance_seconds: u32, input_sample_rate: u32) -> Option<Self> {
        if !(8_000..=192_000).contains(&input_sample_rate) {
            return None;
        }
        Some(Self {
            model: silerovad::SileroVad::new(SILERO_WEIGHTS),
            workspace: silerovad::SileroVadWorkspace::new(),
            model_state: vec![0.0; 2 * 128],
            gate: SpeechGate::new(max_utterance_seconds),
            noise_floor: INITIAL_NOISE_FLOOR,
            input: [0.0; MAX_INPUT_SAMPLES],
            output: [0; FRAME_SAMPLES],
            output_level: 0.0,
            resample_source: Vec::with_capacity(MAX_INPUT_SAMPLES + 8),
            resample_position: 0.0,
            resample_ratio: input_sample_rate as f64 / SAMPLE_RATE as f64,
            frame: [0; FRAME_SAMPLES],
            frame_offset: 0,
            pre_roll: VecDeque::with_capacity(PRE_ROLL_FRAMES + 1),
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
        let probability = self.predict_speech(&frame);
        if !self.gate.accepts_active_energy() && probability < SPEECH_THRESHOLD {
            self.update_noise_floor(level);
        }
        let start_level = START_LEVEL_THRESHOLD.max(self.noise_floor * START_SIGNAL_TO_NOISE);
        let active_level = ACTIVE_LEVEL_THRESHOLD.max(self.noise_floor * ACTIVE_SIGNAL_TO_NOISE);
        let model_voiced = probability >= SPEECH_THRESHOLD && level >= start_level;
        let voiced = model_voiced || (self.gate.accepts_active_energy() && level >= active_level);
        let flags = self.gate.process(voiced);
        self.route_frame(frame, level, flags);
    }

    fn update_noise_floor(&mut self, level: f32) {
        let observed = level.clamp(0.000_2, 0.08);
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
            if self.pre_roll.len() == PRE_ROLL_FRAMES {
                self.pre_roll.pop_front();
            }
            self.pre_roll.push_back((frame, level));
        }

        if flags & FLAG_SPEECH_ENDED != 0 {
            self.segment_active = false;
            self.pre_roll.clear();
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
        self.model_state.fill(0.0);
        self.noise_floor = INITIAL_NOISE_FLOOR;
        self.output.fill(0);
        self.output_level = 0.0;
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

#[unsafe(no_mangle)]
pub extern "C" fn voice_elf_audio_create(
    max_utterance_seconds: u32,
    input_sample_rate: u32,
) -> *mut c_void {
    BrowserAudioVad::new(max_utterance_seconds, input_sample_rate)
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
    fn starts_on_the_first_silero_voiced_frame() {
        let mut gate = SpeechGate::new(20);
        let flags = gate.process(true);
        assert_ne!(flags & FLAG_SPEECH_STARTED, 0);
        assert_ne!(flags & FLAG_SPEECH_ACTIVE, 0);
    }

    #[test]
    fn ends_after_silence_hangover() {
        let mut gate = SpeechGate::new(20);
        gate.process(true);
        gate.process(true);
        for _ in 0..END_SILENCE_FRAMES - 1 {
            assert_eq!(gate.process(false) & FLAG_SPEECH_ENDED, 0);
        }
        assert_ne!(gate.process(false) & FLAG_SPEECH_ENDED, 0);
    }

    #[test]
    fn force_ends_at_maximum_duration() {
        let mut gate = SpeechGate::new(5);
        gate.process(true);
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
    fn ambient_noise_raises_the_energy_gate() {
        let mut vad = BrowserAudioVad::new(20, 16_000).unwrap();
        for _ in 0..120 {
            vad.update_noise_floor(0.02);
        }
        assert!(vad.noise_floor > 0.019);
        assert!(vad.noise_floor * START_SIGNAL_TO_NOISE > 0.05);
    }

    #[test]
    fn resamples_native_float_audio_and_builds_fixed_frames() {
        let mut vad = BrowserAudioVad::new(20, 48_000).unwrap();
        vad.input.fill(0.25);
        assert!(vad.process_input(1_536));
        assert_eq!(vad.frame_offset, 0);
        assert_eq!(vad.pre_roll.len(), 1);
        assert!(vad.pre_roll.iter().all(|(frame, _)| frame[0] > 8_000));
    }

    #[test]
    fn rejects_invalid_input_rates_and_chunk_lengths() {
        assert!(BrowserAudioVad::new(20, 1_000).is_none());
        let mut vad = BrowserAudioVad::new(20, 48_000).unwrap();
        assert!(!vad.process_input(0));
        assert!(!vad.process_input(MAX_INPUT_SAMPLES + 1));
    }
}
