import { apiRequest, type RoomSummary, type User } from '../api';
import { RoomEditor } from '../components/room-editor';
import { refreshIcons } from '../components/icons';
import { languageNames } from '../shared/languages';
import type { Page } from './page';

type MeetingScope = 'all' | 'owned' | 'joined';
type DurationFilter = 'all' | 'short' | 'medium' | 'long';

interface RoomsViewState {
  scope: MeetingScope;
  duration: DurationFilter;
  query: string;
  dateFrom: string;
  dateTo: string;
  scrollTop: number;
}

const MINUTE_MS = 60_000;
const roomsViewStates = new Map<string, RoomsViewState>();
const roomsCache = new Map<string, RoomSummary[]>();

export class RoomsPage implements Page {
  private root: HTMLElement | null = null;
  private editor: RoomEditor | null = null;
  private rooms: RoomSummary[] = [];
  private scope: MeetingScope = 'all';
  private duration: DurationFilter = 'all';
  private query = '';
  private dateFrom = '';
  private dateTo = '';

  constructor(
    private readonly user: User,
    private readonly onSelect: (roomId: string) => void,
    private readonly onError: (message: string) => void,
  ) {
    const saved = roomsViewStates.get(user.id);
    if (!saved) return;
    this.scope = saved.scope;
    this.duration = saved.duration;
    this.query = saved.query;
    this.dateFrom = saved.dateFrom;
    this.dateTo = saved.dateTo;
  }

