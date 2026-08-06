<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { RoomsPage } from '../../pages/rooms-page';
  import { currentUser, showError } from '$lib/session';

  let host: HTMLElement;

  onMount(() => {
    let destroyPage = () => {};
    const unsubscribe = currentUser.subscribe((user) => {
      if (!user || host.childElementCount) return;
      const roomsPage = new RoomsPage(
        user,
        (roomId) => void goto(`/rooms/${roomId}`),
        showError,
      );
      void roomsPage.mount(host);
      destroyPage = () => void roomsPage.destroy();
    });
    return () => {
      unsubscribe();
      destroyPage();
    };
  });
</script>

<div bind:this={host}></div>
