import type { LatencyReport } from './protocol';

export interface User {
  id: string;
  username: string;
  created_at: string;
}

export interface RoomSummary {
  id: string;
  owner_id: string;
  owner_username: string;
  name: string;
  source_language: string;
  target_language: string;
  max_utterance_seconds: number;
  created_at: string;
  updated_at: string;
  is_owner: boolean;
  is_member: boolean;
  member_count: number;
  utterance_count: number;
  preview_text: string | null;
}

export interface UtteranceHistory {
  id: string;
  source_text: string;
  translated_text: string;
  source_language: string;
  target_language: string;
  source_audio_url: string | null;
  translated_audio_url: string | null;
  status: string;
  processing_error: string | null;
  created_at: string;
  latency: LatencyReport;
}

export interface RoomDetail {
  room: RoomSummary;
  utterances: UtteranceHistory[];
}

export interface RoomInput {
  name: string;
  source_language: string;
  target_language: string;
  max_utterance_seconds: number;
}

export class ApiRequestError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
  }
}

export async function apiRequest<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  if (init.body) headers.set('content-type', 'application/json');
  const response = await fetch(path, { ...init, headers });
  if (!response.ok) {
    const payload = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new ApiRequestError(payload?.error ?? `请求失败 (${response.status})`, response.status);
  }
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}
