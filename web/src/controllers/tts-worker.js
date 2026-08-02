import * as ort from 'onnxruntime-web';
import ortWasmPath from '../../node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.jsep.wasm?url';
import { loadTextToSpeech, loadVoiceStyle } from './supertonic-helper.js';

const MODEL_BASE = '/models/supertonic3';
ort.env.wasm.wasmPaths = { wasm: ortWasmPath };
let runtimePromise = null;
const styles = new Map();

function progress(id, value) {
  self.postMessage({ type: 'progress', id, value });
}

async function createRuntime(id) {
  const onLoad = (_name, current, total) => progress(id, Math.round((current / total) * 38));
  const gpuAdapter = 'gpu' in navigator
    ? await navigator.gpu.requestAdapter().catch(() => null)
    : null;
  if (!gpuAdapter) {
    const result = await loadTextToSpeech(`${MODEL_BASE}/onnx`, {
      executionProviders: ['wasm'], graphOptimizationLevel: 'all',
    }, onLoad);
    return { ...result, backend: 'wasm' };
  }
  try {
    const result = await loadTextToSpeech(`${MODEL_BASE}/onnx`, {
      executionProviders: ['webgpu'], graphOptimizationLevel: 'all',
    }, onLoad);
    return { ...result, backend: 'webgpu' };
  } catch (error) {
    console.info('Supertonic WebGPU unavailable; using WASM.', error);
    const result = await loadTextToSpeech(`${MODEL_BASE}/onnx`, {
      executionProviders: ['wasm'], graphOptimizationLevel: 'all',
    }, onLoad);
    return { ...result, backend: 'wasm' };
  }
}

async function getStyle(name) {
  if (!styles.has(name)) {
    styles.set(name, loadVoiceStyle([`${MODEL_BASE}/voice_styles/${name}.json`]));
  }
  return styles.get(name);
}

self.onmessage = async (event) => {
  if (event.data.type !== 'synthesize') return;
  const { id, text, language, voice } = event.data;
  try {
    runtimePromise ??= createRuntime(id);
    const [{ textToSpeech, backend }, style] = await Promise.all([
      runtimePromise, getStyle(voice === 'F1' ? 'F1' : 'M1'),
    ]);
    progress(id, 45);
    const started = performance.now();
    const result = await textToSpeech.call(text, language, style, 8, 1.05, 0.3,
      (step, total) => progress(id, 45 + Math.round((step / total) * 48)));
    const sampleCount = Math.min(result.wav.length,
      Math.floor(textToSpeech.sampleRate * result.duration[0]));
    const pcm = new Int16Array(sampleCount);
    for (let index = 0; index < sampleCount; index += 1) {
      const value = Math.max(-1, Math.min(1, result.wav[index]));
      pcm[index] = Math.round(value * (value < 0 ? 32768 : 32767));
    }
    self.postMessage({ type: 'result', id, pcm: pcm.buffer,
      sampleRate: textToSpeech.sampleRate,
      synthesisMs: Math.round(performance.now() - started), backend }, [pcm.buffer]);
  } catch (error) {
    self.postMessage({ type: 'error', id,
      message: error instanceof Error ? error.message : String(error) });
  }
};
