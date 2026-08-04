export type PipelinePhase =
  | 'listening'
  | 'speech'
  | 'transcribing'
  | 'translating'
  | 'synthesizing'
  | 'playing';

export interface SessionConfig {
  source_language: string;
  target_language: string;
  voice: string;
  max_utterance_seconds: number;
}

export interface LatencyReport {
  vad_ms: number;
  stt_ms: number;
  translation_ms: number;
  tts_ms: number;
  total_ms: number;
  audio_ms: number;
  t0_unix_ms: number;
  t1_unix_ms: number;
  t2_unix_ms: number;
  t3_unix_ms: number;
  t4_unix_ms: number;
}

export type ServerEvent =
  | {
      type: 'room_subscribed';
      room_id: string;
      can_publish: boolean;
      backend: string;
    }
  | {
      type: 'ready';
      session_id: string;
      room_id: string;
      backend: string;
      input_sample_rate: number;
    }
  | ({ type: 'configured' } & SessionConfig)
  | { type: 'state'; phase: PipelinePhase; utterance_id: string | null }
  | {
      type: 'vad';
      active: boolean;
      level: number;
      utterance_id: string | null;
      reason:
        | 'silence'
        | 'max_duration'
        | 'manual'
        | 'superseded'
        | 'silent'
        | 'server_limit'
        | 'unknown'
        | null;
      sample_count: number;
    }
  | { type: 'utterance_queued'; utterance_id: string; tc_id: string }
  | {
      type: 'utterance_discarded';
      utterance_id: string;
      tc_id: string;
      reason: string;
    }
  | { type: 'recognition_failed'; utterance_id: string; message: string }
  | {
      type: 'processing_failed';
      utterance_id: string;
      stage: 'translation' | 'tts';
      message: string;
    }
  | {
      type: 'transcript';
      utterance_id: string;
      text: string;
      language: string;
    }
  | {
      type: 'transcript_delta';
      utterance_id: string;
      delta: string;
      text: string;
      language: string;
      done: boolean;
    }
  | {
      type: 'translation';
      utterance_id: string;
      source_text: string;
      translated_text: string;
      source_language: string;
      target_language: string;
    }
  | {
      type: 'translation_delta';
      utterance_id: string;
      delta: string;
      text: string;
      target_language: string;
      done: boolean;
    }
  | {
      type: 'media';
      utterance_id: string;
      source_audio_url: string | null;
      translated_audio_url: string | null;
    }
  | {
      type: 'audio_start';
      utterance_id: string;
      sample_rate: number;
      sample_count: number;
    }
  | { type: 'audio_end'; utterance_id: string }
  | { type: 'latency'; utterance_id: string; latency: LatencyReport }
  | { type: 'warning'; message: string };
