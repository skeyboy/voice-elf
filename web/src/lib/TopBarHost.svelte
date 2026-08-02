<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { apiRequest, type User } from '../api';
  import { TopBar } from '../components/topbar';
  import { connectionStatus, resetSession } from './session';

  export let user: User;
  let host: HTMLElement;

  async function logout() {
    try {
      await apiRequest('/api/auth/logout', { method: 'DELETE' });
    } finally {
      resetSession(null);
      await goto('/login', { replaceState: true });
    }
  }

  onMount(() => {
    const topbar = new TopBar(
      user,
      () => void goto('/rooms'),
      () => void goto('/settings'),
      () => void logout(),
    );
    host.append(topbar.element);
    const unsubscribe = connectionStatus.subscribe((status) => topbar.setConnection(status));
    return () => {
      unsubscribe();
      topbar.element.remove();
    };
  });
</script>

<div bind:this={host}></div>
