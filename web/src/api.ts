import type { LatencyReport, TranscriptionSegment } from './protocol';

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
  speakers: SpeakerIdentity[];
  refinements: UtteranceRefinement[];
}

export interface UtteranceRefinement {
  engine: string;
  text: string;
  language: string;
  segments: TranscriptionSegment[];
  status: 'processing' | 'completed' | 'failed' | 'interrupted';
  processing_error: string | null;
  created_at: string;
  completed_at: string | null;
}

export interface SpeakerIdentity {
  user_id: string | null;
  username: string;
}

export interface RoomMemberState {
  user_id: string;
  username: string;
  is_owner: boolean;
  is_muted: boolean;
  is_online: boolean;
  is_speaking: boolean;
}

export interface RoomDetail {
  room: RoomSummary;
  members: RoomMemberState[];
  utterances: UtteranceHistory[];
}

export interface RoomInput {
  name: string;
  source_language: string;
  target_language: string;
  max_utterance_seconds: number;
}

export interface VoiceReference {
  id: string;
  name: string;
  duration_ms: number;
  created_at: string;
  audio_url: string;
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
  if (init.body && !(init.body instanceof FormData)) headers.set('content-type', 'application/json');
  const response = await fetch(path, { ...init, headers });
  if (!response.ok) {
    const payload = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new ApiRequestError(payload?.error ?? `请求失败 (${response.status})`, response.status);
  }
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

export function listVoiceReferences() {
  return apiRequest<VoiceReference[]>('/api/voice-references');
}

export function createVoiceReference(name: string, audio: Blob) {
  const body = new FormData();
  body.append('name', name);
  body.append('audio', audio, 'reference.wav');
  return apiRequest<VoiceReference>('/api/voice-references', { method: 'POST', body });
}

export function deleteVoiceReference(id: string) {
  return apiRequest<void>(`/api/voice-references/${encodeURIComponent(id)}`, { method: 'DELETE' });
}
