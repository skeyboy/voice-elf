<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { TranslatorPage } from '../../../pages/translator-page';
  import { connectionStatus, currentUser, showError } from '$lib/session';

  let host: HTMLElement;

  onMount(() => {
    let destroyPage = () => {};
    const unsubscribe = currentUser.subscribe((user) => {
      if (!user || host.childElementCount) return;
      const translatorPage = new TranslatorPage(
        user.id,
        $page.params.room_id!,
        () => void goto('/rooms'),
        () => void goto('/rooms', { replaceState: true }),
        (status) => connectionStatus.set(status),
        showError,
      );
      void translatorPage.mount(host);
      destroyPage = () => void translatorPage.destroy();
    });
    return () => {
      unsubscribe();
      destroyPage();
      connectionStatus.set('hidden');
    };
  });
</script>

<div bind:this={host}></div>
