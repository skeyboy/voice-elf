export interface AppConfig {
  api_url: string;
}

const configEndpoint = '/__voice_elf/config';

export async function loadAppConfig(): Promise<AppConfig | null> {
  try {
    const response = await fetch(configEndpoint, {
      cache: 'no-store',
      headers: { accept: 'application/json' },
    });
    if (!response.ok || !response.headers.get('content-type')?.includes('application/json')) {
      return null;
    }
    const config = (await response.json()) as Partial<AppConfig>;
    return typeof config.api_url === 'string' ? { api_url: config.api_url } : null;
  } catch {
    return null;
  }
}

export async function saveAppConfig(apiUrl: string): Promise<AppConfig> {
  const response = await fetch(configEndpoint, {
    method: 'PUT',
    headers: {
      accept: 'application/json',
      'content-type': 'application/json',
    },
    body: JSON.stringify({ api_url: apiUrl }),
  });
  const payload = (await response.json().catch(() => null)) as
    | (Partial<AppConfig> & { error?: string })
    | null;
  if (!response.ok) {
    throw new Error(payload?.error ?? `保存失败 (${response.status})`);
  }
  if (typeof payload?.api_url !== 'string') {
    throw new Error('应用返回了无效的 API 地址');
  }
  return { api_url: payload.api_url };
}
