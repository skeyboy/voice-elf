import { writable } from 'svelte/store';
import { ApiRequestError, apiRequest, type User } from '../api';
import type { ConnectionStatus } from '../components/topbar';

export const currentUser = writable<User | null | undefined>(undefined);
export const connectionStatus = writable<ConnectionStatus>('hidden');
export const toastMessages = writable<Array<{ id: number; message: string }>>([]);

let sessionRequest: Promise<User | null> | null = null;
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

export function resetSession(user: User | null) {
  sessionRequest = user ? Promise.resolve(user) : null;
  currentUser.set(user);
  connectionStatus.set('hidden');
}

export function showError(message: string) {
  const id = nextToastId++;
  toastMessages.update((messages) => [...messages, { id, message }]);
  window.setTimeout(() => {
    toastMessages.update((messages) => messages.filter((item) => item.id !== id));
  }, 5200);
}
