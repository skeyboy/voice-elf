import { apiRequest, type RuntimeDependency, type RuntimeSnapshot } from '../api';
import { refreshIcons } from '../components/icons';
import type { Page } from './page';

const DEPENDENCY_LABELS: Record<string, string> = {
  postgresql: 'PostgreSQL',
  system_installation: '系统初始化',
  instance_authorization: '实例授权',
  asr_provider: 'ASR Provider',
  funasr_streaming: 'FunASR 流式识别',
  tts_provider: 'TTS Provider',
  public_command_stream: 'Public gRPC 命令流',
  smtp: 'SMTP 邮件服务',
  qwen_tts: 'Qwen3-TTS',
};

const GROUPS = [
  { id: 'foundation', label: '基础设施', names: ['postgresql', 'system_installation'] },
  { id: 'control', label: '控制面', names: ['instance_authorization', 'public_command_stream'] },
  { id: 'speech', label: '语音能力', names: ['asr_provider', 'funasr_streaming', 'tts_provider', 'qwen_tts'] },
  { id: 'notification', label: '通知服务', names: ['smtp'] },
];

export class DependenciesPage implements Page {
  private root: HTMLElement | null = null;
  private snapshot: RuntimeSnapshot | null = null;
  private refreshSeconds = 15;
  private nextRefreshAt = 0;
  private pollTimer = 0;
  private countdownTimer = 0;
  private refreshing = false;
  private lastError = '';

  constructor(private readonly onError: (message: string) => void) {}

  async mount(root: HTMLElement) {
    this.root = root;
    root.innerHTML = `
      <main class="dependencies-page app-shell">
        <header class="dependencies-heading">
          <div class="dependencies-title">
            <a class="icon-button" href="/admin" title="返回系统管理" aria-label="返回系统管理"><i data-lucide="arrow-left"></i></a>
            <div><span class="section-kicker"><i data-lucide="activity"></i> RUNTIME OBSERVATORY</span><h1>服务依赖观测</h1></div>
          </div>
          <div class="dependencies-controls">
            <label><span>自动校验</span><select data-monitor-interval><option value="5">5 秒</option><option value="15" selected>15 秒</option><option value="30">30 秒</option><option value="60">60 秒</option><option value="0">暂停</option></select></label>
            <button class="button-primary" type="button" data-monitor-refresh><i data-lucide="refresh-cw"></i><span>立即校验</span></button>
          </div>
        </header>
        <div class="dependencies-live-strip"><span><i></i>持续观测中</span><strong data-monitor-countdown>正在连接</strong><small data-monitor-updated>尚无快照</small></div>
        <div class="dependencies-content" aria-live="polite" aria-busy="true"><div class="admin-loading"><i data-lucide="loader-circle"></i><span>正在执行依赖校验</span></div></div>
      </main>`;
    root.querySelector<HTMLSelectElement>('[data-monitor-interval]')?.addEventListener('change', (event) => {
      this.refreshSeconds = Number((event.currentTarget as HTMLSelectElement).value);
      this.schedule();
    });
    root.querySelector('[data-monitor-refresh]')?.addEventListener('click', () => void this.refresh(false));
    refreshIcons(root);
    await this.refresh(false);
  }

  destroy() {
    this.stopTimers();
    this.root = null;
  }

  private async refresh(silent: boolean) {
    if (!this.root || this.refreshing) return;
    this.stopTimers();
    this.refreshing = true;
    this.updateControls();
    try {
      this.snapshot = await apiRequest<RuntimeSnapshot>('/api/runtime/dependencies');
      this.lastError = '';
      if (this.root) this.render();
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : '依赖检测接口不可用';
      if (this.snapshot) this.render();
      else this.renderUnavailable();
      if (!silent) this.onError(this.lastError);
    } finally {
      this.refreshing = false;
      this.updateControls();
      this.schedule();
    }
  }