  async mount(root: HTMLElement) {
    this.root = root;
    const avatar = escapeHtml(Array.from(this.user.username)[0]?.toUpperCase() ?? 'V');
    root.innerHTML = `
      <main class="home-page app-shell">
        <section class="home-user-overview" aria-labelledby="home-welcome">
          <span class="home-avatar" aria-hidden="true">${avatar}</span>
          <div class="home-welcome-copy">
            <span class="section-kicker"><i data-lucide="audio-lines"></i> VOICE ELF</span>
            <h1 id="home-welcome">你好，${escapeHtml(this.user.username)}</h1>
            <p>${formatMemberSince(this.user.created_at)}加入 · 只显示你创建或参加过的会议</p>
          </div>
          <dl class="home-meeting-stats" aria-label="会议统计">
            <div><dt>全部</dt><dd data-stat="all">—</dd></div>
            <div><dt>我创建的</dt><dd data-stat="owned">—</dd></div>
            <div><dt>我参加的</dt><dd data-stat="joined">—</dd></div>
          </dl>
          <button class="primary-command create-room" type="button"><i data-lucide="plus"></i><span>新建会议</span></button>
        </section>

        <section class="meeting-directory" aria-labelledby="meeting-directory-title">
          <header class="meeting-directory-heading">
            <div><span class="section-kicker"><i data-lucide="calendar-days"></i> MEETINGS</span><h2 id="meeting-directory-title">会议</h2></div>
            <span class="meeting-result-count" role="status" aria-live="polite"></span>
          </header>

          <form class="meeting-filters" role="search">
            <label class="meeting-search-field">
              <span>搜索</span>
              <span class="meeting-input-shell"><i data-lucide="search"></i><input type="search" name="query" autocomplete="off" placeholder="会议名称、创建者或字幕摘要"></span>
            </label>
            <label>
              <span>开始日期</span>
              <input class="meeting-date-input" type="date" name="date_from">
            </label>
            <label>
              <span>结束日期</span>
              <input class="meeting-date-input" type="date" name="date_to">
            </label>
            <label>
              <span>会议时长</span>
              <select name="duration">
                <option value="all">全部时长</option>
                <option value="short">少于 15 分钟</option>
                <option value="medium">15–60 分钟</option>
                <option value="long">超过 60 分钟</option>
              </select>
            </label>
            <button class="icon-button reset-meeting-filters" type="button" title="清除筛选" aria-label="清除筛选"><i data-lucide="rotate-ccw"></i></button>
          </form>

          <div class="meeting-list-toolbar">
            <div class="meeting-scope-tabs" role="group" aria-label="会议范围">
              <button type="button" data-scope="all" class="active">全部会议</button>
              <button type="button" data-scope="owned">我创建的</button>
              <button type="button" data-scope="joined">我参加的</button>
            </div>
          </div>
          <div class="meeting-list" aria-live="polite" aria-busy="true">
            <div class="meeting-list-loading" role="status">
              <span>正在同步会议</span>
              <i></i><i></i><i></i>
            </div>
          </div>
        </section>
      </main>
    `;
    this.editor = new RoomEditor((room) => this.onSelect(room.id));
    root.querySelector('.create-room')?.addEventListener('click', () => this.editor?.open());
    root.querySelector<HTMLFormElement>('.meeting-filters')?.addEventListener('submit', (event) => {
      event.preventDefault();
    });
    root.querySelector<HTMLInputElement>('[name="query"]')?.addEventListener('input', (event) => {
      this.query = (event.currentTarget as HTMLInputElement).value;
      this.render();
    });
    root.querySelector<HTMLInputElement>('[name="date_from"]')?.addEventListener('change', (event) => {
      this.dateFrom = (event.currentTarget as HTMLInputElement).value;
      this.render();
    });
    root.querySelector<HTMLInputElement>('[name="date_to"]')?.addEventListener('change', (event) => {
      this.dateTo = (event.currentTarget as HTMLInputElement).value;
      this.render();
    });
    root.querySelector<HTMLSelectElement>('[name="duration"]')?.addEventListener('change', (event) => {
      this.duration = (event.currentTarget as HTMLSelectElement).value as DurationFilter;
      this.render();
    });
    root.querySelector('.meeting-scope-tabs')?.addEventListener('click', (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-scope]');
      if (!button) return;
      this.scope = button.dataset.scope as MeetingScope;
      this.render();
    });
    root.querySelector('.reset-meeting-filters')?.addEventListener('click', () => this.resetFilters());
    this.restoreControls();
    refreshIcons(root);
    const cached = roomsCache.get(this.user.id);
    if (cached) {
      this.rooms = cached;
      this.render();
      this.restoreScroll();
      void this.load(true);
      return;
    }
    await this.load(false);
  }

  destroy() {
    this.persistState(window.scrollY);
    this.editor?.destroy();
    this.editor = null;
    this.root = null;
  }

  private async load(background: boolean) {
    if (!this.root) return;
    const list = this.root.querySelector<HTMLElement>('.meeting-list')!;
    if (!background) list.setAttribute('aria-busy', 'true');
    try {
      this.rooms = await apiRequest<RoomSummary[]>('/api/rooms');
      roomsCache.set(this.user.id, this.rooms);
      this.render();
      if (!background) this.restoreScroll();
    } catch (error) {
      if (!background) list.setAttribute('aria-busy', 'false');
      this.onError(error instanceof Error ? error.message : '无法加载会议');
      if (!background) this.restoreScroll();
    }
  }

  private resetFilters() {
    if (!this.root) return;
    this.scope = 'all';
    this.duration = 'all';
    this.query = '';
    this.dateFrom = '';
    this.dateTo = '';
    this.root.querySelector<HTMLFormElement>('.meeting-filters')?.reset();
    this.render();
  }

  private restoreControls() {
    if (!this.root) return;
    this.root.querySelector<HTMLInputElement>('[name="query"]')!.value = this.query;
    this.root.querySelector<HTMLInputElement>('[name="date_from"]')!.value = this.dateFrom;
    this.root.querySelector<HTMLInputElement>('[name="date_to"]')!.value = this.dateTo;
    this.root.querySelector<HTMLSelectElement>('[name="duration"]')!.value = this.duration;
    this.root.querySelectorAll<HTMLButtonElement>('[data-scope]').forEach((button) => {
      const active = button.dataset.scope === this.scope;
      button.classList.toggle('active', active);
      button.setAttribute('aria-pressed', String(active));
    });
  }

  private persistState(scrollTop = roomsViewStates.get(this.user.id)?.scrollTop ?? 0) {
    roomsViewStates.set(this.user.id, {
      scope: this.scope,
      duration: this.duration,
      query: this.query,
      dateFrom: this.dateFrom,
      dateTo: this.dateTo,
      scrollTop,
    });
  }

  private restoreScroll() {
    const scrollTop = roomsViewStates.get(this.user.id)?.scrollTop;
    if (!scrollTop) return;
    requestAnimationFrame(() => window.scrollTo({ top: scrollTop }));
  }

  private filteredRooms() {
    const normalizedQuery = this.query.trim().toLocaleLowerCase('zh-CN');
    const from = this.dateFrom ? new Date(`${this.dateFrom}T00:00:00`) : null;
    const to = this.dateTo ? new Date(`${this.dateTo}T23:59:59.999`) : null;
    return this.rooms
      .filter((room) => {
        if (this.scope === 'owned' && !room.is_owner) return false;
        if (this.scope === 'joined' && room.is_owner) return false;
        if (normalizedQuery) {
          const searchable = [room.name, room.owner_username, room.preview_text ?? '']
            .join(' ')
            .toLocaleLowerCase('zh-CN');
          if (!searchable.includes(normalizedQuery)) return false;
        }
        const activityAt = new Date(room.last_activity_at);
        if (from && activityAt < from) return false;
        if (to && activityAt > to) return false;
        const minutes = room.duration_ms / MINUTE_MS;
        if (this.duration === 'short' && minutes >= 15) return false;
        if (this.duration === 'medium' && (minutes < 15 || minutes > 60)) return false;
        if (this.duration === 'long' && minutes <= 60) return false;
        return true;
      })
      .sort(
        (left, right) =>
          new Date(right.last_activity_at).getTime() - new Date(left.last_activity_at).getTime(),
      );
  }

  private render() {
    if (!this.root) return;
    this.persistState();
    const owned = this.rooms.filter((room) => room.is_owner).length;
    const joined = this.rooms.length - owned;
    this.root.querySelector<HTMLElement>('[data-stat="all"]')!.textContent = String(this.rooms.length);
    this.root.querySelector<HTMLElement>('[data-stat="owned"]')!.textContent = String(owned);
    this.root.querySelector<HTMLElement>('[data-stat="joined"]')!.textContent = String(joined);
    this.root.querySelectorAll<HTMLButtonElement>('[data-scope]').forEach((button) => {
      const active = button.dataset.scope === this.scope;
      button.classList.toggle('active', active);
      button.setAttribute('aria-pressed', String(active));
    });

    const rooms = this.filteredRooms();
    this.root.querySelector<HTMLElement>('.meeting-result-count')!.textContent =
      `${rooms.length} 个结果`;
    const list = this.root.querySelector<HTMLElement>('.meeting-list')!;
    list.replaceChildren();
    list.setAttribute('aria-busy', 'false');
    if (rooms.length === 0) {
      list.innerHTML = `
        <div class="meetings-empty">
          <i data-lucide="calendar-days"></i>
          <strong>${this.rooms.length ? '没有符合条件的会议' : '还没有会议'}</strong>
          <span>${this.rooms.length ? '调整日期、时长或关键字后重试' : '新建会议，或通过会议链接加入他人的会议'}</span>
          ${this.rooms.length ? '<button class="secondary-command clear-empty-filters" type="button"><i data-lucide="rotate-ccw"></i><span>清除筛选</span></button>' : ''}
        </div>
      `;
      list.querySelector('.clear-empty-filters')?.addEventListener('click', () => this.resetFilters());
      refreshIcons(list);
      return;
    }

    rooms.forEach((room) => {
      const item = document.createElement('article');
      item.className = 'meeting-row';
      const source = languageNames[room.source_language] ?? room.source_language;
      const target = languageNames[room.target_language] ?? room.target_language;
      item.innerHTML = `
        <div class="meeting-primary">
          <span class="meeting-role-icon ${room.is_owner ? 'owner' : ''}"><i data-lucide="${room.is_owner ? 'crown' : 'users'}"></i></span>
          <div>
            <span class="meeting-title-line"><strong>${escapeHtml(room.name)}</strong><span class="meeting-role-badge ${room.is_owner ? 'owner' : ''}">${room.status === 'ended' ? '已结束' : room.is_owner ? '管理员' : '参与者'}</span></span>
            <small>${room.is_owner ? '由我创建' : `创建者 ${escapeHtml(room.owner_username)}`}</small>
          </div>
        </div>
        <p class="meeting-preview">${escapeHtml(room.preview_text ?? '暂无实时字幕记录')}</p>
        <div class="meeting-facts">
          <span><i data-lucide="calendar-days"></i><span><small>最近会议</small>${formatMeetingDate(room.last_activity_at)}</span></span>
          <span><i data-lucide="clock-3"></i><span><small>会议时长</small>${formatDuration(room.duration_ms)}</span></span>
          <span><i data-lucide="users"></i><span><small>成员</small>${room.member_count} 人</span></span>
          <span><i data-lucide="languages"></i><span><small>语言</small>${escapeHtml(source)} → ${escapeHtml(target)}</span></span>
        </div>
        <button class="meeting-enter" type="button"><span>${room.status === 'ended' ? '查看记录' : '进入会议'}</span><i data-lucide="arrow-right"></i></button>
      `;
      item.querySelector('.meeting-enter')?.addEventListener('click', () => this.onSelect(room.id));
      list.append(item);
    });
    refreshIcons(list);
  }
}

function formatDuration(durationMs: number) {
  const minutes = Math.max(0, Math.round(durationMs / MINUTE_MS));
  if (minutes < 1) return '少于 1 分钟';
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  return remainder ? `${hours} 小时 ${remainder} 分钟` : `${hours} 小时`;
}

function formatMeetingDate(value: string) {
  return new Intl.DateTimeFormat('zh-CN', {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value));
}

function formatMemberSince(value: string) {
  return new Intl.DateTimeFormat('zh-CN', { year: 'numeric', month: 'long' }).format(new Date(value));
}

function escapeHtml(value: string) {
  const element = document.createElement('div');
  element.textContent = value;
  return element.innerHTML;
}
