export interface PlaybackPreferences {
  voice: string;
  autoplay: boolean;
  enhancedVoiceFilter: boolean;
  microphoneCapture: boolean;
  systemAudioCapture: boolean;
  noiseSuppression: boolean;
  echoCancellation: boolean;
}

type StoredPlaybackPreferences = Partial<PlaybackPreferences> & {
  browserAudioProcessing?: boolean;
};

const defaults: PlaybackPreferences = {
  voice: 'F1',
  autoplay: false,
  enhancedVoiceFilter: true,
  microphoneCapture: true,
  systemAudioCapture: false,
  noiseSuppression: true,
  echoCancellation: true,
};
const syncEvent = 'voice-elf:preferences-changed';
const syncChannel = 'voice-elf:preferences-sync';

function storageKey(userId: string) {
  return `voice-elf:preferences:${userId}`;
}

export function isCustomVoice(value: string) {
  return /^custom:[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
    value,
  );
}

function isProviderVoice(value: string) {
  return /^[a-z0-9][a-z0-9_-]{0,63}$/i.test(value);
}

function normalize(stored: StoredPlaybackPreferences): PlaybackPreferences {
  const legacyAudioProcessing = typeof stored.browserAudioProcessing === 'boolean'
    ? stored.browserAudioProcessing
    : true;
  return {
    voice:
      typeof stored.voice === 'string' && (isProviderVoice(stored.voice) || isCustomVoice(stored.voice))
        ? stored.voice
        : defaults.voice,
    autoplay: typeof stored.autoplay === 'boolean' ? stored.autoplay : defaults.autoplay,
    enhancedVoiceFilter:
      typeof stored.enhancedVoiceFilter === 'boolean'
        ? stored.enhancedVoiceFilter
        : defaults.enhancedVoiceFilter,
    microphoneCapture:
      typeof stored.microphoneCapture === 'boolean'
        ? stored.microphoneCapture
        : defaults.microphoneCapture,
    systemAudioCapture:
      typeof stored.systemAudioCapture === 'boolean'
        ? stored.systemAudioCapture
        : defaults.systemAudioCapture,
    noiseSuppression:
      typeof stored.noiseSuppression === 'boolean'
        ? stored.noiseSuppression
        : legacyAudioProcessing,
    echoCancellation:
      typeof stored.echoCancellation === 'boolean'
        ? stored.echoCancellation
        : legacyAudioProcessing,
  };
}

export function loadPreferences(userId: string): PlaybackPreferences {
  try {
    const stored = JSON.parse(localStorage.getItem(storageKey(userId)) ?? '{}') as StoredPlaybackPreferences;
    return normalize(stored);
  } catch {
    return { ...defaults };
  }
}

export function savePreferences(userId: string, preferences: PlaybackPreferences) {
  const normalized = normalize(preferences);
  localStorage.setItem(storageKey(userId), JSON.stringify(normalized));
  window.dispatchEvent(new CustomEvent(syncEvent, { detail: { userId, preferences: normalized } }));
  if ('BroadcastChannel' in window) {
    const channel = new BroadcastChannel(syncChannel);
    channel.postMessage({ userId, preferences: normalized });
    channel.close();
  }
  return normalized;
}

export function subscribePreferences(
  userId: string,
  callback: (preferences: PlaybackPreferences) => void,
) {
  const onStorage = (event: StorageEvent) => {
    if (event.key === storageKey(userId)) callback(loadPreferences(userId));
  };
  const onLocal = (event: Event) => {
    const detail = (
      event as CustomEvent<{ userId: string; preferences: PlaybackPreferences }>
    ).detail;
    if (detail?.userId === userId) callback(normalize(detail.preferences));
  };
  const channel = 'BroadcastChannel' in window ? new BroadcastChannel(syncChannel) : null;
  if (channel) {
    channel.onmessage = (
      event: MessageEvent<{ userId: string; preferences: PlaybackPreferences }>,
    ) => {
      if (event.data?.userId === userId) callback(normalize(event.data.preferences));
    };
  }
  window.addEventListener('storage', onStorage);
  window.addEventListener(syncEvent, onLocal);
  return () => {
    window.removeEventListener('storage', onStorage);
    window.removeEventListener(syncEvent, onLocal);
    channel?.close();
  };
}
