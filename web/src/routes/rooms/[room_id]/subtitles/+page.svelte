<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { currentUser, showError } from '$lib/session';
  import { SubtitlePage } from '../../../../pages/subtitle-page';

  let host: HTMLElement;

  onMount(() => {
    let destroyPage = () => {};
    const unsubscribe = currentUser.subscribe((user) => {
      if (!user || host.childElementCount) return;
      const subtitlePage = new SubtitlePage(
        user.id,
        $page.params.room_id!,
        () => void goto(`/rooms/${$page.params.room_id}`),
        showError,
      );
      void subtitlePage.mount(host);
      destroyPage = () => void subtitlePage.destroy();
    });
    return () => {
      unsubscribe();
      destroyPage();
    };
  });
</script>

<div bind:this={host}></div>
