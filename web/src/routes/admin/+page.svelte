<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { AdminPage } from '../../pages/admin-page';
  import { currentUser, showError, showToast } from '$lib/session';

  let host: HTMLElement;

  onMount(() => {
    let destroyPage = () => {};
    const unsubscribe = currentUser.subscribe((user) => {
      if (!user || host.childElementCount) return;
      if (user.role !== 'admin') {
        void goto('/rooms', { replaceState: true });
        return;
      }
      const adminPage = new AdminPage(user, showError, (message) => showToast(message));
      void adminPage.mount(host);
      destroyPage = () => void adminPage.destroy();
    });
    return () => {
      unsubscribe();
      destroyPage();
    };
  });
</script>

<div bind:this={host}></div>
