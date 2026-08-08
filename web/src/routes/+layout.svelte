<script lang="ts">
  import { afterNavigate, beforeNavigate, goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import TopBarHost from '$lib/TopBarHost.svelte';
  import ToastRegion from '$lib/ToastRegion.svelte';
  import { scheduleVadPreload } from '../audio';
  import {
    currentUser,
    instanceAuthorization,
    loadAuthorization,
    loadSetup,
    loadSession,
    showError,
    showToast,
    systemSetup,
    type ToastKind,
  } from '$lib/session';
  import '../styles/index.css';

  let routeNavigating = false;
  let routeDestination = '';
  let navigationTimer = 0;
  const routeScrollPositions = new Map<string, number>();

  beforeNavigate(({ from, to }) => {
    if (!to?.url || from?.url.pathname === to.url.pathname) return;
    if (from?.url) routeScrollPositions.set(from.url.pathname, window.scrollY);
    window.clearTimeout(navigationTimer);
    routeDestination = routeLabel(to.url.pathname);
    routeNavigating = true;
  });

  afterNavigate(({ to }) => {
    if (to?.url) {
      const scrollTop = routeScrollPositions.get(to.url.pathname);
      if (scrollTop !== undefined) requestAnimationFrame(() => window.scrollTo({ top: scrollTop }));
    }
    navigationTimer = window.setTimeout(() => {
      routeNavigating = false;
    }, 180);
  });

  function routeLabel(pathname: string) {
    if (pathname === '/rooms') return '会议目录';
    if (pathname.startsWith('/rooms/') && pathname.endsWith('/subtitles')) return '字幕大屏';
    if (pathname.startsWith('/rooms/')) return '实时会话';
    if (pathname === '/admin') return '系统管理';
    if (pathname === '/me' || pathname === '/settings') return '个人设置';
    if (pathname === '/login') return '登录';
    return '页面';
  }

  const refreshAuthorization = async (force = false) => {
    const authorization = await loadAuthorization(force);
    if (authorization.allowed) void loadSession();
    return authorization;
  };

  onMount(() => {
    document.getElementById('voice-elf-boot')?.remove();
    const cancelVadPreload = scheduleVadPreload();
    void loadSetup().then((setup) => {
      if (setup.initialized) void refreshAuthorization();
    });
    const authorizationTimer = window.setInterval(() => {
      void loadSetup(true).then((setup) => {
        if (setup.initialized) void refreshAuthorization(true);
      });
    }, 60_000);
    let quitPromptTimer = 0;
    let quitPromptOpen = false;

    const resetQuitRequest = () =>
      fetch('/__voice_elf/app/quit/cancel', { method: 'POST' }).catch(() => null);
    const handleQuitRequest = () => {
      if (quitPromptOpen) return;
      quitPromptOpen = true;
      showToast('已拦截退出操作，请确认是否完全退出', 2600, 'warning');
      quitPromptTimer = window.setTimeout(async () => {
        const confirmed = window.confirm(
          '确定退出 Voice Elf 吗？\n\n退出后，实时字幕和后台运行都会停止。',
        );
        if (!confirmed) {
          await resetQuitRequest();
          quitPromptOpen = false;
          showToast('已取消退出，Voice Elf 将继续在后台运行');
          return;
        }
        const response = await fetch('/__voice_elf/app/quit', { method: 'POST' }).catch(() => null);
        if (!response?.ok) {
          await resetQuitRequest();
          quitPromptOpen = false;
          showError('无法退出应用，请从状态栏菜单重试');
        }
      }, 160);
    };
    const handleNativeToast = (event: Event) => {
      const detail = (event as CustomEvent<{ message?: unknown; kind?: unknown }>).detail;
      if (typeof detail?.message !== 'string') return;
      const kind: ToastKind =
        detail.kind === 'warning' || detail.kind === 'error' ? detail.kind : 'info';
      showToast(detail.message, kind === 'error' ? 4200 : 2400, kind);
    };

    window.addEventListener('voice-elf:native-quit-requested', handleQuitRequest);
    window.addEventListener('voice-elf:native-toast', handleNativeToast);
    return () => {
      cancelVadPreload();
      window.clearTimeout(quitPromptTimer);
      window.clearTimeout(navigationTimer);
      window.clearInterval(authorizationTimer);
      window.removeEventListener('voice-elf:native-quit-requested', handleQuitRequest);
      window.removeEventListener('voice-elf:native-toast', handleNativeToast);
    };
  });

  $: if ($systemSetup) {
    const setupRoute = $page.url.pathname === '/setup';
    if (!$systemSetup.initialized && !setupRoute) void goto('/setup', { replaceState: true });
    if ($systemSetup.initialized && setupRoute) {
      void goto($currentUser ? '/rooms' : '/login', { replaceState: true });
    }
    if ($systemSetup.profile) document.title = $systemSetup.profile.system_name;
  }

  $: if ($systemSetup?.initialized && $instanceAuthorization?.allowed && $currentUser !== undefined) {
    const loginRoute = $page.url.pathname === '/login';
    const publicAuthRoute = loginRoute || $page.url.pathname === '/reset-password';
    if (!$currentUser && !publicAuthRoute) void goto('/login', { replaceState: true });
    if ($currentUser && loginRoute) void goto('/rooms', { replaceState: true });
  }
</script>

{#if !$systemSetup}
  <main class="license-state-page">
    <section class="license-state-panel license-state-loading" aria-live="polite">
      <span class="license-state-icon" aria-hidden="true"><i></i></span>
      <h1>正在检查系统状态</h1>
      <small>连接本地服务并读取部署信息</small>
      <span class="initialization-progress" aria-hidden="true"><i></i></span>
    </section>
  </main>
{:else if !$systemSetup.initialized}
  {#if $page.url.pathname === '/setup'}
    <slot />
  {:else}
    <main class="license-state-page">
      <section class="license-state-panel license-state-loading" aria-live="polite">
        <span class="license-state-icon" aria-hidden="true"><i></i></span>
        <h1>正在进入初始化向导</h1>
        <small>准备首次运行配置</small>
        <span class="initialization-progress" aria-hidden="true"><i></i></span>
      </section>
    </main>
  {/if}
{:else if !$instanceAuthorization}
  <main class="license-state-page">
    <section class="license-state-panel license-state-loading" aria-live="polite">
      <span class="license-state-icon" aria-hidden="true"><i></i></span>
      <h1>正在验证实例</h1>
      <small>确认授权与服务可用状态</small>
      <span class="initialization-progress" aria-hidden="true"><i></i></span>
    </section>
  </main>
{:else if !$instanceAuthorization.allowed}
  <main class="license-state-page">
    <section class="license-state-panel" aria-live="polite">
      <span class="license-state-icon" aria-hidden="true">!</span>
      <p>{$instanceAuthorization.tenant_name ?? 'VOICE ELF INSTANCE'}</p>
      <h1>实例授权不可用</h1>
      <strong>{$instanceAuthorization.message}</strong>
      {#if $instanceAuthorization.last_checked_at}
        <small>最后校验 {new Date($instanceAuthorization.last_checked_at).toLocaleString('zh-CN')}</small>
      {/if}
      <button type="button" on:click={() => refreshAuthorization(true)}>重新校验</button>
    </section>
  </main>
{:else}
  {#if $instanceAuthorization && ['warning', 'grace'].includes($instanceAuthorization.status)}
    <aside class="license-warning" role="status">
      <strong>{$instanceAuthorization.message}</strong>
      {#if $instanceAuthorization.license_expires_at}
        <span>到期时间 {new Date($instanceAuthorization.license_expires_at).toLocaleString('zh-CN')}</span>
      {/if}
    </aside>
  {/if}
  {#if $currentUser && $page.url.pathname !== '/login' && !$page.url.pathname.endsWith('/subtitles')}
    <TopBarHost
      user={$currentUser}
      systemName={$systemSetup.profile?.system_name ?? 'Voice Elf'}
      organizationName={$systemSetup.profile?.organization_name ?? ''}
    />
  {/if}
  <slot />
{/if}
{#if routeNavigating}
  <div class="route-transition-status" role="status" aria-live="polite">
    <span aria-hidden="true"></span>
    <strong>正在打开{routeDestination}</strong>
  </div>
{/if}
<ToastRegion />
