import { voices } from './languages';

export interface PlaybackPreferences {
  voice: string;
  autoplay: boolean;
  enhancedVoiceFilter: boolean;
}

const defaults: PlaybackPreferences = {
  voice: 'F1',
  autoplay: true,
  enhancedVoiceFilter: true,
};

function storageKey(userId: string) {
  return `voice-elf:preferences:${userId}`;
}

export function loadPreferences(userId: string): PlaybackPreferences {
  try {
    const stored = JSON.parse(localStorage.getItem(storageKey(userId)) ?? '{}') as Partial<PlaybackPreferences>;
    return {
      voice: typeof stored.voice === 'string' && stored.voice in voices ? stored.voice : defaults.voice,
      autoplay: typeof stored.autoplay === 'boolean' ? stored.autoplay : defaults.autoplay,
      enhancedVoiceFilter:
        typeof stored.enhancedVoiceFilter === 'boolean'
          ? stored.enhancedVoiceFilter
          : defaults.enhancedVoiceFilter,
    };
  } catch {
    return { ...defaults };
  }
}

export function savePreferences(userId: string, preferences: PlaybackPreferences) {
  localStorage.setItem(storageKey(userId), JSON.stringify(preferences));
}
