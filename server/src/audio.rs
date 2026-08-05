pub fn pcm16_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

pub const QUALITY_FRAME_SAMPLES: usize = 512;
const VOICE_HIGHPASS_HZ: f64 = 90.0;
const MIN_AUDIBLE_FRAME_RMS: f32 = 0.01;
const MIN_SPEECH_ZERO_CROSSING_RATE: f32 = 0.015;
const MAX_SPEECH_ZERO_CROSSING_RATE: f32 = 0.32;
const MIN_AUDIBLE_SPEECH_FRAMES: u32 = 12;

#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    fn highpass(sample_rate: u32, frequency: f64, q: f64) -> Self {
        let omega = 2.0 * std::f64::consts::PI * frequency / sample_rate as f64;
        let cosine = omega.cos();
        let alpha = omega.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cosine) / 2.0) / a0,
            b1: -(1.0 + cosine) / a0,
            b2: ((1.0 + cosine) / 2.0) / a0,
            a1: (-2.0 * cosine) / a0,
            a2: (1.0 - alpha) / a0,
            ..Self::default()
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }
}

pub struct VoiceAudioProcessor {
    highpass: [Biquad; 2],
    processed_samples: usize,
    fade_in_samples: usize,
}

impl VoiceAudioProcessor {
    pub fn new(sample_rate: u32) -> Self {
        // Cascaded Q values form a flat fourth-order Butterworth response.
        Self {
            highpass: [
                Biquad::highpass(sample_rate, VOICE_HIGHPASS_HZ, 0.541_196_1),
                Biquad::highpass(sample_rate, VOICE_HIGHPASS_HZ, 1.306_563),
            ],
            processed_samples: 0,
            fade_in_samples: (sample_rate as usize / 100).max(1),
        }
    }

