export type SubtitleDisplayMode = 'both' | 'source' | 'translation';
export type SubtitleContentAlignment = 'top' | 'center' | 'bottom';

export interface SubtitlePreferences {
  displayMode: SubtitleDisplayMode;
  contentAlignment: SubtitleContentAlignment;
  backgroundColor: string;
  sourceColor: string;
  translationColor: string;
  sourceFontSize: number;
  translationFontSize: number;
  lineHeight: number;
  blockGap: number;
  screenPadding: number;
}

export interface SubtitlePreset {
  id: string;
  name: string;
  description: string;
  values: Pick<
    SubtitlePreferences,
    'backgroundColor' | 'sourceColor' | 'translationColor'
  >;
}

export const subtitlePresets: SubtitlePreset[] = [
  {
    id: 'dark',
    name: '深色会议',
    description: '低眩光，适合投影与悬浮窗',
    values: {
      backgroundColor: '#111512',
      sourceColor: '#F7FAF8',
      translationColor: '#9FD6BA',
    },
  },
  {
    id: 'light',
    name: '明亮会议',
    description: '适合光线充足的会议空间',
    values: {
      backgroundColor: '#F4F5F1',
      sourceColor: '#17201B',
      translationColor: '#216747',
    },
  },
  {
    id: 'contrast',
    name: '高对比',
    description: '远距离观看时保持醒目',
    values: {
      backgroundColor: '#000000',
      sourceColor: '#FFFFFF',
      translationColor: '#FFD65A',
    },
  },
];

export const defaultSubtitlePreferences: SubtitlePreferences = {
  displayMode: 'both',
  contentAlignment: 'bottom',
  ...subtitlePresets[0].values,
  sourceFontSize: 48,
  translationFontSize: 38,
  lineHeight: 1.3,
  blockGap: 18,
  screenPadding: 40,
};

const syncEvent = 'voice-elf:subtitle-preferences-changed';
const syncChannel = 'voice-elf:subtitle-preferences-sync';

function storageKey(userId: string) {
  return `voice-elf:subtitle-preferences:${userId}`;
}

function clamp(value: unknown, fallback: number, minimum: number, maximum: number) {
  const number = typeof value === 'number' ? value : Number.NaN;
  return Number.isFinite(number) ? Math.min(maximum, Math.max(minimum, number)) : fallback;
}

function color(value: unknown, fallback: string) {
  return typeof value === 'string' && /^#[0-9a-f]{6}$/i.test(value) ? value.toUpperCase() : fallback;
}

function normalize(stored: Partial<SubtitlePreferences>): SubtitlePreferences {
  const mode = stored.displayMode;
  const alignment = stored.contentAlignment;
  return {
    displayMode:
      mode === 'source' || mode === 'translation' || mode === 'both'
        ? mode
        : defaultSubtitlePreferences.displayMode,
    contentAlignment:
      alignment === 'top' || alignment === 'center' || alignment === 'bottom'
        ? alignment
        : defaultSubtitlePreferences.contentAlignment,
    backgroundColor: color(stored.backgroundColor, defaultSubtitlePreferences.backgroundColor),
    sourceColor: color(stored.sourceColor, defaultSubtitlePreferences.sourceColor),
    translationColor: color(
      stored.translationColor,
      defaultSubtitlePreferences.translationColor,
    ),
    sourceFontSize: clamp(stored.sourceFontSize, defaultSubtitlePreferences.sourceFontSize, 24, 88),
    translationFontSize: clamp(
      stored.translationFontSize,
      defaultSubtitlePreferences.translationFontSize,
      20,
      76,
    ),
    lineHeight: clamp(stored.lineHeight, defaultSubtitlePreferences.lineHeight, 1, 1.8),
    blockGap: clamp(stored.blockGap, defaultSubtitlePreferences.blockGap, 0, 48),
    screenPadding: clamp(stored.screenPadding, defaultSubtitlePreferences.screenPadding, 12, 96),
  };
}

export function loadSubtitlePreferences(userId: string): SubtitlePreferences {
  try {
    const stored = JSON.parse(localStorage.getItem(storageKey(userId)) ?? '{}') as Partial<SubtitlePreferences>;
    return normalize(stored);
  } catch {
    return { ...defaultSubtitlePreferences };
  }
}

export function saveSubtitlePreferences(userId: string, preferences: SubtitlePreferences) {
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

export function subscribeSubtitlePreferences(
  userId: string,
  callback: (preferences: SubtitlePreferences) => void,
) {
  const onStorage = (event: StorageEvent) => {
    if (event.key === storageKey(userId)) callback(loadSubtitlePreferences(userId));
  };
  const onLocal = (event: Event) => {
    const detail = (event as CustomEvent<{ userId: string; preferences: SubtitlePreferences }>).detail;
    if (detail?.userId === userId) callback(normalize(detail.preferences));
  };
  const channel = 'BroadcastChannel' in window ? new BroadcastChannel(syncChannel) : null;
  if (channel) {
    channel.onmessage = (event: MessageEvent<{ userId: string; preferences: SubtitlePreferences }>) => {
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

export function matchingSubtitlePreset(preferences: SubtitlePreferences) {
  return subtitlePresets.find((preset) =>
    (Object.keys(preset.values) as Array<keyof SubtitlePreset['values']>).every(
      (key) => preset.values[key] === preferences[key],
    ),
  );
}
