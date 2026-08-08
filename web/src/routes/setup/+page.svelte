<script lang="ts">
  import { goto } from '$app/navigation';
  import { afterUpdate, onMount } from 'svelte';
  import {
    apiRequest,
    type InitializeSystemResponse,
    type SetupStatus,
  } from '../../api';
  import { refreshIcons } from '../../components/icons';
  import { loadAppConfig, saveAppConfig } from '../../app-config';
  import {
    loadAuthorization,
    loadSetup,
    resetSession,
    setupConnectionError,
    systemSetup,
  } from '$lib/session';

  const steps = ['环境检查', '系统资料', '管理员', '确认启动'];
  let host: HTMLElement;
  let form: HTMLFormElement;
  let confirmationInput: HTMLInputElement;
  let step = 0;
  let systemName = 'Voice Elf';
  let organizationName = '';
  let publicUrl = '';
  let adminUsername = '';
  let adminEmail = '';
  let setupToken = '';
  let adminPassword = '';
  let passwordConfirmation = '';
  let error = '';
  let submitting = false;
  let appConfigAvailable = false;
  let appApiUrl = '';
  let appConfigStatus = '';
  let savingAppConfig = false;

  onMount(() => {
    publicUrl = window.location.origin;
    if (!$systemSetup) void loadSetup();
    void loadAppConfig().then((config) => {
      if (!config) return;
      appConfigAvailable = true;
      appApiUrl = config.api_url;
    });
  });

  afterUpdate(() => {
    if (host) refreshIcons(host);
  });

  function next() {
    error = '';
    if (step > 0 && !form.reportValidity()) return;
    if (step === 2 && adminPassword !== passwordConfirmation) {
      error = '两次输入的密码不一致';
      confirmationInput?.setCustomValidity(error);
      confirmationInput?.reportValidity();
      return;
    }
    step = Math.min(3, step + 1);
  }

  function back() {
    error = '';
    step = Math.max(0, step - 1);
  }

  function syncPasswordConfirmation() {
    confirmationInput?.setCustomValidity(
      passwordConfirmation && passwordConfirmation !== adminPassword ? '两次输入的密码不一致' : '',
    );
  }

  async function initializeSystem() {
    if (!form.reportValidity() || adminPassword !== passwordConfirmation) return;
    error = '';
    submitting = true;
    try {
      const result = await apiRequest<InitializeSystemResponse>('/api/setup/initialize', {
        method: 'POST',
        body: JSON.stringify({
          system_name: systemName,
          setup_token: setupToken,
          organization_name: organizationName,
          public_url: publicUrl,
          admin_username: adminUsername,
          admin_email: adminEmail,
          admin_password: adminPassword,
        }),
      });
      resetSession(result.user);
      await loadSetup(true);
      await loadAuthorization(true);
      await goto('/rooms', { replaceState: true });
    } catch (reason) {
      error = reason instanceof Error ? reason.message : '无法完成系统初始化';
    } finally {
      submitting = false;
    }
  }

  function modeLabel(mode: SetupStatus['deployment_mode']) {
    return mode === 'bus' ? '授权总线' : mode === 'tenant' ? '租户自建服务' : '独立运行';
  }

  async function saveServerAndRetry() {
    appConfigStatus = '';
    const apiUrl = appApiUrl.trim();
    if (!apiUrl) {
      appConfigStatus = '请输入 API 地址';
      return;
    }
    savingAppConfig = true;
    try {
      const saved = await saveAppConfig(apiUrl);
      appApiUrl = saved.api_url;
      appConfigStatus = '地址已保存，正在重新连接';
      const setup = await loadSetup(true);
      if (setup.initialized) await loadAuthorization(true);
      if ($setupConnectionError) appConfigStatus = $setupConnectionError;
    } catch (reason) {
      appConfigStatus = reason instanceof Error ? reason.message : '无法保存 API 地址';
    } finally {
      savingAppConfig = false;
    }
  }
</script>

