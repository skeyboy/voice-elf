<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { RoomsPage } from '../../pages/rooms-page';
  import { showError } from '$lib/session';

  let host: HTMLElement;

  onMount(() => {
    const roomsPage = new RoomsPage(
      (roomId) => void goto(`/rooms/${roomId}`),
      showError,
    );
    void roomsPage.mount(host);
    return () => void roomsPage.destroy();
  });
</script>

<div bind:this={host}></div>
