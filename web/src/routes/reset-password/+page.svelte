<script lang="ts">
  import { goto } from '$app/navigation';
  import { afterUpdate, onMount } from 'svelte';
  import { apiRequest } from '../../api';
  import { refreshIcons } from '../../components/icons';
  import { systemSetup } from '$lib/session';

  let host: HTMLElement;
  let confirmationInput: HTMLInputElement;
  let token = '';
  let password = '';
  let confirmation = '';
  let submitting = false;
  let completed = false;
  let error = '';

  onMount(() => {
    token = new URL(window.location.href).searchParams.get('token') ?? '';
    if (!/^[a-f0-9]{64}$/i.test(token)) error = '密码重置链接无效或已过期';
  });

  afterUpdate(() => {
    if (host) refreshIcons(host);
  });

  function syncConfirmation() {
    confirmationInput?.setCustomValidity(
      confirmation && confirmation !== password ? '两次输入的密码不一致' : '',
    );
  }

  async function submit(event: SubmitEvent) {
    const form = event.currentTarget as HTMLFormElement;
    if (!/^[a-f0-9]{64}$/i.test(token) || !form.reportValidity() || password !== confirmation) return;
    submitting = true;
    error = '';
    try {
      await apiRequest<void>('/api/auth/password/reset', {
        method: 'POST',
        body: JSON.stringify({ token, password }),
      });
      completed = true;
      password = '';
      confirmation = '';
    } catch (reason) {
      error = reason instanceof Error ? reason.message : '无法重置密码';
    } finally {
      submitting = false;
    }
  }
</script>

<svelte:head>
  <title>重置密码 · {$systemSetup?.profile?.system_name ?? 'Voice Elf'}</title>
</svelte:head>

<main class="auth-page" bind:this={host}>
  <section class="auth-panel password-reset-panel">
    <div class="auth-brand"><span class="brand-mark"><i data-lucide="audio-lines"></i></span><strong>{$systemSetup?.profile?.system_name ?? 'Voice Elf'}</strong></div>
    {#if completed}
      <div class="password-reset-result" role="status">
        <span><i data-lucide="badge-check"></i></span>
        <h1>密码已更新</h1>
        <p>所有旧登录会话均已退出，请使用新密码重新登录。</p>
        <button class="primary-command" type="button" on:click={() => goto('/login', { replaceState: true })}>返回登录</button>
      </div>
    {:else}
      <form on:submit|preventDefault={submit}>
        <header class="password-reset-heading">
          <span><i data-lucide="key-round"></i></span>
          <div><h1>设置新密码</h1><p>完成后，其他设备上的登录状态将立即失效。</p></div>
        </header>
        <label><span>新密码</span><input bind:value={password} on:input={syncConfirmation} type="password" minlength="8" maxlength="128" required autocomplete="new-password"></label>
        <label><span>确认新密码</span><input bind:this={confirmationInput} bind:value={confirmation} on:input={syncConfirmation} type="password" minlength="8" maxlength="128" required autocomplete="new-password"></label>
        {#if error}<p class="form-error" role="alert">{error}</p>{/if}
        <button class="primary-command" type="submit" disabled={submitting || !/^[a-f0-9]{64}$/i.test(token)}>{submitting ? '正在更新' : '更新密码'}</button>
        <button class="auth-back" type="button" on:click={() => goto('/login')}><i data-lucide="arrow-left"></i><span>返回登录</span></button>
      </form>
    {/if}
  </section>
</main>
