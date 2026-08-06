<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { apiRequest, type User } from '../api';
  import { TopBar, type AppSection } from '../components/topbar';
  import { connectionStatus, resetSession } from './session';

  export let user: User;
  export let systemName = 'Voice Elf';
  export let organizationName = '';
  let host: HTMLElement;
  let topbar: TopBar | null = null;
  let activeSection: AppSection = 'home';

  $: activeSection = $page.url.pathname === '/admin'
    ? 'admin'
    : $page.url.pathname === '/me' || $page.url.pathname === '/settings'
      ? 'profile'
      : 'home';
  $: topbar?.setActiveSection(activeSection);

  async function logout() {
    try {
      await apiRequest('/api/auth/logout', { method: 'DELETE' });
    } finally {
      resetSession(null);
      await goto('/login', { replaceState: true });
    }
  }

  onMount(() => {
    topbar = new TopBar(
      user,
      systemName,
      organizationName,
      () => void goto('/rooms'),
      () => void goto('/me'),
      () => void goto('/admin'),
      () => void logout(),
    );
    topbar.setActiveSection(activeSection);
    host.append(topbar.element);
    const unsubscribe = connectionStatus.subscribe((status) => topbar?.setConnection(status));
    return () => {
      unsubscribe();
      topbar?.element.remove();
      topbar = null;
    };
  });
</script>

<div bind:this={host}></div>
