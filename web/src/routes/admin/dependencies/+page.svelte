<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { currentUser, showError } from '$lib/session';
  import { DependenciesPage } from '../../../pages/dependencies-page';

  let host: HTMLElement;

  onMount(() => {
    let destroyPage = () => {};
    const unsubscribe = currentUser.subscribe((user) => {
      if (!user || host.childElementCount) return;
      if (user.role !== 'admin') {
        void goto('/rooms', { replaceState: true });
        return;
      }
      const page = new DependenciesPage(showError);
      void page.mount(host);
      destroyPage = () => void page.destroy();
    });
    return () => {
      unsubscribe();
      destroyPage();
    };
  });
</script>

<div bind:this={host}></div>
