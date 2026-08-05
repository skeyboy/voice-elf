<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import TopBarHost from '$lib/TopBarHost.svelte';
  import ToastRegion from '$lib/ToastRegion.svelte';
  import { currentUser, loadSession, showError, showToast, type ToastKind } from '$lib/session';
  import '../styles/index.css';

  onMount(() => {
    void loadSession();
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
      window.clearTimeout(quitPromptTimer);
      window.removeEventListener('voice-elf:native-quit-requested', handleQuitRequest);
      window.removeEventListener('voice-elf:native-toast', handleNativeToast);
    };
  });

  $: if ($currentUser !== undefined) {
    const loginRoute = $page.url.pathname === '/login';
    if (!$currentUser && !loginRoute) void goto('/login', { replaceState: true });
    if ($currentUser && loginRoute) void goto('/rooms', { replaceState: true });
  }
</script>

{#if $currentUser && $page.url.pathname !== '/login' && !$page.url.pathname.endsWith('/subtitles')}
  <TopBarHost user={$currentUser} />
{/if}

<slot />
<ToastRegion />
