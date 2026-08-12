import {
  apiRequest,
  type AdminOverview,
  type AdminUser,
  type AsrManagement,
  type AsrProvider,
  type AuthorityInstance,
  type AuthorityTenant,
  type ChangeHistoryRecord,
  type InstanceAuthorization,
  type IssuedAuthorityCredential,
  type MailStatus,
  type Paginated,
  type RoomDetail,
  type RoomSummary,
  type RuntimeSnapshot,
  type TtsManagement,
  type TtsProvider,
  type User,
} from '../api';
import { refreshIcons } from '../components/icons';
import type { Page } from './page';

type AdminSection = 'deployment' | 'users' | 'rooms' | 'asr' | 'tts' | 'email' | 'history' | 'authority';
type SortOrder = 'asc' | 'desc';

const USER_STATUS = {
  pending: ['待验证', 'pending'],
  active: ['正常', 'active'],
  suspended: ['已停用', 'suspended'],
} as const;

const ROOM_STATUS = {
  active: ['进行中', 'active'],
  ended: ['已结束', 'ended'],
  archived: ['已归档', 'archived'],
} as const;

const TENANT_STATUS = {
  active: ['正常', 'active'],
  suspended: ['已暂停', 'suspended'],
  revoked: ['已撤销', 'archived'],
} as const;

export class AdminPage implements Page {
  private root: HTMLElement | null = null;
  private section: AdminSection = 'users';
  private query = '';
  private status = '';
  private role = '';
  private sort = 'created_at';
  private order: SortOrder = 'desc';
  private page = 1;
  private pageSize = 20;
  private requestId = 0;
  private searchTimer = 0;
  private ttsPollTimer = 0;
  private deploymentPollTimer = 0;
  private deploymentCountdownTimer = 0;
  private deploymentRefreshSeconds = 15;
  private deploymentNextRefreshAt = 0;
  private deploymentRefreshing = false;
  private deploymentLastError = '';
  private authorityEnabled = false;
  private asrManagement: AsrManagement | null = null;
  private ttsManagement: TtsManagement | null = null;
  private tenants = new Map<string, AuthorityTenant>();
  private users = new Map<string, AdminUser>();
  private mailStatus: MailStatus | null = null;
  private historyEntityType = '';

  constructor(
    private readonly currentUser: User,
    private readonly onError: (message: string) => void,
    private readonly onMessage: (message: string) => void,
  ) {}

  async mount(root: HTMLElement) {
    this.root = root;
    root.innerHTML = `
      <main class="admin-page app-shell">
        <header class="admin-heading">
          <div>
            <span class="section-kicker"><i data-lucide="shield-check"></i> ADMIN</span>
            <h1>系统管理</h1>
          </div>
          <div class="admin-heading-actions">
            <a class="button-secondary" href="/admin/lexicons"><i data-lucide="library"></i><span>词库管理</span></a>
            <a class="button-secondary" href="/admin/dependencies"><i data-lucide="activity"></i><span>依赖观测</span></a>
            <button class="icon-button admin-refresh" type="button" title="刷新管理数据" aria-label="刷新管理数据"><i data-lucide="refresh-cw"></i></button>
          </div>
        </header>

        <dl class="admin-overview" aria-label="系统概览" aria-busy="true">
          <div><dt>人员总数</dt><dd data-overview="total_users">—</dd></div>
          <div><dt>待验证</dt><dd data-overview="pending_users">—</dd></div>
          <div><dt>进行中会议</dt><dd data-overview="active_rooms">—</dd></div>
          <div><dt>会议总数</dt><dd data-overview="total_rooms">—</dd></div>
        </dl>

        <section class="admin-workspace">
          <div class="admin-section-tabs" role="tablist" aria-label="管理对象">
            <button type="button" role="tab" data-section="deployment"><i data-lucide="server"></i><span>部署检测</span></button>
            <button class="active" type="button" role="tab" data-section="users"><i data-lucide="users"></i><span>人员管理</span></button>
            <button type="button" role="tab" data-section="rooms"><i data-lucide="calendar-clock"></i><span>会议管理</span></button>
            <button type="button" role="tab" data-section="asr"><i data-lucide="audio-waveform"></i><span>ASR 管理</span></button>
            <button type="button" role="tab" data-section="tts"><i data-lucide="speech"></i><span>TTS 管理</span></button>
            <button type="button" role="tab" data-section="email"><i data-lucide="mail-cog"></i><span>邮箱配置</span></button>
            <button type="button" role="tab" data-section="history"><i data-lucide="history"></i><span>变更历史</span></button>
            <button type="button" role="tab" data-section="authority" hidden><i data-lucide="key-round"></i><span>授权管理</span></button>
          </div>

          <form class="admin-filters" role="search">
            <label class="admin-search-field">
              <span>搜索</span>
              <span class="admin-input-shell"><i data-lucide="search"></i><input type="search" name="q" autocomplete="off" placeholder="账号名称"></span>
            </label>
            <label>
              <span>状态</span>
              <select name="status"></select>
            </label>
            <label class="admin-role-filter">
              <span>角色</span>
              <select name="role">
                <option value="">全部角色</option>
                <option value="admin">系统管理员</option>
                <option value="member">普通成员</option>
              </select>
            </label>
            <label>
              <span>排序</span>
              <select name="sort"></select>
            </label>
            <button class="icon-button admin-order" type="button" title="切换排序方向" aria-label="切换排序方向"><i data-lucide="arrow-down-wide-narrow"></i></button>
            <button class="icon-button admin-reset" type="button" title="清除筛选" aria-label="清除筛选"><i data-lucide="rotate-ccw"></i></button>
          </form>

          <div class="admin-result-bar">
            <span class="admin-result-count" role="status" aria-live="polite"></span>
            <span class="admin-result-actions">
              <button class="admin-mail-state" type="button" hidden></button>
              <button class="admin-import-users button-secondary" type="button" hidden><i data-lucide="file-up"></i><span>批量导入</span></button>
              <button class="admin-create-user button-primary" type="button" hidden><i data-lucide="user-plus"></i><span>新建用户</span></button>
              <button class="admin-create-tenant button-secondary" type="button" hidden><i data-lucide="plus"></i><span>创建租户</span></button>
              <label><span>每页</span><select name="page_size"><option>10</option><option selected>20</option><option>50</option></select></label>
            </span>
          </div>
          <div class="admin-table-shell" aria-live="polite" aria-busy="true"></div>
          <nav class="admin-pagination" aria-label="分页"></nav>
        </section>

        <dialog class="admin-inspector">
          <header class="admin-inspector-heading">
            <div><span class="section-kicker"><i data-lucide="eye-off"></i> STEALTH INSPECT</span><h2>会议检查</h2></div>
            <button class="icon-button inspector-close" type="button" title="关闭" aria-label="关闭"><i data-lucide="x"></i></button>
          </header>
          <div class="admin-inspector-body"></div>
        </dialog>
      </main>
    `;
    this.bindEvents();
    this.configureFilters();
    refreshIcons(root);
    const authorization = await apiRequest<InstanceAuthorization>('/api/instance/authorization').catch(() => null);
    this.authorityEnabled = authorization?.mode === 'bus';
    const authorityTab = root.querySelector<HTMLButtonElement>('[data-section="authority"]');
    if (authorityTab) authorityTab.hidden = !this.authorityEnabled;
    await Promise.all([this.loadOverview(), this.loadAsrManagement(), this.loadTtsManagement(), this.loadMailStatus(), this.loadList()]);
  }

  destroy() {
    window.clearTimeout(this.searchTimer);
    window.clearTimeout(this.ttsPollTimer);
    this.stopDeploymentPolling();
    this.requestId += 1;
    this.root?.querySelector<HTMLDialogElement>('.admin-inspector')?.close();
    this.root = null;
  }