  private render() {
    if (!this.root || !this.snapshot) return;
    const content = this.root.querySelector<HTMLElement>('.dependencies-content')!;
    const data = this.snapshot;
    const counts = {
      ready: data.dependencies.filter((item) => item.status === 'ready').length,
      degraded: data.dependencies.filter((item) => item.status === 'degraded').length,
      unavailable: data.dependencies.filter((item) => ['unavailable', 'unknown'].includes(item.status)).length,
    };
    const overallLabel = data.overall_status === 'ready' ? '全部依赖正常' : data.overall_status === 'degraded' ? '存在降级能力' : '必需依赖不可用';
    content.innerHTML = `
      ${this.lastError ? `<div class="dependencies-stale" role="alert"><i data-lucide="wifi-off"></i><strong>当前显示最近成功快照</strong><span>${escapeHtml(this.lastError)}</span></div>` : ''}
      <section class="dependencies-overview status-${escapeHtml(data.overall_status)}">
        <div class="dependencies-overall"><span class="dependencies-overall-icon"><i data-lucide="${data.overall_status === 'ready' ? 'circle-check' : data.overall_status === 'degraded' ? 'triangle-alert' : 'circle-x'}"></i></span><div><small>${escapeHtml(data.service)} · v${escapeHtml(data.version)}</small><h2>${overallLabel}</h2><p>${data.initialized ? '系统已初始化' : '系统未初始化'} · ${data.authorized ? '实例授权有效' : '实例授权不可用'}</p></div></div>
        <dl><div><dt>正常</dt><dd>${counts.ready}</dd></div><div><dt>降级</dt><dd>${counts.degraded}</dd></div><div><dt>异常</dt><dd>${counts.unavailable}</dd></div><div><dt>总计</dt><dd>${data.dependencies.length}</dd></div></dl>
      </section>
      <div class="dependency-groups">
        ${GROUPS.map((group) => {
          const items = group.names.map((name) => data.dependencies.find((item) => item.name === name)).filter((item): item is RuntimeDependency => Boolean(item));
          if (!items.length) return '';
          const groupReady = items.every((item) => item.status === 'ready');
          return `<section class="dependency-group"><header><div><span class="section-kicker">${group.id.toUpperCase()}</span><h2>${group.label}</h2></div><span class="dependency-group-state ${groupReady ? 'ready' : 'attention'}"><i></i>${groupReady ? '全部正常' : '需要关注'}</span></header><div class="dependency-card-grid">${items.map((item) => this.dependencyCard(item)).join('')}</div></section>`;
        }).join('')}
      </div>`;
    content.setAttribute('aria-busy', 'false');
    const updated = this.root.querySelector<HTMLElement>('[data-monitor-updated]');
    if (updated) updated.textContent = `最近成功校验 ${formatDate(data.generated_at)}`;
    refreshIcons(content);
  }

  private dependencyCard(item: RuntimeDependency) {
    const label = item.status === 'ready' ? '正常' : item.status === 'degraded' ? '降级' : item.status === 'unavailable' ? '不可用' : '未知';
    const icon = item.status === 'ready' ? 'check' : item.status === 'degraded' ? 'triangle-alert' : 'x';
    return `<article class="dependency-card status-${escapeHtml(item.status)}"><header><span class="dependency-card-icon"><i data-lucide="${icon}"></i></span><div><strong>${escapeHtml(DEPENDENCY_LABELS[item.name] ?? item.name)}</strong><code>${escapeHtml(item.name)}</code></div><span class="dependency-card-status">${label}</span></header><p>${escapeHtml(item.message)}</p><footer><span>${item.required ? '必需依赖' : '可选能力'}</span><time>${formatDate(item.checked_at)}</time></footer></article>`;
  }

  private renderUnavailable() {
    if (!this.root) return;
    const content = this.root.querySelector<HTMLElement>('.dependencies-content')!;
    content.innerHTML = `<div class="dependencies-unavailable"><i data-lucide="server-off"></i><strong>无法取得依赖快照</strong><p>${escapeHtml(this.lastError)}</p><button class="button-primary" type="button" data-monitor-retry><i data-lucide="refresh-cw"></i><span>重新校验</span></button></div>`;
    content.querySelector('[data-monitor-retry]')?.addEventListener('click', () => void this.refresh(false));
    content.setAttribute('aria-busy', 'false');
    refreshIcons(content);
  }

  private schedule() {
    this.stopTimers();
    if (!this.root || this.refreshSeconds <= 0) {
      this.nextRefreshAt = 0;
      this.updateCountdown();
      return;
    }
    this.nextRefreshAt = Date.now() + this.refreshSeconds * 1_000;
    this.pollTimer = window.setTimeout(() => void this.refresh(true), this.refreshSeconds * 1_000);
    this.countdownTimer = window.setInterval(() => this.updateCountdown(), 1_000);
    this.updateCountdown();
  }

  private stopTimers() {
    window.clearTimeout(this.pollTimer);
    window.clearInterval(this.countdownTimer);
  }

  private updateControls() {
    const button = this.root?.querySelector<HTMLButtonElement>('[data-monitor-refresh]');
    if (!button) return;
    button.disabled = this.refreshing;
    button.innerHTML = `<i data-lucide="${this.refreshing ? 'loader-circle' : 'refresh-cw'}"></i><span>${this.refreshing ? '校验中' : '立即校验'}</span>`;
    refreshIcons(button);
    this.updateCountdown();
  }

  private updateCountdown() {
    const countdown = this.root?.querySelector<HTMLElement>('[data-monitor-countdown]');
    if (!countdown) return;
    if (this.refreshing) countdown.textContent = '正在执行校验';
    else if (!this.refreshSeconds || !this.nextRefreshAt) countdown.textContent = '自动校验已暂停';
    else countdown.textContent = `${Math.max(0, Math.ceil((this.nextRefreshAt - Date.now()) / 1_000))} 秒后再次校验`;
  }
}

function escapeHtml(value: string) {
  return value.replace(/[&<>'"]/g, (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', "'": '&#39;', '"': '&quot;' })[character]!);
}

function formatDate(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' }).format(date);
}
