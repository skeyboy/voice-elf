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
  ryan: 'Ryan',
  serena: 'Serena',
  vivian: 'Vivian',
  aiden: 'Aiden',
  dylan: 'Dylan',
  eric: 'Eric',
  sohee: 'Sohee',
  onoanna: 'Ono Anna',
  unclefu: 'Uncle Fu',
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
