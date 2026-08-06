<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';

  onMount(() => {
    void goto('/me', { replaceState: true });
  });
</script>

<div></div>