<svelte:head>
  <title>初始化 Voice Elf</title>
</svelte:head>

<main class="setup-page" bind:this={host}>
  <section class="setup-shell">
    <aside class="setup-rail">
      <div class="setup-brand">
        <span class="brand-mark"><i data-lucide="audio-lines"></i></span>
        <span><strong>Voice Elf</strong><small>INITIAL SETUP</small></span>
      </div>
      <ol aria-label="初始化进度">
        {#each steps as item, index}
          <li class:active={index === step} class:complete={index < step} aria-current={index === step ? 'step' : undefined}>
            <span>{index < step ? '✓' : index + 1}</span>
            <strong>{item}</strong>
          </li>
        {/each}
      </ol>
      <small class="setup-security-note">部署密钥只由后端环境读取，不会进入浏览器或初始化记录。</small>
    </aside>

    <form class="setup-workspace" bind:this={form} on:submit|preventDefault={initializeSystem}>
      {#if !$systemSetup}
        <div class="setup-loading" role="status"><i data-lucide="loader-circle"></i><span>正在读取部署状态</span></div>
      {:else}
        <header class="setup-heading">
          <span>步骤 {step + 1} / {steps.length}</span>
          <h1>{steps[step]}</h1>
        </header>

        <div class="setup-content">
          {#if step === 0}
            <section class="setup-environment" aria-label="部署环境">
              <p>{$setupConnectionError ? '当前无法连接 Voice Elf 服务，请检查服务地址后重试。' : '确认服务端运行条件后，继续创建系统资料和首个管理员。'}</p>
              {#if $setupConnectionError && appConfigAvailable}
                <div class="setup-server-recovery" role="alert">
                  <div>
                    <i data-lucide="server-off"></i>
                    <span><strong>服务连接失败</strong><small>{$setupConnectionError}</small></span>
                  </div>
                  <label>
                    <span>API 地址</span>
                    <input bind:value={appApiUrl} type="url" inputmode="url" autocomplete="url" autocapitalize="none" spellcheck="false" placeholder="http://192.168.1.4:3001">
                  </label>
                  <button type="button" class="button-primary" disabled={savingAppConfig} on:click={saveServerAndRetry}>
                    <i data-lucide={savingAppConfig ? 'loader-circle' : 'refresh-cw'}></i>
                    <span>{savingAppConfig ? '正在连接' : '保存并重试'}</span>
                  </button>
                  {#if appConfigStatus}<p class:error={$setupConnectionError}>{appConfigStatus}</p>{/if}
                </div>
              {/if}
              <dl>
                <div class:ready={$systemSetup.database_ready}>
                  <dt><i data-lucide="database"></i><span>PostgreSQL</span></dt>
                  <dd>{$systemSetup.database_ready ? '已连接' : '未配置或无法连接'}</dd>
                </div>
                <div class:ready={$systemSetup.authorization.allowed}>
                  <dt><i data-lucide="shield-check"></i><span>实例授权</span></dt>
                  <dd>{$systemSetup.authorization.allowed ? '校验通过' : $systemSetup.authorization.message}</dd>
                </div>
                <div class="ready">
                  <dt><i data-lucide="network"></i><span>部署模式</span></dt>
                  <dd>{modeLabel($systemSetup.deployment_mode)}</dd>
                </div>
                <div class="ready">
                  <dt><i data-lucide="cpu"></i><span>语音后端</span></dt>
                  <dd>{$systemSetup.backend}</dd>
                </div>
                <div class:ready={$systemSetup.email_ready}>
                  <dt><i data-lucide="mail-check"></i><span>密码找回邮件</span></dt>
                  <dd>{$systemSetup.email_ready ? 'SMTP 已配置' : '尚未配置，可稍后启用'}</dd>
                </div>
              </dl>
              {#if !$systemSetup.initialization_allowed && !$setupConnectionError}
                <div class="setup-blocker" role="alert">
                  <i data-lucide="triangle-alert"></i>
                  <span>请先在服务端完成数据库与授权配置，然后重新检查。</span>
                  <button type="button" class="button-secondary" on:click={() => loadSetup(true)}><i data-lucide="refresh-cw"></i><span>重新检查</span></button>
                </div>
              {/if}
            </section>
          {:else if step === 1}
            <section class="setup-fields">
              <label>
                <span>系统名称</span>
                <input bind:value={systemName} name="system_name" maxlength="64" required autocomplete="organization-title">
              </label>
              <label>
                <span>组织名称</span>
                <input bind:value={organizationName} name="organization_name" maxlength="120" required autocomplete="organization" placeholder="例如：上海声语科技">
              </label>
              <label class="setup-wide-field">
                <span>系统访问地址</span>
                <span class="setup-input-icon"><i data-lucide="globe-2"></i><input bind:value={publicUrl} name="public_url" type="url" required inputmode="url" autocomplete="url" placeholder="https://voice.example.com"></span>
              </label>
            </section>
          {:else if step === 2}
            <section class="setup-fields setup-admin-fields">
              <label class="setup-wide-field">
                <span>初始化口令</span>
                <input bind:value={setupToken} name="setup_token" type="password" minlength="16" required autocomplete="one-time-code" placeholder="查看服务端启动日志或部署环境配置">
              </label>
              <label class="setup-wide-field">
                <span>管理员账号</span>
                <input bind:value={adminUsername} name="admin_username" minlength="3" maxlength="32" pattern="[A-Za-z0-9_-]+" required autocomplete="username">
              </label>
              <label class="setup-wide-field">
                <span>管理员邮箱</span>
                <input bind:value={adminEmail} name="admin_email" type="email" maxlength="254" required autocomplete="email" placeholder="用于找回密码">
              </label>
              <label>
                <span>管理员密码</span>
                <input bind:value={adminPassword} on:input={syncPasswordConfirmation} name="admin_password" type="password" minlength="8" maxlength="128" required autocomplete="new-password">
              </label>
              <label>
                <span>确认密码</span>
                <input bind:this={confirmationInput} bind:value={passwordConfirmation} on:input={syncPasswordConfirmation} name="password_confirmation" type="password" minlength="8" maxlength="128" required autocomplete="new-password">
              </label>
            </section>
          {:else}
            <section class="setup-review">
              <div class="setup-review-primary">
                <span class="brand-mark"><i data-lucide="audio-lines"></i></span>
                <div><strong>{systemName}</strong><small>{organizationName}</small></div>
              </div>
              <dl>
                <div><dt>部署模式</dt><dd>{modeLabel($systemSetup.deployment_mode)}</dd></div>
                <div><dt>访问地址</dt><dd>{publicUrl}</dd></div>
                <div><dt>首个管理员</dt><dd>{adminUsername}</dd></div>
                <div><dt>管理员邮箱</dt><dd>{adminEmail}</dd></div>
                <div><dt>账号策略</dt><dd>后续注册由管理员验证</dd></div>
              </dl>
              <p><i data-lucide="info"></i><span>完成后系统立即启用，并以首个管理员身份登录。</span></p>
            </section>
          {/if}
        </div>

        {#if error}<p class="setup-error" role="alert">{error}</p>{/if}

        <footer class="setup-actions">
          {#if step > 0}
            <button class="button-secondary" type="button" on:click={back} disabled={submitting}><i data-lucide="arrow-left"></i><span>上一步</span></button>
          {:else}
            <span></span>
          {/if}
          {#if step < 3}
            <button class="button-primary" type="button" on:click={next} disabled={step === 0 && !$systemSetup.initialization_allowed}><span>继续</span><i data-lucide="arrow-right"></i></button>
          {:else}
            <button class="button-primary" type="submit" disabled={submitting}><i data-lucide={submitting ? 'loader-circle' : 'rocket'}></i><span>{submitting ? '正在初始化' : '启动系统'}</span></button>
          {/if}
        </footer>
      {/if}
    </form>
  </section>
</main>
