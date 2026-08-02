const FRAME_SAMPLES = 512;
const FLAG_SPEECH_STARTED = 1 << 0;
const FLAG_SPEECH_ENDED = 1 << 2;
const FLAG_FRAME_READY = 1 << 30;
const FLAG_INVALID_INPUT = 1 << 31;

let wasm;
let processor = 0;
let inputPointer = 0;
let inputCapacity = 0;
let outputPointer = 0;

async function loadWasm(maxUtteranceSeconds, inputSampleRate) {
  const response = await fetch('/wasm/voice_elf_web_vad.wasm', {
    cache: 'no-store',
    credentials: 'same-origin',
  });
  if (!response.ok) throw new Error(`VAD WASM request failed (${response.status})`);
  const bytes = await response.arrayBuffer();
  const module = await WebAssembly.instantiate(bytes, {});
  wasm = module.instance.exports;
  processor = wasm.voice_elf_audio_create(maxUtteranceSeconds, inputSampleRate);
  inputCapacity = wasm.voice_elf_audio_input_capacity();
  inputPointer = wasm.voice_elf_audio_input_ptr(processor);
  outputPointer = wasm.voice_elf_audio_output_ptr(processor);
  if (!processor || !inputPointer || !outputPointer || !inputCapacity) {
    throw new Error('Audio VAD WASM initialization failed');
  }
}

function drainFrames() {
  while (true) {
    const flags = wasm.voice_elf_audio_next(processor) >>> 0;
    if (flags & FLAG_INVALID_INPUT) throw new Error('Audio VAD WASM state is invalid');
    if (!(flags & FLAG_FRAME_READY)) break;

    const pcm = new Int16Array(wasm.memory.buffer, outputPointer, FRAME_SAMPLES).slice();
    const level = wasm.voice_elf_audio_output_level(processor);
    postMessage({ type: 'level', value: Math.min(1, level * 4) });
    if (flags & FLAG_SPEECH_STARTED) postMessage({ type: 'speech_start' });
    postMessage({ type: 'pcm', payload: pcm.buffer }, [pcm.buffer]);
    if (flags & FLAG_SPEECH_ENDED) postMessage({ type: 'speech_end' });
  }
}

function processSamples(payload) {
  if (!(payload instanceof ArrayBuffer) || payload.byteLength % 4 !== 0) {
    throw new Error('Audio VAD expects Float32 microphone samples');
  }
  const samples = new Float32Array(payload);
  if (samples.length === 0 || samples.length > inputCapacity) {
    throw new Error(`AudioWorklet block exceeds WASM capacity (${samples.length})`);
  }
  new Float32Array(wasm.memory.buffer, inputPointer, samples.length).set(samples);
  const result = wasm.voice_elf_audio_process(processor, samples.length) >>> 0;
  if (result & FLAG_INVALID_INPUT) throw new Error('Audio VAD rejected the microphone block');
  drainFrames();
}

self.onmessage = async (event) => {
  try {
    if (event.data.type === 'init') {
      await loadWasm(event.data.maxUtteranceSeconds, event.data.inputSampleRate);
      postMessage({ type: 'ready' });
    } else if (event.data.type === 'samples') {
      processSamples(event.data.payload);
    } else if (event.data.type === 'flush') {
      wasm.voice_elf_audio_reset(processor);
      postMessage({ type: 'flushed' });
    }
  } catch (error) {
    postMessage({
      type: 'error',
      message: error instanceof Error ? error.message : 'Audio VAD worker failed',
    });
  }
};
