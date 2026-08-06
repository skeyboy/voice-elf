import type { User } from '../api';
import { refreshIcons } from './icons';

export type ConnectionStatus = 'hidden' | 'connecting' | 'connected' | 'offline' | 'viewer';
export type AppSection = 'home' | 'profile' | 'admin';

export class TopBar {
  readonly element: HTMLElement;
  private readonly connection: HTMLElement;

  constructor(
    user: User,
    systemName: string,
    organizationName: string,
    onHome: () => void,
    onProfile: () => void,
    onAdmin: () => void,
    onLogout: () => void,
  ) {
    const avatar = escapeHtml(Array.from(user.username)[0]?.toUpperCase() ?? 'V');
    this.element = document.createElement('header');
    this.element.className = 'topbar';
    this.element.innerHTML = `
      <div class="topbar-inner">
        <button class="brand brand-button" type="button" aria-label="${escapeAttribute(systemName)} 房间目录">
          <span class="brand-mark"><i data-lucide="audio-lines"></i></span>
          <span><strong>${escapeHtml(systemName)}</strong><small>${escapeHtml(organizationName || 'LOCAL INTERPRETER')}</small></span>
        </button>
        <nav class="top-tabs ${user.role === 'admin' ? 'has-admin' : ''}" aria-label="主要页面">
          <button class="top-tab home-link" type="button"><i data-lucide="house"></i><span>首页</span></button>
          ${user.role === 'admin' ? '<button class="top-tab admin-link" type="button"><i data-lucide="shield-check"></i><span>管理</span></button>' : ''}
          <button class="top-tab profile-link" type="button"><i data-lucide="user-round"></i><span>我的</span></button>
        </nav>
        <div class="top-actions">
          <div class="connection" role="status" hidden><i data-lucide="wifi-off"></i><span></span></div>
          <button class="account-summary" type="button" title="打开我的页面">
            <span class="account-avatar">${avatar}</span>
            <span><strong>${escapeHtml(user.username)}</strong><small>${user.role === 'admin' ? '系统管理员' : '个人账户'}</small></span>
          </button>
          <button class="icon-button top-logout" type="button" title="退出账号" aria-label="退出账号"><i data-lucide="log-out"></i></button>
        </div>
      </div>
    `;
    this.connection = this.element.querySelector<HTMLElement>('.connection')!;
    this.element.querySelectorAll('.brand-button, .home-link').forEach((button) =>
      button.addEventListener('click', onHome),
    );
    this.element.querySelectorAll('.profile-link, .account-summary').forEach((button) =>
      button.addEventListener('click', onProfile),
    );
    this.element.querySelector('.admin-link')?.addEventListener('click', onAdmin);
    this.element.querySelector('.top-logout')?.addEventListener('click', onLogout);
    refreshIcons(this.element);
  }

  setActiveSection(section: AppSection) {
    this.element.querySelectorAll<HTMLButtonElement>('.top-tab').forEach((button) => {
      const sectionClass = section === 'home' ? 'home-link' : section === 'admin' ? 'admin-link' : 'profile-link';
      const active = button.classList.contains(sectionClass);
      button.classList.toggle('active', active);
      if (active) button.setAttribute('aria-current', 'page');
      else button.removeAttribute('aria-current');
    });
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

function escapeAttribute(value: string) {
  return escapeHtml(value).replaceAll('"', '&quot;').replaceAll("'", '&#39;');
}
