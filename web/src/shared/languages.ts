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

export const voices: Record<string, string> = {
  F1: 'F1 女声',
  M1: 'M1 男声',
};

export function languageOptions(includeAuto: boolean) {
  return Object.entries(languageNames)
    .filter(([value]) => includeAuto || value !== 'auto')
    .map(([value, label]) => `<option value="${value}">${label}</option>`)
    .join('');
}

export function voiceOptions() {
  return Object.entries(voices)
    .map(([value, label]) => `<option value="${value}">${label}</option>`)
    .join('');
}