    pub fn process(&mut self, samples: &mut [i16]) {
        for sample in samples {
            let mut filtered = *sample as f64;
            for stage in &mut self.highpass {
                filtered = stage.process(filtered);
            }
            let fade = (self.processed_samples as f64 / self.fade_in_samples as f64).min(1.0);
            filtered *= fade;
            self.processed_samples = self.processed_samples.saturating_add(1);
            *sample = filtered.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16;
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SpeechQuality {
    total_frames: u32,
    audible_speech_frames: u32,
    peak_frame_rms: f32,
}

impl SpeechQuality {
    pub fn observe(&mut self, samples: &[i16]) {
        for frame in samples.chunks_exact(QUALITY_FRAME_SAMPLES) {
            self.observe_frame(frame);
        }
    }

    pub fn accepts_asr(&self) -> bool {
        self.audible_speech_frames >= MIN_AUDIBLE_SPEECH_FRAMES
    }

    pub fn summary(&self) -> String {
        format!(
            "可听人声帧 {}/{}，峰值帧 RMS {:.4}",
            self.audible_speech_frames, self.total_frames, self.peak_frame_rms
        )
    }

    fn observe_frame(&mut self, frame: &[i16]) {
        self.total_frames = self.total_frames.saturating_add(1);
        let (rms, crossing_rate) = frame_features(frame);
        self.peak_frame_rms = self.peak_frame_rms.max(rms);
        if rms >= MIN_AUDIBLE_FRAME_RMS
            && (MIN_SPEECH_ZERO_CROSSING_RATE..=MAX_SPEECH_ZERO_CROSSING_RATE)
                .contains(&crossing_rate)
        {
            self.audible_speech_frames = self.audible_speech_frames.saturating_add(1);
        }
    }
}

pub fn assess_speech_quality(samples: &[i16]) -> SpeechQuality {
    let mut quality = SpeechQuality::default();
    quality.observe(samples);
    quality
}

fn frame_features(frame: &[i16]) -> (f32, f32) {
    if frame.len() < 2 {
        return (0.0, 0.0);
    }
    let mean = frame.iter().map(|sample| *sample as f64).sum::<f64>() / frame.len() as f64;
    let mut squared_sum = 0.0;
    let mut crossings = 0_usize;
    let mut previous = frame[0] as f64 - mean;
    for sample in frame {
        let centered = *sample as f64 - mean;
        squared_sum += centered * centered;
    }
    for sample in &frame[1..] {
        let current = *sample as f64 - mean;
        if (previous < 0.0 && current >= 0.0) || (previous >= 0.0 && current < 0.0) {
            crossings += 1;
        }
        previous = current;
    }
    (
        (squared_sum / frame.len() as f64).sqrt() as f32 / i16::MAX as f32,
        crossings as f32 / (frame.len() - 1) as f32,
    )
}

pub fn pcm16_wav_bytes(samples: &[i16], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    {
        let cursor = std::io::Cursor::new(&mut bytes);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(cursor, spec)?;
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_pcm16_as_little_endian() {
        assert_eq!(pcm16_bytes(&[0x1234, -2]), [0x34, 0x12, 0xfe, 0xff]);
    }

    #[test]
    fn serializes_pcm16_as_a_readable_wav() {
        let bytes = pcm16_wav_bytes(&[0, 1, -1], 16_000).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.duration(), 3);
    }

    #[test]
    fn accepts_sustained_audible_speech_shaped_audio() {
        let mut quality = SpeechQuality::default();
        for frame_index in 0..MIN_AUDIBLE_SPEECH_FRAMES {
            let frame = (0..QUALITY_FRAME_SAMPLES)
                .map(|sample_index| {
                    let phase = (frame_index as usize * QUALITY_FRAME_SAMPLES + sample_index)
                        as f32
                        * 2.0
                        * std::f32::consts::PI
                        * 220.0
                        / 16_000.0;
                    (phase.sin() * 1_200.0) as i16
                })
                .collect::<Vec<_>>();
            quality.observe(&frame);
        }
        assert!(quality.accepts_asr(), "{}", quality.summary());
    }

    #[test]
    fn rejects_inaudible_audio_even_when_it_is_speech_shaped() {
        let samples = (0..QUALITY_FRAME_SAMPLES * 40)
            .map(|index| {
                let phase = index as f32 * 2.0 * std::f32::consts::PI * 220.0 / 16_000.0;
                (phase.sin() * 300.0) as i16
            })
            .collect::<Vec<_>>();
        assert!(!assess_speech_quality(&samples).accepts_asr());
    }

    #[test]
    fn rejects_sustained_hum_and_broadband_noise() {
        let hum = (0..QUALITY_FRAME_SAMPLES * 40)
            .map(|index| {
                let phase = index as f32 * 2.0 * std::f32::consts::PI * 50.0 / 16_000.0;
                (phase.sin() * 8_000.0) as i16
            })
            .collect::<Vec<_>>();
        assert!(!assess_speech_quality(&hum).accepts_asr());

        let mut state = 0x1234_5678_u32;
        let noise = (0..QUALITY_FRAME_SAMPLES * 40)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 16) as i16
            })
            .collect::<Vec<_>>();
        assert!(!assess_speech_quality(&noise).accepts_asr());
    }

    fn sine_wave(frequency: f32, amplitude: f32, seconds: usize) -> Vec<i16> {
        (0..16_000 * seconds)
            .map(|index| {
                let phase = index as f32 * 2.0 * std::f32::consts::PI * frequency / 16_000.0;
                (phase.sin() * amplitude) as i16
            })
            .collect()
    }

    fn rms(samples: &[i16]) -> f64 {
        (samples
            .iter()
            .map(|sample| (*sample as f64).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }

    #[test]
    fn voice_cleanup_suppresses_mains_hum_without_thinning_speech_band() {
        let mut hum = sine_wave(50.0, 8_000.0, 2);
        let hum_before = rms(&hum[16_000..]);
        VoiceAudioProcessor::new(16_000).process(&mut hum);
        let hum_after = rms(&hum[16_000..]);
        assert!(hum_after < hum_before * 0.12);

        let mut speech = sine_wave(300.0, 4_000.0, 2);
        let speech_before = rms(&speech[16_000..]);
        VoiceAudioProcessor::new(16_000).process(&mut speech);
        let speech_after = rms(&speech[16_000..]);
        assert!(speech_after > speech_before * 0.98);
    }

    #[test]
    fn voice_cleanup_keeps_filter_state_across_websocket_frames() {
        let mut whole = sine_wave(120.0, 4_000.0, 1);
        let mut chunked = whole.clone();
        VoiceAudioProcessor::new(16_000).process(&mut whole);
        let mut processor = VoiceAudioProcessor::new(16_000);
        for frame in chunked.chunks_mut(QUALITY_FRAME_SAMPLES) {
            processor.process(frame);
        }
        assert_eq!(chunked, whole);
    }
}
