import { writable } from 'svelte/store';
import {
  ApiRequestError,
  apiRequest,
  type InstanceAuthorization,
  type SetupStatus,
  type User,
} from '../api';
import type { ConnectionStatus } from '../components/topbar';

export const currentUser = writable<User | null | undefined>(undefined);
export const instanceAuthorization = writable<InstanceAuthorization | undefined>(undefined);
export const systemSetup = writable<SetupStatus | undefined>(undefined);
export const connectionStatus = writable<ConnectionStatus>('hidden');
export type ToastKind = 'info' | 'warning' | 'error';
export const toastMessages = writable<Array<{ id: number; message: string; kind: ToastKind }>>([]);

let sessionRequest: Promise<User | null> | null = null;
let authorizationRequest: Promise<InstanceAuthorization> | null = null;
let setupRequest: Promise<SetupStatus> | null = null;
let nextToastId = 1;

export function loadSession() {
  if (!sessionRequest) {
    sessionRequest = apiRequest<User>('/api/auth/me')
      .catch((error) => {
        if (error instanceof ApiRequestError && error.status === 401) return null;
        showError(error instanceof Error ? error.message : '无法连接账号服务');
        return null;
      })
      .then((user) => {
        currentUser.set(user);
        return user;
      });
  }
  return sessionRequest;
}

export function loadAuthorization(force = false) {
  if (force) authorizationRequest = null;
  if (!authorizationRequest) {
    authorizationRequest = apiRequest<InstanceAuthorization>('/api/instance/authorization')
      .catch(() => ({
        mode: 'tenant' as const,
        allowed: false,
        status: 'blocked' as const,
        message: '无法连接本地授权服务',
        tenant_id: null,
        tenant_name: null,
        instance_id: null,
        instance_name: null,
        asr_backend_id: null,
        asr_config_source: null,
        license_expires_at: null,
        grace_ends_at: null,
        lease_expires_at: null,
        last_checked_at: null,
        next_check_at: null,
      }))
      .then((authorization) => {
        instanceAuthorization.set(authorization);
        return authorization;
      });
  }
  return authorizationRequest;
}

export function loadSetup(force = false) {
  if (force) setupRequest = null;
  if (!setupRequest) {
    setupRequest = apiRequest<SetupStatus>('/api/setup/status')
      .catch(() => ({
        initialized: false,
        database_ready: false,
        initialization_allowed: false,
        deployment_mode: 'standalone' as const,
        backend: 'unknown',
        authorization: {
          mode: 'standalone' as const,
          allowed: false,
          status: 'blocked' as const,
          message: '无法连接初始化服务',
          tenant_id: null,
          tenant_name: null,
          instance_id: null,
          instance_name: null,
          asr_backend_id: null,
          asr_config_source: null,
          license_expires_at: null,
          grace_ends_at: null,
          lease_expires_at: null,
          last_checked_at: null,
          next_check_at: null,
        },
        profile: null,
      }))
      .then((setup) => {
        systemSetup.set(setup);
        return setup;
      });
  }
  return setupRequest;
}

export function resetSession(user: User | null) {
  sessionRequest = user ? Promise.resolve(user) : null;
  currentUser.set(user);
  connectionStatus.set('hidden');
}

export function showError(message: string) {
  showToast(message, 5200, 'error');
}

export function showToast(message: string, duration = 2400, kind: ToastKind = 'info') {
  const id = nextToastId++;
  toastMessages.update((messages) => [...messages, { id, message, kind }]);
  window.setTimeout(() => {
    toastMessages.update((messages) => messages.filter((item) => item.id !== id));
  }, duration);
}
