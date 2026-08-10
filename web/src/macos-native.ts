export type MacNativeAudioEvent =
  | { type: 'capture-started'; sampleRate: number }
  | { type: 'capture-stopped' }
  | { type: 'capture-error'; message: string }
  | { type: 'audio-pcm'; data: string; sampleRate: number };

declare global {
  interface Window {
    __VOICE_ELF_NATIVE_PLATFORM__?: string;
  }
}

const NATIVE_EVENT = 'voice-elf:mac-native';

export function isMacNativeShell() {
  return window.__VOICE_ELF_NATIVE_PLATFORM__ === 'macos';
}

export function subscribeMacNativeAudio(
  listener: (event: MacNativeAudioEvent) => void,
) {
  const handler = (event: Event) => {
    const detail = (event as CustomEvent<MacNativeAudioEvent>).detail;
    if (detail?.type) listener(detail);
  };
  window.addEventListener(NATIVE_EVENT, handler);
  return () => window.removeEventListener(NATIVE_EVENT, handler);
}

export async function startMacSystemAudioCapture() {
  if (!isMacNativeShell()) return false;
  let unsubscribe = () => {};
  let timeout = 0;
  const cancel = () => {
    window.clearTimeout(timeout);
    unsubscribe();
  };
  const result = new Promise<void>((resolve, reject) => {
    timeout = window.setTimeout(() => {
      unsubscribe();
      reject(new Error('等待 macOS 系统音频授权超时，请重试'));
    }, 90_000);
    unsubscribe = subscribeMacNativeAudio((event) => {
      if (event.type !== 'capture-started' && event.type !== 'capture-error') return;
      window.clearTimeout(timeout);
      unsubscribe();
      if (event.type === 'capture-error') reject(new Error(event.message));
      else resolve();
    });
  });
  let response: Response;
  try {
    response = await fetch('/__voice_elf/mac-audio/start', { method: 'POST' });
  } catch (error) {
    cancel();
    throw error;
  }
  if (!response.ok) {
    cancel();
    const payload = await response.json().catch(() => null) as { error?: string } | null;
    throw new Error(payload?.error ?? '无法启动 macOS 系统内录');
  }
  await result;
  return true;
}

export async function stopMacSystemAudioCapture() {
  if (!isMacNativeShell()) return;
  await fetch('/__voice_elf/mac-audio/stop', { method: 'POST' }).catch(() => null);
}
