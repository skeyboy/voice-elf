<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';

  onMount(() => {
    void goto('/me', { replaceState: true });
  });
</script>

<main class="page-loading-state" aria-busy="true" aria-live="polite">
  <div class="page-loading-copy"><strong>正在打开个人设置</strong><span>加载已保存的偏好</span></div>
  <span class="page-loading-progress" aria-hidden="true"><i></i></span>
</main>
