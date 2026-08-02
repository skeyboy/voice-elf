<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { AuthPage } from '../../pages/auth-page';
  import { resetSession } from '$lib/session';

  let host: HTMLElement;

  onMount(() => {
    const authPage = new AuthPage((user) => {
      resetSession(user);
      void goto('/rooms', { replaceState: true });
    });
    void authPage.mount(host);
    return () => void authPage.destroy();
  });
</script>

<div bind:this={host}></div>