  private bindEvents() {
    if (!this.root) return;
    this.root.querySelector('.admin-refresh')?.addEventListener('click', () => {
      void Promise.all([this.loadOverview(), this.loadList()]);
    });
    this.root.querySelector('.admin-section-tabs')?.addEventListener('click', (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-section]');
      if (!button || button.dataset.section === this.section) return;
      this.section = button.dataset.section as AdminSection;
      if (this.section !== 'deployment') this.stopDeploymentPolling();
      this.query = '';
      this.status = '';
      this.role = '';
      this.sort = this.section === 'rooms' ? 'updated_at' : 'created_at';
      this.order = 'desc';
      this.page = 1;
      this.root?.querySelector<HTMLFormElement>('.admin-filters')?.reset();
      this.configureFilters();
      void this.loadList();
    });
    this.root.querySelector<HTMLInputElement>('[name="q"]')?.addEventListener('input', (event) => {
      this.query = (event.currentTarget as HTMLInputElement).value;
      this.page = 1;
      window.clearTimeout(this.searchTimer);
      this.searchTimer = window.setTimeout(() => void this.loadList(), 280);
    });
    this.root.querySelector<HTMLSelectElement>('[name="status"]')?.addEventListener('change', (event) => {
      this.status = (event.currentTarget as HTMLSelectElement).value;
      this.page = 1;
      void this.loadList();
    });
    this.root.querySelector<HTMLSelectElement>('[name="role"]')?.addEventListener('change', (event) => {
      this.role = (event.currentTarget as HTMLSelectElement).value;
      this.page = 1;
      void this.loadList();
    });
    this.root.querySelector<HTMLSelectElement>('[name="sort"]')?.addEventListener('change', (event) => {
      this.sort = (event.currentTarget as HTMLSelectElement).value;
      this.page = 1;
      void this.loadList();
    });
    this.root.querySelector<HTMLSelectElement>('[name="page_size"]')?.addEventListener('change', (event) => {
      this.pageSize = Number((event.currentTarget as HTMLSelectElement).value);
      this.page = 1;
      void this.loadList();
    });
    this.root.querySelector('.admin-order')?.addEventListener('click', () => {
      this.order = this.order === 'desc' ? 'asc' : 'desc';
      this.updateOrderButton();
      this.page = 1;
      void this.loadList();
    });
    this.root.querySelector('.admin-reset')?.addEventListener('click', () => {
      this.query = '';
      this.status = '';
      this.role = '';
      this.sort = this.section === 'rooms' ? 'updated_at' : 'created_at';
      this.order = 'desc';
      this.page = 1;
      this.root?.querySelector<HTMLFormElement>('.admin-filters')?.reset();
      this.configureFilters();
      void this.loadList();
    });
    this.root.querySelector<HTMLFormElement>('.admin-filters')?.addEventListener('submit', (event) => event.preventDefault());
    this.root.querySelector('.admin-pagination')?.addEventListener('click', (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-page]');
      if (!button || button.disabled) return;
      this.page = Number(button.dataset.page);
      void this.loadList();
    });
    this.root.querySelector('.admin-table-shell')?.addEventListener('change', (event) => {
      const role = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-user-role]');
      if (role) void this.changeUserRole(role);
      const roomStatus = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-room-status]');
      if (roomStatus) void this.changeRoomStatus(roomStatus);
      const tenantStatus = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-tenant-status]');
      if (tenantStatus) void this.changeTenantStatus(tenantStatus);
      const systemAsr = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-system-asr]');
      if (systemAsr) void this.changeSystemAsr(systemAsr);
      const tenantAsr = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-tenant-asr]');
      if (tenantAsr) void this.changeTenantAsr(tenantAsr);
      const systemTts = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-system-tts]');
      if (systemTts) void this.changeSystemTts(systemTts);
      const tenantTts = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-tenant-tts]');
      if (tenantTts) void this.changeTenantTts(tenantTts);
      const historyEntity = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-history-entity]');
      if (historyEntity) {
        this.historyEntityType = historyEntity.value;
        this.page = 1;
        void this.loadList();
      }
      const deploymentInterval = (event.target as HTMLElement).closest<HTMLSelectElement>('[data-deployment-interval]');
      if (deploymentInterval) {
        this.deploymentRefreshSeconds = Number(deploymentInterval.value);
        this.scheduleDeploymentPolling();
        this.updateDeploymentCountdown();
      }
    });
    this.root.querySelector('.admin-table-shell')?.addEventListener('click', (event) => {
      const deploymentRefresh = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-deployment-refresh]');
      if (deploymentRefresh) void this.refreshDeployment(false);
      const userAction = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-user-action]');
      if (userAction) void this.changeUserStatus(userAction);
      const editUser = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-edit-user]');
      if (editUser?.dataset.editUser) this.openUserEditor(this.users.get(editUser.dataset.editUser));
      const resetUser = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-reset-user]');
      if (resetUser?.dataset.resetUser) void this.sendUserPasswordReset(resetUser.dataset.resetUser, resetUser);
      const inspect = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-inspect-room]');
      if (inspect?.dataset.inspectRoom) void this.inspectRoom(inspect.dataset.inspectRoom);
      const tenant = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-inspect-tenant]');
      if (tenant?.dataset.inspectTenant) void this.inspectTenant(tenant.dataset.inspectTenant);
      const saveVoiceAlias = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-save-voice-alias]');
      if (saveVoiceAlias) void this.saveVoiceAlias(saveVoiceAlias);
      const indexAction = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-index-tts-action]');
      if (indexAction) void this.handleIndexTtsAction(indexAction);
    });
    this.root.querySelector('.admin-create-user')?.addEventListener('click', () => this.openUserEditor());
    this.root.querySelector('.admin-import-users')?.addEventListener('click', () => this.openUserImport());
    this.root.querySelector('.admin-create-tenant')?.addEventListener('click', () => this.openTenantEditor());
    this.root.querySelector('.admin-mail-state')?.addEventListener('click', () => {
      this.root?.querySelector<HTMLButtonElement>('[data-section="email"]')?.click();
    });
    this.root.querySelector('.admin-table-shell')?.addEventListener('submit', (event) => {
      const form = event.target as HTMLFormElement;
      if (!form.matches('[data-email-config-form]')) return;
      event.preventDefault();
      void this.saveMailConfig(form);
    });
    const dialog = this.root.querySelector<HTMLDialogElement>('.admin-inspector')!;
    dialog.querySelector('.inspector-close')?.addEventListener('click', () => dialog.close());
    dialog.addEventListener('click', (event) => {
      if (event.target === dialog) dialog.close();
      const action = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-instance-action]');
      if (action) void this.handleInstanceAction(action);
      const copy = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-copy-value]');
      if (copy?.dataset.copyValue) void this.copyCredential(copy.dataset.copyValue);
      const edit = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-edit-tenant]');
      if (edit?.dataset.editTenant) this.openTenantEditor(this.tenants.get(edit.dataset.editTenant));
      const template = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-download-user-template]');
      if (template) this.downloadUserTemplate();
    });
    dialog.addEventListener('close', () => {
      dialog.querySelector<HTMLElement>('.admin-inspector-body')?.replaceChildren();
    });
    dialog.addEventListener('submit', (event) => {
      event.preventDefault();
      const form = event.target as HTMLFormElement;
      if (form.matches('[data-tenant-form]')) void this.saveTenant(form);
      if (form.matches('[data-instance-form]')) void this.createInstance(form);
      if (form.matches('[data-user-form]')) void this.saveUser(form);
      if (form.matches('[data-user-import-form]')) void this.importUsers(form);
    });
  }

  private configureFilters() {
    if (!this.root) return;
    this.root.querySelectorAll<HTMLButtonElement>('[data-section]').forEach((button) => {
      const active = button.dataset.section === this.section;
      button.classList.toggle('active', active);
      button.setAttribute('aria-selected', String(active));
    });
    const query = this.root.querySelector<HTMLInputElement>('[name="q"]')!;
    const compact = ['deployment', 'asr', 'tts', 'email', 'history'].includes(this.section);
    this.root.querySelector<HTMLFormElement>('.admin-filters')!.hidden = compact;
    this.root.querySelector<HTMLElement>('.admin-result-bar')!.hidden = compact;
    if (compact) this.root.querySelector<HTMLElement>('.admin-pagination')!.replaceChildren();
    query.value = this.query;
    query.placeholder = this.section === 'users' ? '账号名称' : this.section === 'rooms' ? '会议名称或创建者' : '租户名称或标识';
    const status = this.root.querySelector<HTMLSelectElement>('[name="status"]')!;
    status.innerHTML = this.section === 'users'
      ? '<option value="">全部状态</option><option value="pending">待验证</option><option value="active">正常</option><option value="suspended">已停用</option>'
      : this.section === 'rooms'
        ? '<option value="">全部状态</option><option value="active">进行中</option><option value="ended">已结束</option><option value="archived">已归档</option>'
        : '<option value="">全部状态</option><option value="active">正常</option><option value="suspended">已暂停</option><option value="revoked">已撤销</option>';
    status.value = this.status;
    const role = this.root.querySelector<HTMLElement>('.admin-role-filter')!;
    role.hidden = this.section !== 'users';
    const sort = this.root.querySelector<HTMLSelectElement>('[name="sort"]')!;
    sort.innerHTML = this.section === 'users'
      ? '<option value="created_at">注册时间</option><option value="username">账号名称</option><option value="last_login">最近登录</option>'
      : this.section === 'rooms'
        ? '<option value="updated_at">最近活动</option><option value="created_at">创建时间</option><option value="name">会议名称</option>'
        : '<option value="created_at">创建时间</option><option value="name">租户名称</option><option value="license_expires_at">授权到期</option>';
    sort.value = this.sort;
    this.root.querySelector<HTMLButtonElement>('.admin-create-user')!.hidden = this.section !== 'users';
    this.root.querySelector<HTMLButtonElement>('.admin-import-users')!.hidden = this.section !== 'users';
    this.root.querySelector<HTMLButtonElement>('.admin-create-tenant')!.hidden = this.section !== 'authority';
    const mailState = this.root.querySelector<HTMLElement>('.admin-mail-state')!;
    mailState.hidden = this.section !== 'users';
    this.updateOrderButton();
  }

  private updateOrderButton() {
    if (!this.root) return;
    const button = this.root.querySelector<HTMLButtonElement>('.admin-order')!;
    button.title = this.order === 'desc' ? '当前降序，切换为升序' : '当前升序，切换为降序';
    button.setAttribute('aria-label', button.title);
    button.innerHTML = `<i data-lucide="${this.order === 'desc' ? 'arrow-down-wide-narrow' : 'arrow-up-narrow-wide'}"></i>`;
    refreshIcons(button);
  }

  private async loadOverview() {
    if (!this.root) return;
    const overview = this.root.querySelector<HTMLElement>('.admin-overview')!;
    overview.setAttribute('aria-busy', 'true');
    try {
      const data = await apiRequest<AdminOverview>('/api/admin/overview');
      if (!this.root) return;
      (['total_users', 'pending_users', 'active_rooms', 'total_rooms'] as const).forEach((key) => {
        this.root?.querySelector<HTMLElement>(`[data-overview="${key}"]`)?.replaceChildren(String(data[key]));
      });
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法加载系统概览');
    } finally {
      overview.setAttribute('aria-busy', 'false');
    }
  }

  private async loadAsrManagement() {
    try {
      this.asrManagement = await apiRequest<AsrManagement>('/api/admin/asr');
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法加载 ASR 配置');
    }
  }

  private async loadTtsManagement() {
    try {
      this.ttsManagement = await apiRequest<TtsManagement>('/api/admin/tts');
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法加载 TTS 配置');
    }
  }

  private async loadMailStatus() {
    try {
      this.mailStatus = await apiRequest<MailStatus>('/api/admin/email/status');
      if (!this.root) return;
      const state = this.root.querySelector<HTMLElement>('.admin-mail-state')!;
      state.className = `admin-mail-state ${this.mailStatus.configured ? 'configured' : 'unconfigured'}`;
      state.title = this.mailStatus.configured
        ? `${this.mailStatus.host}:${this.mailStatus.port} · ${this.mailStatus.from_address}`
        : '打开邮箱配置完成 SMTP 设置';
      state.innerHTML = `<i data-lucide="${this.mailStatus.configured ? 'mail-check' : 'mail-warning'}"></i><span>${this.mailStatus.configured ? 'SMTP 已配置' : 'SMTP 未配置'}</span>`;
      refreshIcons(state);
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法加载邮件配置状态');
    }
  }

  private async loadList() {
    if (!this.root) return;
    if (this.section === 'deployment') this.stopDeploymentPolling();
    const requestId = ++this.requestId;
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    shell.setAttribute('aria-busy', 'true');
    shell.innerHTML = '<div class="admin-loading"><i data-lucide="loader-circle"></i><span>正在加载</span></div>';
    refreshIcons(shell);
    const params = new URLSearchParams({
      sort: this.sort,
      order: this.order,
      page: String(this.page),
      page_size: String(this.pageSize),
    });
    if (this.query.trim()) params.set('q', this.query.trim());
    if (this.status) params.set('status', this.status);
    if (this.section === 'users' && this.role) params.set('role', this.role);
    try {
      if (this.section === 'deployment') {
        const data = await apiRequest<RuntimeSnapshot>('/api/runtime/dependencies');
        if (!this.root || requestId !== this.requestId) return;
        this.deploymentLastError = '';
        this.renderDeployment(data);
        this.scheduleDeploymentPolling();
      } else if (this.section === 'users') {
        const data = await apiRequest<Paginated<AdminUser>>(`/api/admin/users?${params}`);
        if (!this.root || requestId !== this.requestId) return;
        this.users = new Map(data.items.map((user) => [user.id, user]));
        this.renderUsers(data);
      } else if (this.section === 'rooms') {
        const data = await apiRequest<Paginated<RoomSummary>>(`/api/admin/rooms?${params}`);
        if (!this.root || requestId !== this.requestId) return;
        this.renderRooms(data);
      } else if (this.section === 'asr') {
        const data = await apiRequest<AsrManagement>('/api/admin/asr');
        if (!this.root || requestId !== this.requestId) return;
        this.asrManagement = data;
        this.renderAsr(data);
      } else if (this.section === 'tts') {
        const data = await apiRequest<TtsManagement>('/api/admin/tts');
        if (!this.root || requestId !== this.requestId) return;
        this.ttsManagement = data;
        this.renderTts(data);
      } else if (this.section === 'email') {
        const data = await apiRequest<MailStatus>('/api/admin/email/config');
        if (!this.root || requestId !== this.requestId) return;
        this.mailStatus = data;
        this.renderMailConfig(data);
      } else if (this.section === 'history') {
        const historyParams = new URLSearchParams({
          page: String(this.page),
          page_size: String(this.pageSize),
        });
        if (this.historyEntityType) historyParams.set('entity_type', this.historyEntityType);
        const data = await apiRequest<Paginated<ChangeHistoryRecord>>(`/api/admin/change-history?${historyParams}`);
        if (!this.root || requestId !== this.requestId) return;
        this.renderChangeHistory(data);
      } else {
        const data = await apiRequest<Paginated<AuthorityTenant>>(`/api/admin/authority/tenants?${params}`);
        if (!this.root || requestId !== this.requestId) return;
        this.tenants = new Map(data.items.map((tenant) => [tenant.id, tenant]));
        this.renderTenants(data);
      }
    } catch (error) {
      if (requestId !== this.requestId) return;
      shell.innerHTML = '<div class="admin-empty"><i data-lucide="triangle-alert"></i><strong>加载失败</strong></div>';
      refreshIcons(shell);
      this.onError(error instanceof Error ? error.message : '无法加载管理列表');
    } finally {
      if (requestId === this.requestId) shell.setAttribute('aria-busy', 'false');
    }
  }

  private renderDeployment(data: RuntimeSnapshot) {
    if (!this.root) return;
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    const statusLabel = data.overall_status === 'ready' ? '可以接收流量' : data.overall_status === 'degraded' ? '部分能力降级' : '依赖尚未就绪';
    const statusIcon = data.overall_status === 'ready' ? 'circle-check' : data.overall_status === 'degraded' ? 'triangle-alert' : 'circle-x';
    const readyCount = data.dependencies.filter((dependency) => dependency.status === 'ready').length;
    const degradedCount = data.dependencies.filter((dependency) => dependency.status === 'degraded').length;
    const unavailableCount = data.dependencies.filter((dependency) => ['unavailable', 'unknown'].includes(dependency.status)).length;
    const dependencyNames: Record<string, string> = {
      postgresql: 'PostgreSQL',
      system_installation: '系统初始化',
      instance_authorization: '实例授权',
      asr_provider: 'ASR Provider',
      tts_provider: 'TTS Provider',
      public_command_stream: 'Public gRPC 命令流',
      smtp: 'SMTP 邮件服务',
      qwen_tts: 'Qwen3-TTS',
    };
    shell.innerHTML = `
      <section class="deployment-diagnostics">
        <div class="deployment-monitor-bar">
          <div class="deployment-monitor-state"><i></i><span>持续校验</span><strong data-deployment-countdown>${this.deploymentRefreshSeconds ? '准备刷新' : '已暂停'}</strong></div>
          <label><span>自动刷新</span><select data-deployment-interval>
            ${[5, 15, 30, 60].map((seconds) => `<option value="${seconds}" ${seconds === this.deploymentRefreshSeconds ? 'selected' : ''}>${seconds} 秒</option>`).join('')}
            <option value="0" ${this.deploymentRefreshSeconds === 0 ? 'selected' : ''}>暂停</option>
          </select></label>
          <button class="button-secondary deployment-refresh-now" type="button" data-deployment-refresh ${this.deploymentRefreshing ? 'disabled' : ''}><i data-lucide="${this.deploymentRefreshing ? 'loader-circle' : 'refresh-cw'}"></i><span>${this.deploymentRefreshing ? '检测中' : '立即检测'}</span></button>
        </div>
        ${this.deploymentLastError ? `<div class="deployment-stale-alert" role="alert"><i data-lucide="wifi-off"></i><span>自动校验失败，当前展示最近一次成功快照</span><small>${escapeHtml(this.deploymentLastError)}</small></div>` : ''}
        <header class="deployment-summary deployment-status-${escapeAttribute(data.overall_status)}">
          <span class="deployment-summary-icon"><i data-lucide="${statusIcon}"></i></span>
          <div><span>${escapeHtml(data.service)}</span><strong>${statusLabel}</strong><small>版本 ${escapeHtml(data.version)} · 检测于 ${formatDate(data.generated_at)}</small></div>
          <dl><div><dt>正常</dt><dd>${readyCount}</dd></div><div><dt>降级</dt><dd>${degradedCount}</dd></div><div><dt>异常</dt><dd>${unavailableCount}</dd></div><div><dt>必需项</dt><dd>${data.dependencies.filter((dependency) => dependency.required).length}</dd></div></dl>
        </header>
        <div class="deployment-check-list">
          ${data.dependencies.map((dependency) => {
            const label = dependency.status === 'ready' ? '正常' : dependency.status === 'degraded' ? '降级' : dependency.status === 'unavailable' ? '不可用' : '未知';
            const icon = dependency.status === 'ready' ? 'check' : dependency.status === 'degraded' ? 'triangle-alert' : 'x';
            return `<article class="deployment-check deployment-check-${escapeAttribute(dependency.status)}">
              <span class="deployment-check-icon"><i data-lucide="${icon}"></i></span>
              <div><header><strong>${escapeHtml(dependencyNames[dependency.name] ?? dependency.name)}</strong><code>${escapeHtml(dependency.name)}</code><span>${escapeHtml(dependency.kind)}</span>${dependency.required ? '<em>必需</em>' : '<em>可选</em>'}</header><p>${escapeHtml(dependency.message)}</p><small>校验时间 ${formatDate(dependency.checked_at)}</small></div>
              <span class="deployment-check-state">${label}</span>
            </article>`;
          }).join('')}
        </div>
      </section>
    `;
    refreshIcons(shell);
    this.updateDeploymentCountdown();
  }

  private scheduleDeploymentPolling() {
    window.clearTimeout(this.deploymentPollTimer);
    window.clearInterval(this.deploymentCountdownTimer);
    if (!this.root || this.section !== 'deployment' || this.deploymentRefreshSeconds <= 0) {
      this.deploymentNextRefreshAt = 0;
      this.updateDeploymentCountdown();
      return;
    }
    this.deploymentNextRefreshAt = Date.now() + this.deploymentRefreshSeconds * 1_000;
    this.deploymentPollTimer = window.setTimeout(() => void this.refreshDeployment(true), this.deploymentRefreshSeconds * 1_000);
    this.deploymentCountdownTimer = window.setInterval(() => this.updateDeploymentCountdown(), 1_000);
    this.updateDeploymentCountdown();
  }

  private stopDeploymentPolling() {
    window.clearTimeout(this.deploymentPollTimer);
    window.clearInterval(this.deploymentCountdownTimer);
    this.deploymentPollTimer = 0;
    this.deploymentCountdownTimer = 0;
    this.deploymentNextRefreshAt = 0;
  }

  private updateDeploymentCountdown() {
    const countdown = this.root?.querySelector<HTMLElement>('[data-deployment-countdown]');
    if (!countdown) return;
    if (this.deploymentRefreshing) {
      countdown.textContent = '正在执行校验';
    } else if (!this.deploymentRefreshSeconds || !this.deploymentNextRefreshAt) {
      countdown.textContent = '已暂停';
    } else {
      const seconds = Math.max(0, Math.ceil((this.deploymentNextRefreshAt - Date.now()) / 1_000));
      countdown.textContent = `${seconds} 秒后刷新`;
    }
  }

  private async refreshDeployment(silent: boolean) {
    if (!this.root || this.section !== 'deployment' || this.deploymentRefreshing) return;
    window.clearTimeout(this.deploymentPollTimer);
    window.clearInterval(this.deploymentCountdownTimer);
    this.deploymentRefreshing = true;
    this.updateDeploymentCountdown();
    const button = this.root.querySelector<HTMLButtonElement>('[data-deployment-refresh]');
    if (button) {
      button.disabled = true;
      button.innerHTML = '<i data-lucide="loader-circle"></i><span>检测中</span>';
      refreshIcons(button);
    }
    try {
      const data = await apiRequest<RuntimeSnapshot>('/api/runtime/dependencies');
      if (!this.root || this.section !== 'deployment') return;
      this.deploymentLastError = '';
      this.renderDeployment(data);
    } catch (error) {
      if (!this.root || this.section !== 'deployment') return;
      this.deploymentLastError = error instanceof Error ? error.message : '依赖检测接口不可用';
      const alert = this.root.querySelector<HTMLElement>('.deployment-stale-alert');
      if (!alert) {
        const diagnostics = this.root.querySelector<HTMLElement>('.deployment-diagnostics');
        diagnostics?.insertAdjacentHTML('afterbegin', `<div class="deployment-stale-alert" role="alert"><i data-lucide="wifi-off"></i><span>自动校验失败，当前展示最近一次成功快照</span><small>${escapeHtml(this.deploymentLastError)}</small></div>`);
        if (diagnostics) refreshIcons(diagnostics);
      }
      if (!silent) this.onError(this.deploymentLastError);
    } finally {
      this.deploymentRefreshing = false;
      const currentButton = this.root?.querySelector<HTMLButtonElement>('[data-deployment-refresh]');
      if (currentButton) {
        currentButton.disabled = false;
        currentButton.innerHTML = '<i data-lucide="refresh-cw"></i><span>立即检测</span>';
        refreshIcons(currentButton);
      }
      this.scheduleDeploymentPolling();
    }
  }

  private renderMailConfig(data: MailStatus) {
    if (!this.root) return;
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    shell.innerHTML = `
      <section class="email-management">
        <header class="asr-current">
          <span class="asr-current-icon"><i data-lucide="${data.configured ? 'mail-check' : 'mail-warning'}"></i></span>
          <div><span>密码重置邮件</span><strong>${data.configured ? 'SMTP 可以发送' : 'SMTP 尚未就绪'}</strong><small>${escapeHtml(data.host)}:${data.port} · ${escapeHtml(data.from_address)}</small></div>
          <span class="asr-live-status ${data.configured ? '' : 'unavailable'}"><i></i>${data.enabled ? '已启用' : '已停用'}</span>
        </header>
        <form class="email-config-form" data-email-config-form>
          <header><div><span class="section-kicker">VERSIONED CONFIG</span><h2>邮件服务配置</h2></div><label class="email-enabled"><input name="enabled" type="checkbox" ${data.enabled ? 'checked' : ''}><span>启用发送</span></label></header>
          <div class="authority-form-grid">
            <label><span>SMTP 主机</span><input name="host" maxlength="255" required value="${escapeAttribute(data.host)}"></label>
            <label><span>端口</span><input name="port" type="number" min="1" max="65535" required value="${data.port}"></label>
            <label><span>安全模式</span><select name="security"><option value="wrapper" ${data.security === 'wrapper' ? 'selected' : ''}>TLS / SSL</option><option value="starttls" ${data.security === 'starttls' ? 'selected' : ''}>STARTTLS</option><option value="none" ${data.security === 'none' ? 'selected' : ''}>无加密</option></select></label>
            <label><span>SMTP 用户名</span><input name="username" maxlength="255" autocomplete="off" value="${escapeAttribute(data.username)}"></label>
            <label class="account-form-wide"><span>SMTP 密码</span><input name="password" type="password" autocomplete="new-password" placeholder="${data.password_configured ? '已保存，留空保持不变' : '输入 SMTP 密码或授权码'}"></label>
            <label><span>发件邮箱</span><input name="from_address" type="email" maxlength="254" required value="${escapeAttribute(data.from_address)}"></label>
            <label><span>发件人名称</span><input name="from_name" maxlength="128" required value="${escapeAttribute(data.from_name)}"></label>
            <label class="account-form-wide"><span>系统访问地址</span><input name="public_url" type="url" placeholder="https://voice.example.com" value="${escapeAttribute(data.public_url ?? '')}"></label>
            <label><span>重置链接有效期</span><input name="reset_expiry_minutes" type="number" min="5" max="1440" required value="${data.reset_expiry_minutes}"></label>
            <label class="email-clear-password"><input name="clear_password" type="checkbox"><span>清除已保存密码</span></label>
          </div>
          <footer class="authority-form-actions"><span class="email-version-note"><i data-lucide="history"></i>保存会创建新版本，旧配置保留为历史记录</span><button class="button-primary" type="submit"><i data-lucide="save"></i><span>保存配置</span></button></footer>
        </form>
      </section>
    `;
    refreshIcons(shell);
  }

  private async saveMailConfig(form: HTMLFormElement) {
    if (!form.reportValidity()) return;
    const data = new FormData(form);
    const submit = form.querySelector<HTMLButtonElement>('[type="submit"]')!;
    submit.disabled = true;
    try {
      const status = await apiRequest<MailStatus>('/api/admin/email/config', {
        method: 'PUT',
        body: JSON.stringify({
          enabled: form.querySelector<HTMLInputElement>('[name="enabled"]')!.checked,
          host: String(data.get('host') ?? ''),
          port: Number(data.get('port')),
          security: String(data.get('security') ?? 'wrapper'),
          username: String(data.get('username') ?? ''),
          password: String(data.get('password') ?? '') || null,
          clear_password: form.querySelector<HTMLInputElement>('[name="clear_password"]')!.checked,
          from_address: String(data.get('from_address') ?? ''),
          from_name: String(data.get('from_name') ?? ''),
          public_url: String(data.get('public_url') ?? '') || null,
          reset_expiry_minutes: Number(data.get('reset_expiry_minutes')),
        }),
      });
      this.mailStatus = status;
      this.renderMailConfig(status);
      await this.loadMailStatus();
      this.onMessage('邮箱配置新版本已生效');
    } catch (error) {
      submit.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法保存邮箱配置');
    }
  }

  private renderChangeHistory(data: Paginated<ChangeHistoryRecord>) {
    if (!this.root) return;
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    shell.innerHTML = `
      <section class="change-history-management">
        <header class="change-history-heading"><div><span class="section-kicker">IMMUTABLE HISTORY</span><h2>数据变更历史</h2></div><label><span>对象类型</span><select data-history-entity>${historyEntityOptions(this.historyEntityType)}</select></label></header>
        ${data.items.length ? `<table class="admin-table change-history-table">
          <thead><tr><th>时间</th><th>对象</th><th>操作</th><th>版本状态</th><th>变化字段</th></tr></thead>
          <tbody>${data.items.map((item) => `<tr>
            <td><strong class="admin-date">${formatDate(item.created_at)}</strong></td>
            <td><strong>${escapeHtml(historyEntityLabel(item.entity_type))}</strong><code>${escapeHtml(item.entity_id)}</code></td>
            <td><span class="admin-status ${item.action === 'delete' ? 'suspended' : item.action === 'create' ? 'active' : 'pending'}"><i></i>${item.action === 'create' ? '新增' : item.action === 'update' ? '修改' : '删除'}</span></td>
            <td><span class="history-record-status ${escapeAttribute(item.record_status)}">${item.record_status === 'current' ? '当前版本' : item.record_status === 'historical' ? '历史版本' : '已删除'}</span></td>
            <td><span class="history-change-fields">${escapeHtml(changeFieldSummary(item))}</span></td>
          </tr>`).join('')}</tbody>
        </table>` : '<div class="admin-empty"><i data-lucide="history"></i><strong>暂无变更历史</strong></div>'}
      </section>
    `;
    this.renderResultMeta(data);
    refreshIcons(shell);
  }

  private renderAsr(data: AsrManagement) {
    if (!this.root) return;
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    const effective = data.providers.find((provider) => provider.id === data.effective.backend_id);
    const system = data.providers.find((provider) => provider.id === data.system_setting.backend_id);
    const funAsrRuntime = data.fun_asr_runtime;
    const effectiveAvailable = effective?.available !== false
      && (effective?.id !== 'funasr-streaming' || funAsrRuntime.healthy);
    shell.innerHTML = `
      <section class="asr-management">
        <header class="asr-current">
          <span class="asr-current-icon"><i data-lucide="audio-waveform"></i></span>
          <div><span>当前生效</span><strong>${escapeHtml(effective?.name ?? data.effective.backend_id)}</strong><small>${data.effective.source === 'tenant' ? `租户策略 · ${escapeHtml(data.effective.tenant_name ?? '')}` : '系统默认策略'}</small></div>
          <span class="asr-live-status ${effectiveAvailable ? '' : 'offline'}"><i></i>${effectiveAvailable ? '已启用' : '不可用'}</span>
        </header>

        <section class="qwen-tts-runtime ${funAsrRuntime.healthy ? 'healthy' : funAsrRuntime.enabled ? 'unavailable' : 'disabled'}">
          <span class="qwen-tts-runtime-icon"><i data-lucide="radio-tower"></i></span>
          <div><span class="section-kicker">FUNASR STREAMING RUNTIME</span><strong>${funAsrRuntime.healthy ? '流式服务可用' : funAsrRuntime.enabled ? '流式服务不可用' : '尚未启用'}</strong><small>${escapeHtml(funAsrRuntime.message)}</small></div>
          <dl><div><dt>传输</dt><dd>WebSocket</dd></div><div><dt>识别模式</dt><dd>2-pass</dd></div><div><dt>健康检查</dt><dd>${funAsrRuntime.healthy ? '通过' : '未通过'}</dd></div></dl>
        </section>

        <section class="asr-system-setting">
          <div><span class="section-kicker">SYSTEM DEFAULT</span><h2>系统默认后端</h2></div>
          <label>
            <span>ASR Provider</span>
            <select data-system-asr data-current-backend="${escapeAttribute(data.system_setting.backend_id)}" ${data.can_update_system ? '' : 'disabled'}>
              ${this.asrOptions(data.providers, data.system_setting.backend_id, false)}
            </select>
          </label>
          <dl><div><dt>当前默认</dt><dd>${escapeHtml(system?.name ?? data.system_setting.backend_id)}</dd></div><div><dt>更新时间</dt><dd>${formatDate(data.system_setting.updated_at)}</dd></div><div><dt>生效范围</dt><dd>新建音频管线</dd></div></dl>
        </section>

        <section class="asr-provider-list">
          <header><span class="section-kicker">PROVIDERS</span><h2>后端能力</h2></header>
          <table class="admin-table asr-provider-table">
            <thead><tr><th>Provider</th><th>引擎</th><th>用途</th><th>本机状态</th></tr></thead>
            <tbody>${data.providers.map((provider) => `
              <tr>
                <td><span class="asr-provider-name"><i data-lucide="${provider.production ? 'cpu' : 'flask-conical'}"></i><span><strong>${escapeHtml(provider.name)}</strong><code>${escapeHtml(provider.id)}</code></span></span></td>
                <td><strong>${escapeHtml(provider.engine)}</strong></td>
                <td><span class="admin-cell-note">${escapeHtml(provider.description)}</span></td>
                <td><span class="admin-status ${provider.available && (provider.id !== 'funasr-streaming' || funAsrRuntime.healthy) ? 'active' : 'suspended'}"><i></i>${provider.available ? provider.id === 'funasr-streaming' && !funAsrRuntime.healthy ? '连接异常' : '可用' : '未配置'}</span>${provider.production ? '' : '<small class="admin-cell-note warning">非生产</small>'}</td>
              </tr>`).join('')}</tbody>
          </table>
        </section>

      </section>
    `;
    refreshIcons(shell);
  }

  private renderTts(data: TtsManagement) {
    if (!this.root) return;
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    const effective = data.providers.find((provider) => provider.id === data.effective.backend_id);
    const system = data.providers.find((provider) => provider.id === data.system_setting.backend_id);
    const effectiveAvailable = effective?.available !== false;
    const runtime = data.index_tts_runtime;
    const qwenRuntime = data.qwen_tts_runtime;
    const runtimeBusy = Boolean(runtime.action) || ['installing', 'starting', 'stopping'].includes(runtime.phase);
    const runtimeAction = !runtime.model_ready ? 'install' : runtime.running || runtime.healthy ? 'stop' : 'start';
    const runtimeActionLabel = runtimeAction === 'install' ? '安装并启动' : runtimeAction === 'start' ? '启动服务' : '停止服务';
    const runtimeActionIcon = runtimeAction === 'install' ? 'download' : runtimeAction === 'start' ? 'play' : 'square';
    shell.innerHTML = `
      <section class="asr-management">
        <header class="asr-current">
          <span class="asr-current-icon"><i data-lucide="speech"></i></span>
          <div><span>当前生效</span><strong>${escapeHtml(effective?.name ?? data.effective.backend_id)}</strong><small>${data.effective.source === 'tenant' ? `租户策略 · ${escapeHtml(data.effective.tenant_name ?? '')}` : '系统默认策略'}</small></div>
          <span class="asr-live-status ${effectiveAvailable ? '' : 'offline'}"><i></i>${effectiveAvailable ? '已启用' : '不可用'}</span>
        </header>

        <section class="asr-system-setting">
          <div><span class="section-kicker">SYSTEM DEFAULT</span><h2>系统默认后端</h2></div>
          <label>
            <span>TTS Provider</span>
            <select data-system-tts data-current-backend="${escapeAttribute(data.system_setting.backend_id)}" ${data.can_update_system ? '' : 'disabled'}>
              ${this.ttsOptions(data.providers, data.system_setting.backend_id, false)}
            </select>
          </label>
          <dl><div><dt>当前默认</dt><dd>${escapeHtml(system?.name ?? data.system_setting.backend_id)}</dd></div><div><dt>更新时间</dt><dd>${formatDate(data.system_setting.updated_at)}</dd></div><div><dt>生效范围</dt><dd>新建音频管线</dd></div></dl>
        </section>

        <section class="index-tts-runtime" data-phase="${escapeAttribute(runtime.phase)}">
          <span class="index-tts-runtime-icon"><i data-lucide="sparkles"></i></span>
          <div class="index-tts-runtime-copy">
            <span class="section-kicker">INDEXTTS2 RUNTIME</span>
            <strong>${runtime.healthy ? '模型服务可用' : runtime.model_ready ? '模型已安装' : '模型尚未安装'}</strong>
            <small>${escapeHtml(runtime.message)}</small>
          </div>
          <dl>
            <div><dt>模型</dt><dd>${runtime.model_ready ? '完整' : '未下载'}</dd></div>
            <div><dt>进程</dt><dd>${runtime.running ? '运行中' : '已停止'}</dd></div>
            <div><dt>健康检查</dt><dd>${runtime.healthy ? '通过' : '未通过'}</dd></div>
          </dl>
          <div class="index-tts-runtime-actions">
            <button class="button-secondary" type="button" data-index-tts-action="refresh" ${runtimeBusy ? 'disabled' : ''}><i data-lucide="refresh-cw"></i><span>刷新状态</span></button>
            <button class="button-primary" type="button" data-index-tts-action="${runtimeAction}" ${runtimeBusy || !runtime.script_available ? 'disabled' : ''}><i data-lucide="${runtimeBusy ? 'loader-circle' : runtimeActionIcon}"></i><span>${runtimeBusy ? '正在处理' : runtimeActionLabel}</span></button>
          </div>
          <small class="index-tts-runtime-path" title="${escapeAttribute(runtime.model_dir)}">模型目录：${escapeHtml(runtime.model_dir)}</small>
        </section>

        <section class="qwen-tts-runtime ${qwenRuntime.healthy ? 'healthy' : qwenRuntime.enabled ? 'unavailable' : 'disabled'}">
          <span class="qwen-tts-runtime-icon"><i data-lucide="audio-lines"></i></span>
          <div><span class="section-kicker">QWEN3-TTS RUNTIME</span><strong>${qwenRuntime.healthy ? '模型服务可用' : qwenRuntime.enabled ? '模型服务不可用' : '尚未启用'}</strong><small>${escapeHtml(qwenRuntime.message)}</small></div>
          <dl><div><dt>模型</dt><dd>${escapeHtml(qwenRuntime.model)}</dd></div><div><dt>服务地址</dt><dd>${escapeHtml(qwenRuntime.base_url)}</dd></div><div><dt>健康检查</dt><dd>${qwenRuntime.healthy ? '通过' : '未通过'}</dd></div></dl>
        </section>

        <section class="asr-provider-list">
          <header><span class="section-kicker">PROVIDERS</span><h2>后端能力</h2></header>
          <table class="admin-table asr-provider-table">
            <thead><tr><th>Provider</th><th>引擎</th><th>用途</th><th>本机状态</th></tr></thead>
            <tbody>${data.providers.map((provider) => `
              <tr>
                <td><span class="asr-provider-name"><i data-lucide="${provider.id === 'index-tts2' ? 'sparkles' : 'audio-lines'}"></i><span><strong>${escapeHtml(provider.name)}</strong><code>${escapeHtml(provider.id)}</code></span></span></td>
                <td><strong>${escapeHtml(provider.engine)}</strong><small class="admin-cell-note">${provider.voice_clone ? '支持自定义音色' : '预置音色'}</small></td>
                <td><span class="admin-cell-note">${escapeHtml(provider.description)}</span></td>
                <td><span class="admin-status ${provider.available ? 'active' : 'suspended'}"><i></i>${provider.available ? '可用' : '不可用'}</span></td>
              </tr>`).join('')}</tbody>
          </table>
        </section>

        <section class="tts-voice-list">
          <header><span class="section-kicker">VOICES</span><h2>音色与别名</h2><small>${escapeHtml(effective?.name ?? data.effective.backend_id)} · ${data.voices.length} 个可选音色</small></header>
          <table class="admin-table tts-voice-table">
            <thead><tr><th>音色</th><th>稳定 ID</th><th>支持语言</th><th>人工别名</th></tr></thead>
            <tbody>${data.voices.map((voice) => `
              <tr>
                <td><strong>${escapeHtml(voice.display_name)}</strong><small class="admin-cell-note">默认：${escapeHtml(voice.default_name)}</small></td>
                <td><code>${escapeHtml(voice.id)}</code><small class="admin-cell-note">${escapeHtml(voice.group)}</small></td>
                <td><span class="tts-language-list">${voice.languages.map((language) => `<span>${escapeHtml(language.toUpperCase())}</span>`).join('')}</span><small class="admin-cell-note">${escapeHtml(voice.description)}</small></td>
                <td><span class="tts-alias-editor"><input data-voice-alias maxlength="64" value="${escapeAttribute(voice.alias ?? '')}" placeholder="${escapeAttribute(voice.default_name)}" aria-label="${escapeAttribute(voice.default_name)} 的人工别名"><button class="icon-button" type="button" data-save-voice-alias="${escapeAttribute(voice.id)}" title="保存别名" aria-label="保存 ${escapeAttribute(voice.default_name)} 的别名"><i data-lucide="save"></i></button></span><small class="admin-cell-note">留空保存可恢复默认名称</small></td>
              </tr>`).join('')}</tbody>
          </table>
        </section>
      </section>
    `;
    refreshIcons(shell);
    this.scheduleTtsRuntimePoll(runtime.phase);
  }

  private scheduleTtsRuntimePoll(phase: string) {
    window.clearTimeout(this.ttsPollTimer);
    if (!this.root || this.section !== 'tts' || !['installing', 'starting', 'stopping'].includes(phase)) return;
    this.ttsPollTimer = window.setTimeout(() => void this.refreshTtsView(), 3_000);
  }

  private async refreshTtsView() {
    if (!this.root || this.section !== 'tts') return;
    try {
      const data = await apiRequest<TtsManagement>('/api/admin/tts');
      if (!this.root || this.section !== 'tts') return;
      this.ttsManagement = data;
      this.renderTts(data);
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法刷新 IndexTTS2 状态');
    }
  }

  private async handleIndexTtsAction(button: HTMLButtonElement) {
    const action = button.dataset.indexTtsAction;
    if (!action) return;
    if (action === 'refresh') {
      button.disabled = true;
      await this.refreshTtsView();
      return;
    }
    if (action === 'install' && !window.confirm('将下载约 5.9 GB 的 IndexTTS2 模型并安装独立运行环境，是否继续？')) return;
    button.disabled = true;
    try {
      await apiRequest(`/api/admin/tts/index-tts/${action}`, { method: 'POST' });
      this.onMessage(action === 'install' ? 'IndexTTS2 已开始后台安装' : action === 'start' ? 'IndexTTS2 正在启动' : 'IndexTTS2 正在停止');
      await this.refreshTtsView();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : 'IndexTTS2 操作失败');
      await this.refreshTtsView();
    }
  }

  private renderUsers(data: Paginated<AdminUser>) {
    if (!this.root) return;
    this.renderResultMeta(data);
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    if (!data.items.length) {
      this.renderEmpty(shell, '没有符合条件的人员');
      return;
    }
    shell.innerHTML = `
      <table class="admin-table admin-users-table">
        <thead><tr><th>人员</th><th>状态</th><th>角色</th><th>使用情况</th><th>最近活动</th><th aria-label="操作"></th></tr></thead>
        <tbody>${data.items.map((user) => this.userRow(user)).join('')}</tbody>
      </table>
    `;
    refreshIcons(shell);
  }

  private userRow(user: AdminUser) {
    const [statusLabel, statusClass] = USER_STATUS[user.status];
    const isSelf = user.id === this.currentUser.id;
    const nextStatus = user.status === 'active' ? 'suspended' : 'active';
    const actionLabel = user.status === 'pending' ? '通过验证' : user.status === 'active' ? '停用账号' : '恢复账号';
    const actionIcon = user.status === 'active' ? 'user-x' : 'user-check';
    return `
      <tr>
        <td><span class="admin-identity"><span class="admin-avatar">${escapeHtml(Array.from(user.username)[0]?.toUpperCase() ?? 'U')}</span><span><strong>${escapeHtml(user.username)}${isSelf ? '<small>当前账号</small>' : ''}</strong><small class="admin-user-email">${escapeHtml(user.email ?? '未设置邮箱')}</small></span></span></td>
        <td><span class="admin-status ${statusClass}"><i></i>${statusLabel}</span><small class="admin-cell-note">${user.verified_at ? `验证于 ${formatDate(user.verified_at)}` : '尚未验证'}</small></td>
        <td><select class="admin-inline-select" data-user-role="${user.id}" data-user-status="${user.status}" ${isSelf ? 'disabled title="不能修改当前账号"' : ''}><option value="member" ${user.role === 'member' ? 'selected' : ''}>普通成员</option><option value="admin" ${user.role === 'admin' ? 'selected' : ''}>系统管理员</option></select></td>
        <td><span class="admin-usage"><span title="创建的会议"><i data-lucide="crown"></i>${user.owned_room_count}</span><span title="参加的会议"><i data-lucide="users"></i>${user.joined_room_count}</span><span title="发言记录"><i data-lucide="messages-square"></i>${user.utterance_count}</span></span></td>
        <td><strong class="admin-date">${formatDate(user.last_activity_at ?? user.last_login_at ?? user.created_at)}</strong><small class="admin-cell-note">注册于 ${formatDate(user.created_at)}</small></td>
        <td class="admin-row-action"><span class="admin-row-buttons"><button class="icon-button" type="button" data-edit-user="${user.id}" title="编辑账号" aria-label="编辑 ${escapeAttribute(user.username)}"><i data-lucide="pencil"></i></button><button class="icon-button" type="button" data-reset-user="${user.id}" title="发送密码重置链接" aria-label="为 ${escapeAttribute(user.username)} 发送密码重置链接" ${!user.email || !this.mailStatus?.configured ? 'disabled' : ''}><i data-lucide="key-round"></i></button><button class="icon-button ${user.status === 'active' ? 'danger' : ''}" type="button" data-user-action="${user.id}" data-user-role="${user.role}" data-current-status="${user.status}" data-next-status="${nextStatus}" title="${actionLabel}" aria-label="${actionLabel}" ${isSelf ? 'disabled' : ''}><i data-lucide="${actionIcon}"></i></button></span></td>
      </tr>
    `;
  }

  private renderRooms(data: Paginated<RoomSummary>) {
    if (!this.root) return;
    this.renderResultMeta(data);
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    if (!data.items.length) {
      this.renderEmpty(shell, '没有符合条件的会议');
      return;
    }
    shell.innerHTML = `
      <table class="admin-table admin-rooms-table">
        <thead><tr><th>会议</th><th>创建者</th><th>状态</th><th>规模</th><th>最近活动</th><th aria-label="操作"></th></tr></thead>
        <tbody>${data.items.map((room) => this.roomRow(room)).join('')}</tbody>
      </table>
    `;
    refreshIcons(shell);
  }

  private roomRow(room: RoomSummary) {
    return `
      <tr>
        <td><span class="admin-room-name"><span><i data-lucide="audio-lines"></i></span><span><strong>${escapeHtml(room.name)}</strong><code>${escapeHtml(shortId(room.id))}</code></span></span></td>
        <td><strong>${escapeHtml(room.owner_username)}</strong><small class="admin-cell-note">${escapeHtml(shortId(room.owner_id))}</small></td>
        <td><select class="admin-inline-select room-status-select ${ROOM_STATUS[room.status][1]}" data-room-status="${room.id}" data-current-status="${room.status}"><option value="active" ${room.status === 'active' ? 'selected' : ''}>进行中</option><option value="ended" ${room.status === 'ended' ? 'selected' : ''}>已结束</option><option value="archived" ${room.status === 'archived' ? 'selected' : ''}>已归档</option></select></td>
        <td><span class="admin-usage"><span title="成员"><i data-lucide="users"></i>${room.member_count}</span><span title="字幕记录"><i data-lucide="messages-square"></i>${room.utterance_count}</span><span title="累计时长"><i data-lucide="clock-3"></i>${formatDuration(room.duration_ms)}</span></span></td>
        <td><strong class="admin-date">${formatDate(room.last_activity_at)}</strong><small class="admin-cell-note">创建于 ${formatDate(room.created_at)}</small></td>
        <td class="admin-row-action"><button class="icon-button" type="button" data-inspect-room="${room.id}" title="隐身进入" aria-label="隐身进入 ${escapeHtml(room.name)}"><i data-lucide="eye-off"></i></button></td>
      </tr>
    `;
  }

  private renderTenants(data: Paginated<AuthorityTenant>) {
    if (!this.root) return;
    this.renderResultMeta(data);
    const shell = this.root.querySelector<HTMLElement>('.admin-table-shell')!;
    if (!data.items.length) {
      this.renderEmpty(shell, '没有符合条件的租户');
      return;
    }
    shell.innerHTML = `
      <table class="admin-table admin-tenants-table">
        <thead><tr><th>租户</th><th>状态</th><th>ASR 后端</th><th>TTS 后端</th><th>授权期限</th><th>实例</th><th>最近校验</th><th aria-label="操作"></th></tr></thead>
        <tbody>${data.items.map((tenant) => this.tenantRow(tenant)).join('')}</tbody>
      </table>
    `;
    refreshIcons(shell);
  }

  private tenantRow(tenant: AuthorityTenant) {
    const [label, className] = TENANT_STATUS[tenant.status];
    const expiresSoon = new Date(tenant.license_expires_at).getTime() <= Date.now() + tenant.warning_days * 86_400_000;
    return `
      <tr>
        <td><span class="admin-room-name"><span><i data-lucide="building-2"></i></span><span><strong>${escapeHtml(tenant.name)}</strong><code>${escapeHtml(tenant.slug)}</code></span></span></td>
        <td><select class="admin-inline-select tenant-status-select ${className}" data-tenant-status="${tenant.id}" data-current-status="${tenant.status}"><option value="active" ${tenant.status === 'active' ? 'selected' : ''}>正常</option><option value="suspended" ${tenant.status === 'suspended' ? 'selected' : ''}>已暂停</option><option value="revoked" ${tenant.status === 'revoked' ? 'selected' : ''}>已撤销</option></select><small class="admin-cell-note">${label}</small></td>
        <td><select class="admin-inline-select tenant-asr-select" data-tenant-asr="${tenant.id}" data-current-backend="${escapeAttribute(tenant.asr_backend_id ?? '')}">${this.asrOptions(this.asrManagement?.providers ?? [], tenant.asr_backend_id ?? '', true)}</select><small class="admin-cell-note">${tenant.asr_backend_id ? '租户覆盖' : '继承系统默认'}</small></td>
        <td><select class="admin-inline-select tenant-tts-select" data-tenant-tts="${tenant.id}" data-current-backend="${escapeAttribute(tenant.tts_backend_id ?? '')}">${this.ttsOptions(this.ttsManagement?.providers ?? [], tenant.tts_backend_id ?? '', true)}</select><small class="admin-cell-note">${tenant.tts_backend_id ? '租户覆盖' : '继承系统默认'}</small></td>
        <td><strong class="admin-date ${expiresSoon ? 'warning' : ''}">${formatDate(tenant.license_expires_at)}</strong><small class="admin-cell-note">宽限至 ${formatDate(tenant.grace_ends_at)}</small></td>
        <td><span class="admin-usage"><span title="部署实例"><i data-lucide="server"></i>${tenant.instance_count}</span><span title="离线租约"><i data-lucide="timer"></i>${formatLease(tenant.offline_lease_minutes)}</span></span></td>
        <td><strong class="admin-date">${tenant.last_seen_at ? formatDate(tenant.last_seen_at) : '尚未校验'}</strong><small class="admin-cell-note">提前 ${tenant.warning_days} 天提醒</small></td>
        <td class="admin-row-action"><button class="icon-button" type="button" data-inspect-tenant="${tenant.id}" title="管理租户授权" aria-label="管理 ${escapeAttribute(tenant.name)}"><i data-lucide="settings-2"></i></button></td>
      </tr>
    `;
  }

  private asrOptions(providers: AsrProvider[], selected: string, includeInherited: boolean) {
    const inherited = this.asrManagement?.providers.find(
      (provider) => provider.id === this.asrManagement?.system_setting.backend_id,
    );
    const options = includeInherited
      ? `<option value="" ${selected ? '' : 'selected'}>继承系统 · ${escapeHtml(inherited?.name ?? '默认后端')}</option>`
      : '';
    return options + providers.map((provider) => {
      const runtimeAvailable = provider.available
        && (provider.id !== 'funasr-streaming' || this.asrManagement?.fun_asr_runtime.healthy);
      return `
        <option value="${escapeAttribute(provider.id)}" ${provider.id === selected ? 'selected' : ''} ${runtimeAvailable ? '' : 'disabled'}>${escapeHtml(provider.name)}${provider.production ? '' : ' · 非生产'}${provider.available ? runtimeAvailable ? '' : ' · 连接异常' : ' · 未配置'}</option>
      `;
    }).join('');
  }

  private ttsOptions(providers: TtsProvider[], selected: string, includeInherited: boolean) {
    const inherited = this.ttsManagement?.providers.find(
      (provider) => provider.id === this.ttsManagement?.system_setting.backend_id,
    );
    const options = includeInherited
      ? `<option value="" ${selected ? '' : 'selected'}>继承系统 · ${escapeHtml(inherited?.name ?? '默认后端')}</option>`
      : '';
    return options + providers.map((provider) => `
      <option value="${escapeAttribute(provider.id)}" ${provider.id === selected ? 'selected' : ''} ${provider.available ? '' : 'disabled'}>${escapeHtml(provider.name)}${provider.available ? '' : ' · 不可用'}</option>
    `).join('');
  }

  private renderResultMeta<T>(data: Paginated<T>) {
    if (!this.root) return;
    this.root.querySelector<HTMLElement>('.admin-result-count')!.textContent = `${data.total} 个结果`;
    const nav = this.root.querySelector<HTMLElement>('.admin-pagination')!;
    if (data.total_pages <= 1) {
      nav.replaceChildren();
      return;
    }
    const pages = paginationWindow(data.page, data.total_pages);
    nav.innerHTML = `
      <button class="icon-button" type="button" data-page="${data.page - 1}" title="上一页" aria-label="上一页" ${data.page <= 1 ? 'disabled' : ''}><i data-lucide="chevron-left"></i></button>
      ${pages.map((page) => page === 0 ? '<span class="admin-page-gap">…</span>' : `<button type="button" class="admin-page-number ${page === data.page ? 'active' : ''}" data-page="${page}" ${page === data.page ? 'aria-current="page"' : ''}>${page}</button>`).join('')}
      <button class="icon-button" type="button" data-page="${data.page + 1}" title="下一页" aria-label="下一页" ${data.page >= data.total_pages ? 'disabled' : ''}><i data-lucide="chevron-right"></i></button>
    `;
    refreshIcons(nav);
  }

  private renderEmpty(shell: HTMLElement, message: string) {
    shell.innerHTML = `<div class="admin-empty"><i data-lucide="search-x"></i><strong>${message}</strong></div>`;
    refreshIcons(shell);
  }

  private async changeUserStatus(button: HTMLButtonElement) {
    const id = button.dataset.userAction;
    const role = button.dataset.userRole;
    const currentStatus = button.dataset.currentStatus;
    const status = button.dataset.nextStatus;
    if (!id || !role || !currentStatus || !status) return;
    button.disabled = true;
    try {
      await apiRequest(`/api/admin/users/${encodeURIComponent(id)}`, {
        method: 'PATCH',
        body: JSON.stringify({ role, status }),
      });
      this.onMessage(
        currentStatus === 'pending'
          ? '人员已通过验证'
          : status === 'active'
            ? '人员状态已恢复'
            : '人员已停用并撤销登录会话',
      );
      await Promise.all([this.loadOverview(), this.loadList()]);
    } catch (error) {
      button.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法更新人员状态');
    }
  }

  private async changeUserRole(select: HTMLSelectElement) {
    const id = select.dataset.userRole;
    const status = select.dataset.userStatus;
    if (!id || !status) return;
    select.disabled = true;
    try {
      await apiRequest(`/api/admin/users/${encodeURIComponent(id)}`, {
        method: 'PATCH',
        body: JSON.stringify({ role: select.value, status }),
      });
      this.onMessage(select.value === 'admin' ? '已授予系统管理员权限' : '已调整为普通成员');
      await this.loadList();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法更新人员角色');
      await this.loadList();
    }
  }

  private openUserEditor(user?: AdminUser) {
    if (!this.root) return;
    const dialog = this.root.querySelector<HTMLDialogElement>('.admin-inspector')!;
    const body = dialog.querySelector<HTMLElement>('.admin-inspector-body')!;
    const isSelf = user?.id === this.currentUser.id;
    this.setInspectorHeading(user ? '编辑用户' : '新建用户', 'ACCOUNT MANAGEMENT', 'user-cog');
    body.innerHTML = `
      <form class="authority-form account-form" data-user-form data-user-id="${user?.id ?? ''}">
        <div class="account-form-heading"><span><i data-lucide="${user ? 'user-cog' : 'user-plus'}"></i></span><div><strong>${user ? escapeHtml(user.username) : '创建本租户账号'}</strong><small>${user ? '更新邮箱、角色或账号状态' : '账号资料只保存在当前服务实例'}</small></div></div>
        <div class="authority-form-grid">
          <label><span>账号名称</span><input name="username" minlength="3" maxlength="32" pattern="[A-Za-z0-9_-]+" required autocomplete="off" value="${escapeAttribute(user?.username ?? '')}" ${user ? 'disabled' : ''}></label>
          <label><span>邮箱地址</span><input name="email" type="email" maxlength="254" required autocomplete="off" value="${escapeAttribute(user?.email ?? '')}"></label>
          ${user ? '' : '<label class="account-form-wide"><span>初始密码</span><input name="password" type="password" minlength="8" maxlength="128" required autocomplete="new-password"></label>'}
          <label><span>角色</span><select name="role" ${isSelf ? 'disabled' : ''}><option value="member" ${!user || user.role === 'member' ? 'selected' : ''}>普通成员</option><option value="admin" ${user?.role === 'admin' ? 'selected' : ''}>系统管理员</option></select></label>
          <label><span>状态</span><select name="status" ${isSelf ? 'disabled' : ''}><option value="active" ${!user || user.status === 'active' ? 'selected' : ''}>正常</option><option value="pending" ${user?.status === 'pending' ? 'selected' : ''}>待验证</option><option value="suspended" ${user?.status === 'suspended' ? 'selected' : ''}>已停用</option></select></label>
        </div>
        <p class="account-form-note"><i data-lucide="shield-check"></i><span>${user ? '修改邮箱后，后续密码重置链接将发送到新地址。' : '初始密码不会通过邮件发送；用户可在登录页使用邮箱自行重置。'}</span></p>
        <footer class="authority-form-actions"><button type="button" class="button-secondary" onclick="this.closest('dialog').close()">取消</button><button type="submit" class="button-primary"><i data-lucide="save"></i><span>${user ? '保存账号' : '创建用户'}</span></button></footer>
      </form>
    `;
    refreshIcons(body);
    if (!dialog.open) dialog.showModal();
  }

  private async saveUser(form: HTMLFormElement) {
    if (!form.reportValidity()) return;
    const data = new FormData(form);
    const userId = form.dataset.userId;
    const current = userId ? this.users.get(userId) : undefined;
    const submit = form.querySelector<HTMLButtonElement>('[type="submit"]')!;
    const payload = userId
      ? {
          email: String(data.get('email') ?? ''),
          role: String(data.get('role') ?? current?.role ?? 'member'),
          status: String(data.get('status') ?? current?.status ?? 'active'),
        }
      : {
          username: String(data.get('username') ?? ''),
          email: String(data.get('email') ?? ''),
          password: String(data.get('password') ?? ''),
          role: String(data.get('role') ?? 'member'),
          status: String(data.get('status') ?? 'active'),
        };
    submit.disabled = true;
    try {
      await apiRequest(userId ? `/api/admin/users/${encodeURIComponent(userId)}` : '/api/admin/users', {
        method: userId ? 'PATCH' : 'POST',
        body: JSON.stringify(payload),
      });
      this.root?.querySelector<HTMLDialogElement>('.admin-inspector')?.close();
      this.onMessage(userId ? '账号资料已更新' : '用户已创建');
      await Promise.all([this.loadOverview(), this.loadList()]);
    } catch (error) {
      submit.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法保存用户');
    }
  }

  private openUserImport() {
    if (!this.root) return;
    const dialog = this.root.querySelector<HTMLDialogElement>('.admin-inspector')!;
    const body = dialog.querySelector<HTMLElement>('.admin-inspector-body')!;
    this.setInspectorHeading('批量导入用户', 'BULK IMPORT', 'file-spreadsheet');
    body.innerHTML = `
      <form class="authority-form account-import-form" data-user-import-form>
        <div class="account-form-heading"><span><i data-lucide="file-spreadsheet"></i></span><div><strong>CSV 用户清单</strong><small>UTF-8 编码，首行为字段名称，单次最多 500 人</small></div></div>
        <div class="account-import-columns"><code>username</code><code>email</code><code>password</code><code>role</code><code>status</code></div>
        <label class="account-file-field"><span>选择 CSV 文件</span><input name="file" type="file" accept=".csv,text/csv" required></label>
        <p class="account-form-note"><i data-lucide="circle-alert"></i><span>导入前会整批校验账号、邮箱、密码、角色和状态；任意一行错误时不会写入任何用户。</span></p>
        <footer class="authority-form-actions"><button type="button" class="button-secondary" data-download-user-template><i data-lucide="download"></i><span>下载模板</span></button><span class="account-form-spacer"></span><button type="button" class="button-secondary" onclick="this.closest('dialog').close()">取消</button><button type="submit" class="button-primary"><i data-lucide="file-up"></i><span>开始导入</span></button></footer>
      </form>
    `;
    refreshIcons(body);
    if (!dialog.open) dialog.showModal();
  }

  private async importUsers(form: HTMLFormElement) {
    if (!form.reportValidity()) return;
    const submit = form.querySelector<HTMLButtonElement>('[type="submit"]')!;
    submit.disabled = true;
    try {
      const result = await apiRequest<{ imported: number }>('/api/admin/users/import', {
        method: 'POST',
        body: new FormData(form),
      });
      this.root?.querySelector<HTMLDialogElement>('.admin-inspector')?.close();
      this.onMessage(`已导入 ${result.imported} 个用户`);
      await Promise.all([this.loadOverview(), this.loadList()]);
    } catch (error) {
      submit.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法导入用户');
    }
  }

  private downloadUserTemplate() {
    const content = '\uFEFFusername,email,password,role,status\nzhangsan,zhangsan@example.com,ChangeMe123,member,active\n';
    const url = URL.createObjectURL(new Blob([content], { type: 'text/csv;charset=utf-8' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'voice-elf-users-template.csv';
    anchor.click();
    URL.revokeObjectURL(url);
  }

  private async sendUserPasswordReset(userId: string, button: HTMLButtonElement) {
    const user = this.users.get(userId);
    if (!user?.email) return;
    if (!window.confirm(`向 ${user.email} 发送一次性密码重置链接？`)) return;
    button.disabled = true;
    try {
      await apiRequest(`/api/admin/users/${encodeURIComponent(userId)}/password-reset`, { method: 'POST' });
      this.onMessage(`重置链接已发送至 ${user.email}`);
    } catch (error) {
      button.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法发送密码重置链接');
    }
  }

  private async changeRoomStatus(select: HTMLSelectElement) {
    const id = select.dataset.roomStatus;
    const previous = select.dataset.currentStatus;
    if (!id || !previous || previous === select.value) return;
    const next = select.value as RoomSummary['status'];
    if (next !== 'active' && !window.confirm(next === 'archived' ? '归档后，普通成员将无法在列表或链接中访问该会议。确定继续吗？' : '结束会议会立即断开实时语音连接。确定继续吗？')) {
      select.value = previous;
      return;
    }
    select.disabled = true;
    try {
      await apiRequest(`/api/admin/rooms/${encodeURIComponent(id)}`, {
        method: 'PATCH',
        body: JSON.stringify({ status: next }),
      });
      this.onMessage(`会议状态已更新为${ROOM_STATUS[next][0]}`);
      await Promise.all([this.loadOverview(), this.loadList()]);
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法更新会议状态');
      await this.loadList();
    }
  }

  private async changeTenantStatus(select: HTMLSelectElement) {
    const id = select.dataset.tenantStatus;
    const previous = select.dataset.currentStatus as AuthorityTenant['status'] | undefined;
    const tenant = id ? this.tenants.get(id) : undefined;
    if (!tenant || !previous || previous === select.value) return;
    const status = select.value as AuthorityTenant['status'];
    if (status !== 'active' && !window.confirm(status === 'revoked' ? '撤销后，该租户的所有部署实例将停止业务访问。确定继续吗？' : '暂停后，该租户的所有部署实例将停止业务访问。确定继续吗？')) {
      select.value = previous;
      return;
    }
    select.disabled = true;
    try {
      await apiRequest(`/api/admin/authority/tenants/${encodeURIComponent(id!)}`, {
        method: 'PATCH',
        body: JSON.stringify(this.tenantPayload(tenant, status)),
      });
      this.onMessage(`租户状态已更新为${TENANT_STATUS[status][0]}`);
      await this.loadList();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法更新租户状态');
      await this.loadList();
    }
  }

  private async changeSystemAsr(select: HTMLSelectElement) {
    const previous = select.dataset.currentBackend ?? '';
    if (!select.value || select.value === previous) return;
    if (select.value === 'demo' && !window.confirm('Demo 只输出占位文本，不执行真实语音识别。确定切换吗？')) {
      select.value = previous;
      return;
    }
    select.disabled = true;
    try {
      this.asrManagement = await apiRequest<AsrManagement>('/api/admin/asr', {
        method: 'PATCH',
        body: JSON.stringify({ backend_id: select.value }),
      });
      this.onMessage('系统 ASR 默认后端已更新，新建音频管线将使用该配置');
      this.renderAsr(this.asrManagement);
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法更新系统 ASR 后端');
      await this.loadList();
    }
  }

  private async changeTenantAsr(select: HTMLSelectElement) {
    const tenantId = select.dataset.tenantAsr;
    const previous = select.dataset.currentBackend ?? '';
    if (!tenantId || select.value === previous) return;
    if (select.value === 'demo' && !window.confirm('该租户将使用占位识别，不会产生真实转写。确定继续吗？')) {
      select.value = previous;
      return;
    }
    select.disabled = true;
    try {
      await apiRequest(`/api/admin/authority/tenants/${encodeURIComponent(tenantId)}/asr`, {
        method: 'PATCH',
        body: JSON.stringify({ backend_id: select.value || null }),
      });
      this.onMessage(select.value ? '租户 ASR 覆盖已更新' : '租户已恢复继承系统 ASR 默认配置');
      await this.loadList();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法更新租户 ASR 后端');
      await this.loadList();
    }
  }

  private async changeSystemTts(select: HTMLSelectElement) {
    const previous = select.dataset.currentBackend ?? '';
    if (!select.value || select.value === previous) return;
    select.disabled = true;
    try {
      this.ttsManagement = await apiRequest<TtsManagement>('/api/admin/tts', {
        method: 'PATCH',
        body: JSON.stringify({ backend_id: select.value }),
      });
      this.onMessage('系统 TTS 默认后端已更新，新建音频管线将使用该配置');
      this.renderTts(this.ttsManagement);
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法更新系统 TTS 后端');
      await this.loadList();
    }
  }

  private async changeTenantTts(select: HTMLSelectElement) {
    const tenantId = select.dataset.tenantTts;
    const previous = select.dataset.currentBackend ?? '';
    if (!tenantId || select.value === previous) return;
    select.disabled = true;
    try {
      await apiRequest(`/api/admin/authority/tenants/${encodeURIComponent(tenantId)}/tts`, {
        method: 'PATCH',
        body: JSON.stringify({ backend_id: select.value || null }),
      });
      this.onMessage(select.value ? '租户 TTS 覆盖已更新' : '租户已恢复继承系统 TTS 默认配置');
      await this.loadList();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法更新租户 TTS 后端');
      await this.loadList();
    }
  }

  private async saveVoiceAlias(button: HTMLButtonElement) {
    const voiceId = button.dataset.saveVoiceAlias;
    const input = button.closest('tr')?.querySelector<HTMLInputElement>('[data-voice-alias]');
    if (!voiceId || !input || !this.ttsManagement) return;
    button.disabled = true;
    input.disabled = true;
    try {
      this.ttsManagement = await apiRequest<TtsManagement>(
        `/api/admin/tts/voices/${encodeURIComponent(voiceId)}`,
        {
          method: 'PATCH',
          body: JSON.stringify({ alias: input.value.trim() || null }),
        },
      );
      this.onMessage(input.value.trim() ? '音色别名已更新' : '音色已恢复默认名称');
      this.renderTts(this.ttsManagement);
    } catch (error) {
      button.disabled = false;
      input.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法更新音色别名');
    }
  }

  private openTenantEditor(tenant?: AuthorityTenant) {
    if (!this.root) return;
    const dialog = this.root.querySelector<HTMLDialogElement>('.admin-inspector')!;
    const body = dialog.querySelector<HTMLElement>('.admin-inspector-body')!;
    this.setInspectorHeading(tenant ? '编辑租户' : '创建租户', 'TENANT AUTHORITY', 'key-round');
    body.innerHTML = this.tenantForm(tenant);
    refreshIcons(dialog);
    if (!dialog.open) dialog.showModal();
  }

  private tenantForm(tenant?: AuthorityTenant) {
    const expiry = tenant?.license_expires_at ?? new Date(Date.now() + 365 * 86_400_000).toISOString();
    const grace = tenant?.grace_ends_at ?? new Date(Date.now() + 395 * 86_400_000).toISOString();
    return `
      <form class="authority-form" data-tenant-form data-tenant-id="${tenant?.id ?? ''}">
        <div class="authority-form-grid">
          <label><span>租户名称</span><input name="name" required maxlength="120" value="${escapeAttribute(tenant?.name ?? '')}"></label>
          <label><span>租户标识</span><input name="slug" required minlength="3" maxlength="48" pattern="[a-z0-9](?:[a-z0-9-]*[a-z0-9])?" value="${escapeHtml(tenant?.slug ?? '')}" ${tenant ? 'disabled' : ''}></label>
          <label><span>状态</span><select name="status"><option value="active" ${!tenant || tenant.status === 'active' ? 'selected' : ''}>正常</option><option value="suspended" ${tenant?.status === 'suspended' ? 'selected' : ''}>已暂停</option><option value="revoked" ${tenant?.status === 'revoked' ? 'selected' : ''}>已撤销</option></select></label>
          <label><span>提前提醒（天）</span><input type="number" name="warning_days" min="1" max="180" value="${tenant?.warning_days ?? 30}" required></label>
          <label><span>授权到期</span><input type="datetime-local" name="license_expires_at" value="${toDateTimeInput(expiry)}" required></label>
          <label><span>宽限期结束</span><input type="datetime-local" name="grace_ends_at" value="${toDateTimeInput(grace)}" required></label>
          <label><span>离线租约（分钟）</span><input type="number" name="offline_lease_minutes" min="5" max="10080" value="${tenant?.offline_lease_minutes ?? 1440}" required></label>
        </div>
        <footer class="authority-form-actions"><button type="button" class="button-secondary" onclick="this.closest('dialog').close()">取消</button><button type="submit" class="button-primary"><i data-lucide="save"></i><span>${tenant ? '保存设置' : '创建租户'}</span></button></footer>
      </form>
    `;
  }

  private async saveTenant(form: HTMLFormElement) {
    if (!form.reportValidity()) return;
    const submit = form.querySelector<HTMLButtonElement>('[type="submit"]')!;
    const data = new FormData(form);
    const tenantId = form.dataset.tenantId;
    const payload: Record<string, unknown> = {
      name: String(data.get('name') ?? ''),
      status: String(data.get('status') ?? 'active'),
      license_expires_at: new Date(String(data.get('license_expires_at'))).toISOString(),
      grace_ends_at: new Date(String(data.get('grace_ends_at'))).toISOString(),
      warning_days: Number(data.get('warning_days')),
      offline_lease_minutes: Number(data.get('offline_lease_minutes')),
    };
    if (!tenantId) payload.slug = String(data.get('slug') ?? '');
    submit.disabled = true;
    try {
      await apiRequest(tenantId ? `/api/admin/authority/tenants/${encodeURIComponent(tenantId)}` : '/api/admin/authority/tenants', {
        method: tenantId ? 'PATCH' : 'POST',
        body: JSON.stringify(payload),
      });
      this.root?.querySelector<HTMLDialogElement>('.admin-inspector')?.close();
      this.onMessage(tenantId ? '租户授权设置已保存' : '租户已创建，可以继续签发部署凭据');
      await this.loadList();
    } catch (error) {
      submit.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法保存租户');
    }
  }

  private async inspectTenant(tenantId: string, issued?: IssuedAuthorityCredential) {
    if (!this.root) return;
    const tenant = this.tenants.get(tenantId);
    if (!tenant) return;
    const dialog = this.root.querySelector<HTMLDialogElement>('.admin-inspector')!;
    const body = dialog.querySelector<HTMLElement>('.admin-inspector-body')!;
    this.setInspectorHeading('租户授权', 'TENANT AUTHORITY', 'key-round');
    body.innerHTML = '<div class="admin-loading"><i data-lucide="loader-circle"></i><span>正在读取部署实例</span></div>';
    refreshIcons(body);
    if (!dialog.open) dialog.showModal();
    try {
      const instances = await apiRequest<AuthorityInstance[]>(`/api/admin/authority/tenants/${encodeURIComponent(tenantId)}/instances`);
      if (!dialog.open) return;
      body.innerHTML = this.tenantInspector(tenant, instances, issued);
      refreshIcons(body);
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法读取租户实例');
    }
  }

  private tenantInspector(tenant: AuthorityTenant, instances: AuthorityInstance[], issued?: IssuedAuthorityCredential) {
    const credential = issued ? `
      <section class="authority-secret" role="status">
        <header><span><i data-lucide="key-round"></i></span><div><strong>部署凭据仅显示一次</strong><small>填入租户后端环境变量后关闭此窗口。</small></div></header>
        <label><span>Client ID</span><code>${escapeHtml(issued.instance.client_id)}</code><button type="button" class="icon-button" data-copy-value="${escapeHtml(issued.instance.client_id)}" title="复制 Client ID" aria-label="复制 Client ID"><i data-lucide="copy"></i></button></label>
        <label><span>Client Secret</span><code>${escapeHtml(issued.client_secret)}</code><button type="button" class="icon-button" data-copy-value="${escapeHtml(issued.client_secret)}" title="复制 Client Secret" aria-label="复制 Client Secret"><i data-lucide="copy"></i></button></label>
      </section>
    ` : '';
    return `
      ${credential}
      <section class="authority-tenant-summary">
        <header><div><span class="admin-status ${TENANT_STATUS[tenant.status][1]}"><i></i>${TENANT_STATUS[tenant.status][0]}</span><h3>${escapeHtml(tenant.name)}</h3><code>${escapeHtml(tenant.slug)}</code></div><button type="button" class="icon-button" data-edit-tenant="${tenant.id}" title="编辑租户" aria-label="编辑租户"><i data-lucide="pencil"></i></button></header>
        <dl><div><dt>授权到期</dt><dd>${formatDate(tenant.license_expires_at)}</dd></div><div><dt>宽限期</dt><dd>${formatDate(tenant.grace_ends_at)}</dd></div><div><dt>离线租约</dt><dd>${formatLease(tenant.offline_lease_minutes)}</dd></div></dl>
      </section>
      <section class="authority-instances">
        <header><div><h3>部署实例</h3><span>${instances.length} 个</span></div></header>
        <div class="authority-instance-list">${instances.map((instance) => `
          <article>
            <span class="authority-instance-icon"><i data-lucide="server"></i></span>
            <div><strong>${escapeHtml(instance.name)}</strong><code>${escapeHtml(instance.client_id)}</code><small>${instance.last_seen_at ? `最近校验 ${formatDate(instance.last_seen_at)}` : '尚未完成首次校验'}</small></div>
            <span class="admin-status ${instance.status === 'active' ? 'active' : 'archived'}"><i></i>${instance.status === 'active' ? '正常' : '已撤销'}</span>
            <span class="authority-instance-actions"><button type="button" class="icon-button" data-instance-action="rotate" data-instance-id="${instance.id}" data-tenant-id="${tenant.id}" title="轮换密钥" aria-label="轮换 ${escapeAttribute(instance.name)} 的密钥"><i data-lucide="refresh-cw"></i></button><button type="button" class="icon-button ${instance.status === 'active' ? 'danger' : ''}" data-instance-action="${instance.status === 'active' ? 'revoke' : 'restore'}" data-instance-id="${instance.id}" data-tenant-id="${tenant.id}" title="${instance.status === 'active' ? '撤销实例' : '恢复实例'}" aria-label="${instance.status === 'active' ? '撤销实例' : '恢复实例'}"><i data-lucide="${instance.status === 'active' ? 'ban' : 'check'}"></i></button></span>
          </article>`).join('') || '<div class="admin-empty compact"><i data-lucide="server-off"></i><strong>尚未签发部署实例</strong></div>'}</div>
        <form class="authority-instance-form" data-instance-form data-tenant-id="${tenant.id}"><label><span>新实例名称</span><input name="name" maxlength="120" required placeholder="例如：上海生产环境"></label><button class="button-primary" type="submit"><i data-lucide="key-round"></i><span>签发凭据</span></button></form>
      </section>
    `;
  }

  private async createInstance(form: HTMLFormElement) {
    if (!form.reportValidity()) return;
    const tenantId = form.dataset.tenantId;
    if (!tenantId) return;
    const button = form.querySelector<HTMLButtonElement>('button')!;
    button.disabled = true;
    try {
      const issued = await apiRequest<IssuedAuthorityCredential>(`/api/admin/authority/tenants/${encodeURIComponent(tenantId)}/instances`, {
        method: 'POST',
        body: JSON.stringify({ name: new FormData(form).get('name') }),
      });
      this.onMessage('部署凭据已签发，请立即配置到租户后端');
      await this.inspectTenant(tenantId, issued);
      void this.loadList();
    } catch (error) {
      button.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法签发部署凭据');
    }
  }

  private async handleInstanceAction(button: HTMLButtonElement) {
    const action = button.dataset.instanceAction;
    const instanceId = button.dataset.instanceId;
    const tenantId = button.dataset.tenantId;
    if (!action || !instanceId || !tenantId) return;
    if (action === 'rotate' && !window.confirm('轮换后旧密钥及现有访问令牌立即失效。确定继续吗？')) return;
    if (action === 'revoke' && !window.confirm('撤销后该部署将停止业务访问。确定继续吗？')) return;
    button.disabled = true;
    try {
      if (action === 'rotate') {
        const issued = await apiRequest<IssuedAuthorityCredential>(`/api/admin/authority/instances/${encodeURIComponent(instanceId)}/rotate-secret`, { method: 'POST' });
        this.onMessage('实例密钥已轮换');
        await this.inspectTenant(tenantId, issued);
      } else {
        await apiRequest(`/api/admin/authority/instances/${encodeURIComponent(instanceId)}`, {
          method: 'PATCH',
          body: JSON.stringify({ status: action === 'revoke' ? 'revoked' : 'active' }),
        });
        this.onMessage(action === 'revoke' ? '部署实例已撤销' : '部署实例已恢复');
        await this.inspectTenant(tenantId);
      }
      void this.loadList();
    } catch (error) {
      button.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法更新部署实例');
    }
  }

  private async copyCredential(value: string) {
    try {
      await navigator.clipboard.writeText(value);
      this.onMessage('已复制到剪贴板');
    } catch {
      this.onError('浏览器不允许访问剪贴板');
    }
  }

  private tenantPayload(tenant: AuthorityTenant, status = tenant.status) {
    return {
      name: tenant.name,
      status,
      license_expires_at: tenant.license_expires_at,
      grace_ends_at: tenant.grace_ends_at,
      warning_days: tenant.warning_days,
      offline_lease_minutes: tenant.offline_lease_minutes,
    };
  }

  private async inspectRoom(roomId: string) {
    if (!this.root) return;
    const dialog = this.root.querySelector<HTMLDialogElement>('.admin-inspector')!;
    const body = dialog.querySelector<HTMLElement>('.admin-inspector-body')!;
    this.setInspectorHeading('会议检查', 'STEALTH INSPECT', 'eye-off');
    body.innerHTML = '<div class="admin-loading"><i data-lucide="loader-circle"></i><span>正在读取会议</span></div>';
    refreshIcons(body);
    if (!dialog.open) dialog.showModal();
    try {
      const detail = await apiRequest<RoomDetail>(`/api/admin/rooms/${encodeURIComponent(roomId)}/inspect`);
      if (!this.root || !dialog.open) return;
      body.innerHTML = this.inspectorContent(detail);
      refreshIcons(body);
    } catch (error) {
      body.innerHTML = '<div class="admin-empty"><i data-lucide="triangle-alert"></i><strong>无法读取会议</strong></div>';
      refreshIcons(body);
      this.onError(error instanceof Error ? error.message : '无法读取会议');
    }
  }

  private setInspectorHeading(title: string, kicker: string, icon: string) {
    if (!this.root) return;
    const dialog = this.root.querySelector<HTMLDialogElement>('.admin-inspector')!;
    dialog.querySelector<HTMLElement>('h2')!.textContent = title;
    dialog.querySelector<HTMLElement>('.section-kicker')!.innerHTML = `<i data-lucide="${icon}"></i> ${kicker}`;
    refreshIcons(dialog.querySelector<HTMLElement>('.admin-inspector-heading')!);
  }

  private inspectorContent(detail: RoomDetail) {
    const recent = detail.utterances.slice(-30).reverse();
    return `
      <section class="inspector-summary">
        <div><span class="admin-status ${ROOM_STATUS[detail.room.status][1]}"><i></i>${ROOM_STATUS[detail.room.status][0]}</span><h3>${escapeHtml(detail.room.name)}</h3><small>${escapeHtml(detail.room.owner_username)} · ${formatDate(detail.room.created_at)}</small></div>
        <dl><div><dt>成员</dt><dd>${detail.room.member_count}</dd></div><div><dt>记录</dt><dd>${detail.room.utterance_count}</dd></div><div><dt>时长</dt><dd>${formatDuration(detail.room.duration_ms)}</dd></div></dl>
      </section>
      <section class="inspector-section">
        <header><h3>参会人员</h3><span>${detail.members.filter((member) => member.is_online).length} 人在线</span></header>
        <div class="inspector-members">${detail.members.map((member) => `<span><i class="${member.is_online ? 'online' : ''}"></i><strong>${escapeHtml(member.username)}</strong><small>${member.is_owner ? '房主' : member.is_muted ? '已禁言' : '成员'}</small></span>`).join('') || '<small>暂无参会人员</small>'}</div>
      </section>
      <section class="inspector-section">
        <header><h3>最近字幕</h3><span>${recent.length} 条</span></header>
        <div class="inspector-transcripts">${recent.map((item) => `<article><time>${formatDate(item.created_at)}</time><strong>${escapeHtml(item.source_text || '未识别')}</strong><p>${escapeHtml(item.translated_text || '暂无翻译')}</p>${item.speakers.length ? `<small>${item.speakers.map((speaker) => escapeHtml(speaker.username)).join('、')}</small>` : ''}</article>`).join('') || '<div class="admin-empty compact"><i data-lucide="messages-square"></i><strong>暂无字幕记录</strong></div>'}</div>
      </section>
    `;
  }
}

function paginationWindow(current: number, total: number) {
  const values = new Set([1, total, current - 1, current, current + 1]);
  const pages = [...values].filter((page) => page >= 1 && page <= total).sort((a, b) => a - b);
  const result: number[] = [];
  pages.forEach((page, index) => {
    if (index && page - pages[index - 1] > 1) result.push(0);
    result.push(page);
  });
  return result;
}

const HISTORY_ENTITIES = [
  ['users', '人员'],
  ['rooms', '会议'],
  ['room_members', '会议成员'],
  ['voice_sessions', '语音会话'],
  ['voice_utterances', '对话记录'],
  ['voice_utterance_speakers', '发言人'],
  ['voice_utterance_refinements', '识别精修'],
  ['voice_references', '参考音色'],
  ['system_installations', '系统初始化'],
  ['system_email_settings', '邮箱配置'],
  ['asr_system_settings', 'ASR 配置'],
  ['tts_system_settings', 'TTS 配置'],
  ['tts_voice_aliases', '音色别名'],
  ['authority_tenants', '授权租户'],
  ['authority_instances', '授权实例'],
] as const;

function historyEntityLabel(entityType: string) {
  return HISTORY_ENTITIES.find(([value]) => value === entityType)?.[1] ?? entityType;
}

function historyEntityOptions(selected: string) {
  return `<option value="">全部对象</option>${HISTORY_ENTITIES.map(([value, label]) => `<option value="${value}" ${selected === value ? 'selected' : ''}>${label}</option>`).join('')}`;
}

function changeFieldSummary(item: ChangeHistoryRecord) {
  const before = item.before_state ?? {};
  const after = item.after_state ?? {};
  const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
  const changed = [...keys].filter((key) => JSON.stringify(before[key]) !== JSON.stringify(after[key]));
  if (!changed.length) return '状态记录';
  const visible = changed.slice(0, 7).join('、');
  return changed.length > 7 ? `${visible} 等 ${changed.length} 项` : visible;
}

function formatDate(value: string) {
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value));
}

function formatDuration(durationMs: number) {
  const minutes = Math.max(0, Math.round(durationMs / 60_000));
  if (minutes < 1) return '少于 1 分钟';
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  return remainder ? `${hours}时${remainder}分` : `${hours} 小时`;
}

function formatLease(minutes: number) {
  if (minutes < 60) return `${minutes} 分钟`;
  if (minutes % 1440 === 0) return `${minutes / 1440} 天`;
  if (minutes % 60 === 0) return `${minutes / 60} 小时`;
  return `${Math.floor(minutes / 60)}时${minutes % 60}分`;
}

function toDateTimeInput(value: string) {
  const date = new Date(value);
  const offset = date.getTimezoneOffset() * 60_000;
  return new Date(date.getTime() - offset).toISOString().slice(0, 16);
}

function shortId(value: string) {
  return value.slice(0, 8);
}

function escapeHtml(value: string) {
  const element = document.createElement('div');
  element.textContent = value;
  return element.innerHTML;
}

function escapeAttribute(value: string) {
  return escapeHtml(value).replaceAll('"', '&quot;').replaceAll("'", '&#39;');
}
