const MIN_DURATION_SECONDS = 3;
const MAX_DURATION_SECONDS = 15;
const MAX_SOURCE_BYTES = 25 * 1024 * 1024;
const TARGET_SAMPLE_RATE = 24_000;

export interface PreparedVoiceReference {
  wav: Blob;
  durationSeconds: number;
}

export async function prepareVoiceReference(source: Blob): Promise<PreparedVoiceReference> {
  if (source.size > MAX_SOURCE_BYTES) throw new Error('音频文件不能超过 25 MB');
  const context = new AudioContext();
  try {
    const decoded = await context.decodeAudioData(await source.arrayBuffer());
    if (decoded.duration < MIN_DURATION_SECONDS || decoded.duration > MAX_DURATION_SECONDS + 0.25) {
      throw new Error('参考音频时长必须为 3 到 15 秒');
    }
    const frameCount = Math.round(
      Math.min(decoded.duration, MAX_DURATION_SECONDS) * TARGET_SAMPLE_RATE,
    );
    const renderer = new OfflineAudioContext(1, frameCount, TARGET_SAMPLE_RATE);
    const sourceNode = renderer.createBufferSource();
    sourceNode.buffer = decoded;
    sourceNode.connect(renderer.destination);
    sourceNode.start();
    const rendered = await renderer.startRendering();
    const samples = rendered.getChannelData(0);
    const rms = Math.sqrt(
      samples.reduce((total, sample) => total + sample * sample, 0) / samples.length,
    );
    if (rms < 0.005) throw new Error('参考音频音量过低，请重新录制');
    return {
      wav: encodeMonoPcm16Wav(samples, TARGET_SAMPLE_RATE),
      durationSeconds: rendered.duration,
    };
  } catch (error) {
    if (error instanceof Error && error.message.startsWith('参考音频')) throw error;
    throw new Error('无法读取该音频，请选择常见音频格式');
  } finally {
    void context.close();
  }
}

function encodeMonoPcm16Wav(samples: Float32Array, sampleRate: number) {
  const buffer = new ArrayBuffer(44 + samples.length * 2);
  const view = new DataView(buffer);
  writeAscii(view, 0, 'RIFF');
  view.setUint32(4, 36 + samples.length * 2, true);
  writeAscii(view, 8, 'WAVE');
  writeAscii(view, 12, 'fmt ');
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  writeAscii(view, 36, 'data');
  view.setUint32(40, samples.length * 2, true);
  for (let index = 0; index < samples.length; index += 1) {
    const sample = Math.max(-1, Math.min(1, samples[index]));
    view.setInt16(44 + index * 2, sample < 0 ? sample * 0x8000 : sample * 0x7fff, true);
  }
  return new Blob([buffer], { type: 'audio/wav' });
}

function writeAscii(view: DataView, offset: number, value: string) {
  for (let index = 0; index < value.length; index += 1) {
    view.setUint8(offset + index, value.charCodeAt(index));
  }
}
