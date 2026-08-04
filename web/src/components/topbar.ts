import type { User } from '../api';
import { refreshIcons } from './icons';

export type ConnectionStatus = 'hidden' | 'connecting' | 'connected' | 'offline' | 'viewer';

export class TopBar {
  readonly element: HTMLElement;
  private readonly connection: HTMLElement;

  constructor(user: User, onRooms: () => void, onSettings: () => void, onLogout: () => void) {
    this.element = document.createElement('header');
    this.element.className = 'topbar';
    this.element.innerHTML = `
      <div class="topbar-inner">
        <button class="brand brand-button" type="button" aria-label="Voice Elf 房间目录">
          <span class="brand-mark"><i data-lucide="audio-lines"></i></span>
          <span><strong>Voice Elf</strong><small>LOCAL INTERPRETER</small></span>
        </button>
        <div class="top-actions">
          <div class="connection" role="status" hidden><i data-lucide="wifi-off"></i><span></span></div>
          <button class="top-action rooms-link" type="button"><i data-lucide="door-open"></i><span>房间</span></button>
          <button class="top-action settings-link" type="button"><i data-lucide="settings-2"></i><span>设置</span></button>
          <span class="account-name"><i data-lucide="user-round"></i><span>${escapeHtml(user.username)}</span></span>
          <button class="icon-button top-logout" type="button" title="退出账号" aria-label="退出账号"><i data-lucide="log-out"></i></button>
        </div>
      </div>
    `;
    this.connection = this.element.querySelector<HTMLElement>('.connection')!;
    this.element.querySelectorAll('.brand-button, .rooms-link').forEach((button) =>
      button.addEventListener('click', onRooms),
    );
    this.element.querySelector('.settings-link')?.addEventListener('click', onSettings);
    this.element.querySelector('.top-logout')?.addEventListener('click', onLogout);
    refreshIcons(this.element);
  }

  setConnection(status: ConnectionStatus) {
    this.connection.hidden = status === 'hidden';
    this.connection.classList.toggle('online', status === 'connected');
    this.connection.classList.toggle('viewer', status === 'viewer');
    const content =
      status === 'viewer'
        ? ['radio', '房间实时同步']
        : status === 'connected'
          ? ['wifi', '房主控制已连接']
          : ['wifi-off', status === 'connecting' ? '正在连接' : '连接中断'];
    this.connection.innerHTML = `<i data-lucide="${content[0]}"></i><span>${content[1]}</span>`;
    refreshIcons(this.connection);
  }
}

function escapeHtml(value: string) {
  const element = document.createElement('div');
  element.textContent = value;
  return element.innerHTML;
}
