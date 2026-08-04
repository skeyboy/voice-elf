const FRAME_SAMPLES = 512;
const FLAG_SPEECH_STARTED = 1 << 0;
const FLAG_SPEECH_ENDED = 1 << 2;
const FLAG_FORCED_END = 1 << 4;
const FLAG_FRAME_READY = 1 << 30;
const FLAG_INVALID_INPUT = 1 << 31;
const MIN_CONFIRMED_SPEECH_FRAMES = 6;

let wasm;
let processor = 0;
let inputPointer = 0;
let inputCapacity = 0;
let outputPointer = 0;
let segmentActive = false;
let segmentAccepted = false;
let pendingFrames = [];
let suppressed = false;
let enhancedVoiceFilter = false;

function reportInitialization(stage) {
  postMessage({ type: 'initializing', stage });
}

function segmentSpeechFrames() {
  return wasm.voice_elf_audio_segment_speech_frames(processor) >>> 0;
}

async function instantiateWasm(response) {
  if (typeof WebAssembly.instantiateStreaming === 'function') {
    try {
      return await WebAssembly.instantiateStreaming(response.clone(), {});
    } catch (error) {
      if (!(error instanceof TypeError)) throw error;
    }
  }
  return WebAssembly.instantiate(await response.arrayBuffer(), {});
}

async function loadWasmRuntime() {
  if (wasm) return;
  reportInitialization('manifest');
  const manifestResponse = await fetch('/wasm/manifest.json', {
    cache: 'no-cache',
    credentials: 'same-origin',
  });
  if (!manifestResponse.ok) {
    throw new Error(`VAD manifest request failed (${manifestResponse.status})`);
  }
  const manifest = await manifestResponse.json();
  if (!/^voice_elf_web_vad\.[a-f0-9]{16}\.wasm$/.test(manifest.file ?? '')) {
    throw new Error('VAD manifest is invalid');
  }

  reportInitialization('download');
  const response = await fetch(`/wasm/${manifest.file}`, {
    cache: 'force-cache',
    credentials: 'same-origin',
  });
  if (!response.ok) throw new Error(`VAD WASM request failed (${response.status})`);
  reportInitialization('compile');
  const module = await instantiateWasm(response);
  wasm = module.instance.exports;
}

async function configureWasm(maxUtteranceSeconds, inputSampleRate, enhancedFilter) {
  await loadWasmRuntime();
  if (processor) wasm.voice_elf_audio_destroy(processor);
  processor = wasm.voice_elf_audio_create(
    maxUtteranceSeconds,
    inputSampleRate,
    enhancedFilter ? 1 : 0,
  );
  enhancedVoiceFilter = Boolean(enhancedFilter);
  inputCapacity = wasm.voice_elf_audio_input_capacity();
  inputPointer = wasm.voice_elf_audio_input_ptr(processor);
  outputPointer = wasm.voice_elf_audio_output_ptr(processor);
  if (!processor || !inputPointer || !outputPointer || !inputCapacity) {
    throw new Error('Audio VAD WASM initialization failed');
  }
  segmentActive = false;
  segmentAccepted = false;
  pendingFrames = [];
  suppressed = false;
}

function drainFrames() {
  while (true) {
    const flags = wasm.voice_elf_audio_next(processor) >>> 0;
    if (flags & FLAG_INVALID_INPUT) throw new Error('Audio VAD WASM state is invalid');
    if (!(flags & FLAG_FRAME_READY)) break;

    const pcm = new Int16Array(wasm.memory.buffer, outputPointer, FRAME_SAMPLES).slice();
    if (flags & FLAG_SPEECH_STARTED) {
      segmentActive = true;
      segmentAccepted = !enhancedVoiceFilter;
      pendingFrames = [];
      if (segmentAccepted) postMessage({ type: 'speech_start' });
    }
    if (!segmentAccepted) {
      pendingFrames.push(pcm.buffer);
      if (segmentSpeechFrames() >= MIN_CONFIRMED_SPEECH_FRAMES) {
        segmentAccepted = true;
        postMessage({ type: 'speech_start' });
        for (const pending of pendingFrames) {
          postMessage({ type: 'pcm', payload: pending }, [pending]);
        }
        pendingFrames = [];
      }
    } else {
      postMessage({ type: 'pcm', payload: pcm.buffer }, [pcm.buffer]);
    }
    if (flags & FLAG_SPEECH_ENDED) {
      segmentActive = false;
      if (segmentAccepted) {
        postMessage({
          type: 'speech_end',
          reason: flags & FLAG_FORCED_END ? 'max_duration' : 'silence',
          speechFrames: segmentSpeechFrames(),
        });
      }
      segmentAccepted = false;
      pendingFrames = [];
    }
  }
}

function processSamples(payload) {
  if (suppressed) return;
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
  const level = wasm.voice_elf_audio_input_level(processor);
  postMessage({ type: 'level', value: Math.min(1, level * 8) });
  drainFrames();
}

self.onmessage = async (event) => {
  try {
    if (event.data.type === 'init') {
      await configureWasm(
        event.data.maxUtteranceSeconds,
        event.data.inputSampleRate,
        event.data.enhancedVoiceFilter,
      );
      postMessage({ type: 'ready' });
    } else if (event.data.type === 'samples') {
      processSamples(event.data.payload);
    } else if (event.data.type === 'flush') {
      if (segmentActive && segmentAccepted) {
        postMessage({
          type: 'speech_end',
          reason: 'manual',
          speechFrames: segmentSpeechFrames(),
        });
      }
      segmentActive = false;
      segmentAccepted = false;
      pendingFrames = [];
      wasm.voice_elf_audio_reset(processor);
      postMessage({ type: 'flushed' });
    } else if (event.data.type === 'suppress') {
      suppressed = Boolean(event.data.value);
      if (segmentActive && segmentAccepted) {
        postMessage({
          type: 'speech_end',
          reason: 'manual',
          speechFrames: segmentSpeechFrames(),
        });
      }
      segmentActive = false;
      segmentAccepted = false;
      pendingFrames = [];
      wasm.voice_elf_audio_reset(processor);
    }
  } catch (error) {
    postMessage({
      type: 'error',
      message: error instanceof Error ? error.message : 'Audio VAD worker failed',
    });
  }
};
