import type { LatencyReport, TranscriptionSegment } from './protocol';
import { grpcApiCall } from './grpc-web';

export interface User {
  id: string;
  username: string;
  email: string | null;
  role: 'admin' | 'member';
  status: 'pending' | 'active' | 'suspended';
  verified_at: string | null;
  last_login_at: string | null;
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
  status: 'active' | 'ended' | 'archived';
  created_at: string;
  updated_at: string;
  is_owner: boolean;
  is_member: boolean;
  member_count: number;
  utterance_count: number;
  duration_ms: number;
  last_activity_at: string;
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

export interface TerminologyDictionary {
  id: string;
  name: string;
  industry: string;
  description: string;
  source_language: string;
  target_language: string;
  status: 'active' | 'disabled';
  created_at: string;
  updated_at: string;
}

export interface TerminologyEntry {
  id: string;
  dictionary_id: string;
  source_term: string;
  aliases: string[];
  target_term: string;
  priority: number;
  status: 'active' | 'disabled';
}

export interface BlockedWord {
  id: string;
  word: string;
  replacement: string;
  match_mode: 'substring' | 'word';
  case_sensitive: boolean;
  status: 'active' | 'disabled';
  note: string;
}

export interface RoomTerminologyBinding {
  dictionary_id: string | null;
  dictionary_name: string | null;
}

export interface VoiceReference {
  id: string;
  name: string;
  duration_ms: number;
  created_at: string;
  audio_url: string;
}

export interface AdminOverview {
  total_users: number;
  pending_users: number;
  suspended_users: number;
  active_rooms: number;
  total_rooms: number;
}

export interface RuntimeDependency {
  name: string;
  kind: string;
  required: boolean;
  status: 'ready' | 'degraded' | 'unavailable' | 'unknown';
  message: string;
  checked_at: string;
}

export interface RuntimeSnapshot {
  service: string;
  overall_status: 'ready' | 'degraded' | 'unavailable' | 'unknown';
  generated_at: string;
  initialized: boolean;
  authorized: boolean;
  dependencies: RuntimeDependency[];
  version: string;
}

export interface InstanceAuthorization {
  mode: 'standalone' | 'bus' | 'tenant';
  allowed: boolean;
  status: 'standalone' | 'checking' | 'authorized' | 'warning' | 'grace' | 'blocked';
  message: string;
  tenant_id: string | null;
  tenant_name: string | null;
  instance_id: string | null;
  instance_name: string | null;
  asr_backend_id: string | null;
  asr_config_source: 'system' | 'tenant' | null;
  tts_backend_id: string | null;
  tts_config_source: 'system' | 'tenant' | null;
  license_expires_at: string | null;
  grace_ends_at: string | null;
  lease_expires_at: string | null;
  last_checked_at: string | null;
  next_check_at: string | null;
}

export interface SystemProfile {
  id: string;
  system_name: string;
  organization_name: string;
  public_url: string | null;
  deployment_mode: 'standalone' | 'bus' | 'tenant';
  initialized_by: string;
  initialized_at: string;
}

export interface SetupStatus {
  initialized: boolean;
  database_ready: boolean;
  initialization_allowed: boolean;
  deployment_mode: 'standalone' | 'bus' | 'tenant';
  backend: string;
  authorization: InstanceAuthorization;
  email_ready: boolean;
  profile: SystemProfile | null;
}

export interface InitializeSystemInput {
  setup_token: string;
  system_name: string;
  organization_name: string;
  public_url: string;
  admin_username: string;
  admin_email: string;
  admin_password: string;
}

export interface InitializeSystemResponse {
  profile: SystemProfile;
  user: User;
}

export interface AuthorityTenant {
  id: string;
  name: string;
  slug: string;
  status: 'active' | 'suspended' | 'revoked';
  license_expires_at: string;
  grace_ends_at: string;
  warning_days: number;
  offline_lease_minutes: number;
  asr_backend_id: string | null;
  tts_backend_id: string | null;
  created_at: string;
  updated_at: string;
  instance_count: number;
  last_seen_at: string | null;
}

export interface AuthorityInstance {
  id: string;
  tenant_id: string;
  name: string;
  client_id: string;
  status: 'active' | 'revoked';
  last_seen_at: string | null;
  last_authorized_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface IssuedAuthorityCredential {
  instance: AuthorityInstance;
  client_secret: string;
}

export interface AsrProvider {
  id: string;
  name: string;
  engine: string;
  description: string;
  available: boolean;
  production: boolean;
}

export interface AsrSystemSetting {
  id: string;
  backend_id: string;
  updated_by: string | null;
  updated_at: string;
}

export interface EffectiveAsrSelection {
  backend_id: string;
  source: 'system' | 'tenant';
  tenant_id: string | null;
  tenant_name: string | null;
}

export interface AsrManagement {
  providers: AsrProvider[];
  system_setting: AsrSystemSetting;
  effective: EffectiveAsrSelection;
  can_update_system: boolean;
  applies_to: 'new_room_pipelines';
  fun_asr_runtime: FunAsrRuntimeStatus;
}

export interface FunAsrRuntimeStatus {
  enabled: boolean;
  healthy: boolean;
  message: string;
}

export interface TtsProvider extends AsrProvider {
  voice_clone: boolean;
}

export interface TtsVoice {
  id: string;
  default_name: string;
  display_name: string;
  alias: string | null;
  group: string;
  description: string;
  languages: string[];
}

export interface TtsVoiceCatalog {
  provider: TtsProvider;
  voices: TtsVoice[];
  supports_custom_voices: boolean;
}

export interface IndexTtsRuntimeStatus {
  phase: 'unavailable' | 'not_installed' | 'installing' | 'stopped' | 'starting' | 'ready' | 'stopping' | 'error';
  script_available: boolean;
  model_ready: boolean;
  running: boolean;
  healthy: boolean;
  action: string | null;
  message: string;
  model_dir: string;
  log_path: string;
}

export interface QwenTtsRuntimeStatus {
  enabled: boolean;
  healthy: boolean;
  message: string;
  base_url: string;
  model: string;
}

export interface TtsManagement {
  providers: TtsProvider[];
  system_setting: AsrSystemSetting;
  effective: EffectiveAsrSelection;
  can_update_system: boolean;
  applies_to: 'new_room_pipelines';
  voices: TtsVoice[];
  index_tts_runtime: IndexTtsRuntimeStatus;
  qwen_tts_runtime: QwenTtsRuntimeStatus;
}

export interface AdminUser {
  id: string;
  username: string;
  email: string | null;
  role: User['role'];
  status: User['status'];
  verified_at: string | null;
  last_login_at: string | null;
  created_at: string;
  owned_room_count: number;
  joined_room_count: number;
  utterance_count: number;
  last_activity_at: string | null;
}

export interface MailStatus {
  enabled: boolean;
  configured: boolean;
  password_configured: boolean;
  host: string;
  port: number;
  security: 'wrapper' | 'starttls' | 'none';
  username: string;
  from_address: string;
  from_name: string;
  public_url: string | null;
  reset_expiry_minutes: number;
}

export interface ChangeHistoryRecord {
  id: string;
  entity_type: string;
  entity_id: string;
  action: 'create' | 'update' | 'delete';
  record_status: 'current' | 'historical' | 'deleted';
  actor_user_id: string | null;
  before_state: Record<string, unknown> | null;
  after_state: Record<string, unknown> | null;
  created_at: string;
}

export interface PasswordResetStatus {
  email_enabled: boolean;
  reset_expiry_minutes: number;
}

export interface Paginated<T> {
  items: T[];
  page: number;
  page_size: number;
  total: number;
  total_pages: number;
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
  const response = await grpcApiCall(path, init);
  const text = response.body.length ? new TextDecoder().decode(response.body) : '';
  if (response.status < 200 || response.status >= 300) {
    const payload = safeJson<{ error?: string }>(text);
    throw new ApiRequestError(payload?.error ?? `请求失败 (${response.status})`, response.status);
  }
  if (response.status === 204) return undefined as T;
  return JSON.parse(text) as T;
}

function safeJson<T>(value: string): T | null {
  try {
    return JSON.parse(value) as T;
  } catch {
    return null;
  }
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

export function listTtsVoices() {
  return apiRequest<TtsVoiceCatalog>('/api/tts/voices');
}
