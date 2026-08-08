<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';

  onMount(() => {
    void goto('/rooms', { replaceState: true });
  });
</script>

<main class="page-loading-state" aria-busy="true" aria-live="polite">
  <div class="page-loading-mark" aria-hidden="true"></div>
  <div class="page-loading-copy"><strong>正在进入会议目录</strong><span>恢复上次浏览位置</span></div>
  <span class="page-loading-progress" aria-hidden="true"><i></i></span>
</main>
