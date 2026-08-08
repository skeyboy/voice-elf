export type AndroidNativeEvent =
  | { type: 'capture-started'; systemAudio: boolean }
  | { type: 'capture-stopped' }
  | { type: 'capture-error'; message: string }
  | { type: 'audio-pcm'; data: string }
  | { type: 'overlay-opened' }
  | { type: 'overlay-closed' }
  | { type: 'overlay-error'; message: string };

interface AndroidVoiceElfBridge {
  platform(): string;
  startCapture(microphone: boolean, systemAudio: boolean): void;
  captureReady(): void;
  stopCapture(): void;
  showSubtitleOverlay(payload: string): void;
  updateSubtitleOverlay(payload: string): void;
  hideSubtitleOverlay(): void;
  subtitleOverlayVisible(): boolean;
}

declare global {
  interface Window {
    VoiceElfAndroid?: AndroidVoiceElfBridge;
  }
}

const NATIVE_EVENT = 'voice-elf:android-native';

export function isAndroidNativeShell() {
  try {
    return window.VoiceElfAndroid?.platform() === 'android';
  } catch {
    return false;
  }
}

export function supportsAndroidSystemAudio() {
  return isAndroidNativeShell();
}

export function subscribeAndroidNative(
  listener: (event: AndroidNativeEvent) => void,
) {
  const handler = (event: Event) => {
    const detail = (event as CustomEvent<AndroidNativeEvent>).detail;
    if (detail?.type) listener(detail);
  };
  window.addEventListener(NATIVE_EVENT, handler);
  return () => window.removeEventListener(NATIVE_EVENT, handler);
}

function waitForNativeResult(
  success: AndroidNativeEvent['type'],
  failure: AndroidNativeEvent['type'],
  timeoutMs: number,
) {
  return new Promise<void>((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      unsubscribe();
      reject(new Error('等待 Android 系统授权超时，请重试'));
    }, timeoutMs);
    const unsubscribe = subscribeAndroidNative((event) => {
      if (event.type !== success && event.type !== failure) return;
      window.clearTimeout(timeout);
      unsubscribe();
      if (event.type === failure) {
        reject(new Error('message' in event ? event.message : 'Android 原生操作失败'));
      } else {
        resolve();
      }
    });
  });
}

export async function startAndroidCapture(microphone: boolean, systemAudio: boolean) {
  const bridge = window.VoiceElfAndroid;
  if (!bridge || !isAndroidNativeShell()) return false;
  const result = waitForNativeResult('capture-started', 'capture-error', 90_000);
  bridge.startCapture(microphone, systemAudio);
  await result;
  return true;
}

export function stopAndroidCapture() {
  if (isAndroidNativeShell()) window.VoiceElfAndroid?.stopCapture();
}

export function markAndroidCaptureReady() {
  if (isAndroidNativeShell()) window.VoiceElfAndroid?.captureReady();
}

export function decodeAndroidPcm(base64: string) {
  const binary = window.atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  const input = new Int16Array(bytes.buffer);
  const samples = new Float32Array(input.length);
  for (let index = 0; index < input.length; index += 1) samples[index] = input[index] / 32768;
  return samples.buffer;
}

export interface AndroidSubtitlePayload {
  roomId: string;
  roomName: string;
  source: string;
  translation: string;
  sourceVisible: boolean;
  translationVisible: boolean;
  backgroundColor: string;
  sourceColor: string;
  translationColor: string;
}

export async function showAndroidSubtitleOverlay(payload: AndroidSubtitlePayload) {
  const bridge = window.VoiceElfAndroid;
  if (!bridge || !isAndroidNativeShell()) return false;
  const result = waitForNativeResult('overlay-opened', 'overlay-error', 90_000);
  bridge.showSubtitleOverlay(JSON.stringify(payload));
  await result;
  return true;
}

export function updateAndroidSubtitleOverlay(payload: AndroidSubtitlePayload) {
  if (isAndroidNativeShell()) {
    window.VoiceElfAndroid?.updateSubtitleOverlay(JSON.stringify(payload));
  }
}

export function hideAndroidSubtitleOverlay() {
  if (isAndroidNativeShell()) window.VoiceElfAndroid?.hideSubtitleOverlay();
}

export function isAndroidSubtitleOverlayVisible() {
  try {
    return isAndroidNativeShell() && Boolean(window.VoiceElfAndroid?.subtitleOverlayVisible());
  } catch {
    return false;
  }
}
