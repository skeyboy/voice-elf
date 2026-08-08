import { refreshIcons } from './icons';

export function renderPageLoading(
  root: HTMLElement,
  title: string,
  detail: string,
  mode: 'page' | 'display' = 'page',
) {
  root.innerHTML = `
    <main class="page-loading-state ${mode === 'display' ? 'page-loading-display' : ''}" aria-busy="true" aria-live="polite">
      <div class="page-loading-mark" aria-hidden="true"><i data-lucide="audio-lines"></i></div>
      <div class="page-loading-copy">
        <strong>${escapeHtml(title)}</strong>
        <span>${escapeHtml(detail)}</span>
      </div>
      <span class="page-loading-progress" aria-hidden="true"><i></i></span>
    </main>
  `;
  refreshIcons(root);
}

function escapeHtml(value: string) {
  const element = document.createElement('div');
  element.textContent = value;
  return element.innerHTML;
}
