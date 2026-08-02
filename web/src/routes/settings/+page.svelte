<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { currentUser } from '$lib/session';
  import { SettingsPage } from '../../pages/settings-page';

  let host: HTMLElement;

  onMount(() => {
    let unsubscribePage = () => {};
    const unsubscribe = currentUser.subscribe((user) => {
      if (!user || host.childElementCount) return;
      const settingsPage = new SettingsPage(user.id, () => void goto('/rooms'));
      void settingsPage.mount(host);
      unsubscribePage = () => void settingsPage.destroy();
    });
    return () => {
      unsubscribe();
      unsubscribePage();
    };
  });
</script>

<div bind:this={host}></div>
