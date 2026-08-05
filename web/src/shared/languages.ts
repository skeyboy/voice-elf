export const languageNames: Record<string, string> = {
  auto: '自动检测',
  zh: '中文',
  en: 'English',
  ja: '日本語',
  ko: '한국어',
  fr: 'Français',
  de: 'Deutsch',
  es: 'Español',
  it: 'Italiano',
  pt: 'Português',
  ru: 'Русский',
};

export interface VoiceProfile {
  label: string;
  group: '中文参考声' | 'English voices' | '日本語音声';
  description: string;
}

export const voices: Record<string, VoiceProfile> = {
  F1: {
    label: '模思中文',
    group: '中文参考声',
    description: '中文 · 清晰自然 · MOSS Nano 参考声',
  },
  ZH_GENTLE: {
    label: '温柔晚安',
    group: '中文参考声',
    description: '中文 · 温柔舒缓 · MOSS Nano 参考声',
  },
  ZH_TAIWAN: {
    label: '台湾腔',
    group: '中文参考声',
    description: '中文 · 轻松口语 · MOSS Nano 参考声',
  },
  M1: {
    label: '京味胡同',
    group: '中文参考声',
    description: '中文 · 京味男声 · MOSS Nano 参考声',
  },
  ZH_LECTURE: {
    label: '文化讲述',
    group: '中文参考声',
    description: '中文 · 正式讲述 · MOSS Nano 参考声',
  },
  ZH_MONOLOGUE: {
    label: '沉稳独白',
    group: '中文参考声',
    description: '中文 · 沉稳独白 · MOSS Nano 参考声',
  },
  EN_MOSS: {
    label: 'OpenMOSS English',
    group: 'English voices',
    description: 'English · clear presentation · MOSS Nano reference voice',
  },
  EN_LECTURE: {
    label: 'English Lecture',
    group: 'English voices',
    description: 'English · measured lecture · MOSS Nano reference voice',
  },
  EN_NEWS: {
    label: 'English News',
    group: 'English voices',
    description: 'English · broadcast news · MOSS Nano reference voice',
  },
  EN_GENTLE: {
    label: 'Gentle English',
    group: 'English voices',
    description: 'English · gentle reminder · MOSS Nano reference voice',
  },
  EN_EXPRESSIVE: {
    label: 'Expressive English',
    group: 'English voices',
    description: 'English · expressive speech · MOSS Nano reference voice',
  },
  EN_NARRATION: {
    label: 'English Narration',
    group: 'English voices',
    description: 'English · calm narration · MOSS Nano reference voice',
  },
  JA_NEWS: {
    label: 'ニュース',
    group: '日本語音声',
    description: '日本語 · ニュース読み · MOSS Nano 参考音声',
  },
};

export function languageOptions(includeAuto: boolean) {
  return Object.entries(languageNames)
    .filter(([value]) => includeAuto || value !== 'auto')
    .map(([value, label]) => `<option value="${value}">${label}</option>`)
    .join('');
}

export function voiceOptions() {
  const groups: VoiceProfile['group'][] = ['中文参考声', 'English voices', '日本語音声'];
  return groups
    .map((group) => {
      const options = Object.entries(voices)
        .filter(([, profile]) => profile.group === group)
        .map(([value, profile]) => `<option value="${value}">${profile.label}</option>`)
        .join('');
      return `<optgroup label="${group}">${options}</optgroup>`;
    })
    .join('');
}
