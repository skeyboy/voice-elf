<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { currentUser, showError, showToast } from '$lib/session';
  import { LexiconPage } from '../../../pages/lexicon-page';
  let host: HTMLElement;
  onMount(() => {
    let destroy = () => {};
    const unsubscribe = currentUser.subscribe((user) => {
      if (!user || host.childElementCount) return;
      if (user.role !== 'admin') { void goto('/rooms', { replaceState: true }); return; }
      const page = new LexiconPage(showError, (message) => showToast(message)); void page.mount(host); destroy = () => page.destroy();
    });
    return () => { unsubscribe(); destroy(); };
  });
</script>
<div bind:this={host}></div>
