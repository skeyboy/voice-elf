import { apiRequest, type RoomSummary } from '../api';
import { RoomEditor } from '../components/room-editor';
import { refreshIcons } from '../components/icons';
import type { Page } from './page';

export class RoomsPage implements Page {
  private root: HTMLElement | null = null;
  private editor: RoomEditor | null = null;
  private rooms: RoomSummary[] = [];

  constructor(
    private readonly onSelect: (roomId: string) => void,
    private readonly onError: (message: string) => void,
  ) {}

  async mount(root: HTMLElement) {
    this.root = root;
    root.innerHTML = `
      <main class="rooms-page app-shell">
        <section class="rooms-page-heading">
          <div><span class="section-kicker"><i data-lucide="door-open"></i> ROOM DIRECTORY</span><h1>房间</h1></div>
          <button class="primary-command create-room" type="button"><i data-lucide="plus"></i><span>新建房间</span></button>
        </section>
        <form class="rooms-page-search">
          <i data-lucide="search"></i>
          <input type="search" placeholder="搜索房间名称" aria-label="搜索房间">
          <button type="submit">搜索</button>
        </form>
        <section class="rooms-grid" aria-live="polite"></section>
      </main>
    `;
    this.editor = new RoomEditor((room) => this.onSelect(room.id));
    root.querySelector('.create-room')?.addEventListener('click', () => this.editor?.open());
    root.querySelector('form')?.addEventListener('submit', (event) => {
      event.preventDefault();
      void this.load(root.querySelector<HTMLInputElement>('input')!.value);
    });
    refreshIcons(root);
    await this.load();
  }

  destroy() {
    this.editor?.destroy();
    this.editor = null;
    this.root = null;
  }

  private async load(search = '') {
    if (!this.root) return;
    try {
      const query = search.trim() ? `?q=${encodeURIComponent(search.trim())}` : '';
      this.rooms = await apiRequest<RoomSummary[]>(`/api/rooms${query}`);
      this.render();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法加载房间');
    }
  }

  private render() {
    if (!this.root) return;
    const grid = this.root.querySelector<HTMLElement>('.rooms-grid')!;
    grid.replaceChildren();
    if (this.rooms.length === 0) {
      grid.innerHTML = '<div class="rooms-empty"><i data-lucide="door-open"></i><strong>没有找到房间</strong></div>';
      refreshIcons(grid);
      return;
    }
    this.rooms.forEach((room) => {
      const item = document.createElement('article');
      item.className = 'room-card';
      item.innerHTML = `
        <div class="room-card-heading">
          <span class="room-list-icon"><i data-lucide="${room.is_owner ? 'crown' : 'door-open'}"></i></span>
          <div><strong>${escapeHtml(room.name)}</strong><small>房主 ${escapeHtml(room.owner_username)}</small></div>
          <span class="room-list-role">${room.is_owner ? '房主' : room.is_member ? '已加入' : '可加入'}</span>
        </div>
        <p>${escapeHtml(room.preview_text ?? '暂无翻译记录')}</p>
        <div class="room-card-meta">
          <span><i data-lucide="users"></i>${room.member_count} 人</span>
          <span><i data-lucide="languages"></i>${room.utterance_count} 条记录</span>
        </div>
        <button class="room-enter" type="button">${room.is_member || room.is_owner ? '进入房间' : '加入房间'}</button>
      `;
      item.querySelector('.room-enter')?.addEventListener('click', () => void this.enter(room));
      grid.append(item);
    });
    refreshIcons(grid);
  }

  private async enter(room: RoomSummary) {
    try {
      if (!room.is_owner && !room.is_member) {
        await apiRequest(`/api/rooms/${room.id}/join`, { method: 'POST' });
      }
      this.onSelect(room.id);
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法进入房间');
    }
  }
}

function escapeHtml(value: string) {
  const element = document.createElement('div');
  element.textContent = value;
  return element.innerHTML;
}
