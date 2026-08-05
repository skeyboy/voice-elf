<script lang="ts">
  import { toastMessages } from './session';
</script>

<div class="toast-region" aria-live="polite">
  {#each $toastMessages as toast (toast.id)}
    <div class={`toast ${toast.kind}`}>{toast.message}</div>
  {/each}
</div>
