<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import TopBarHost from '$lib/TopBarHost.svelte';
  import ToastRegion from '$lib/ToastRegion.svelte';
  import { currentUser, loadSession } from '$lib/session';
  import '../styles/index.css';

  onMount(() => {
    void loadSession();
  });

  $: if ($currentUser !== undefined) {
    const loginRoute = $page.url.pathname === '/login';
    if (!$currentUser && !loginRoute) void goto('/login', { replaceState: true });
    if ($currentUser && loginRoute) void goto('/rooms', { replaceState: true });
  }
</script>

{#if $currentUser && $page.url.pathname !== '/login'}
  <TopBarHost user={$currentUser} />
{/if}

<slot />
<ToastRegion />
