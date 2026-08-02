CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY,
    username VARCHAR(64) NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS auth_sessions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash VARCHAR(64) NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS rooms (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(120) NOT NULL,
    source_language VARCHAR(16) NOT NULL DEFAULT 'auto',
    target_language VARCHAR(16) NOT NULL DEFAULT 'zh',
    max_utterance_seconds INTEGER NOT NULL DEFAULT 20,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS room_members (
    room_id UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (room_id, user_id)
);

CREATE TABLE IF NOT EXISTS voice_sessions (
    id UUID PRIMARY KEY,
    user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    room_id UUID NULL REFERENCES rooms(id) ON DELETE CASCADE,
    backend VARCHAR(32) NOT NULL,
    source_language VARCHAR(16) NOT NULL,
    target_language VARCHAR(16) NOT NULL,
    voice VARCHAR(64) NOT NULL,
    max_utterance_seconds INTEGER NOT NULL DEFAULT 20,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at TIMESTAMPTZ NULL
);

CREATE TABLE IF NOT EXISTS voice_utterances (
    id UUID PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES voice_sessions(id) ON DELETE CASCADE,
    user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    room_id UUID NULL REFERENCES rooms(id) ON DELETE CASCADE,
    source_text TEXT NOT NULL,
    translated_text TEXT NOT NULL,
    source_language VARCHAR(16) NOT NULL,
    target_language VARCHAR(16) NOT NULL,
    source_audio_path TEXT NULL,
    source_audio_url TEXT NULL,
    translated_audio_path TEXT NULL,
    translated_audio_url TEXT NULL,
    audio_ms BIGINT NOT NULL,
    vad_ms BIGINT NOT NULL,
    stt_ms BIGINT NOT NULL,
    translation_ms BIGINT NOT NULL,
    tts_ms BIGINT NOT NULL,
    total_ms BIGINT NOT NULL,
    t0_unix_ms BIGINT NOT NULL,
    t1_unix_ms BIGINT NOT NULL,
    t2_unix_ms BIGINT NOT NULL,
    t3_unix_ms BIGINT NOT NULL,
    t4_unix_ms BIGINT NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'completed',
    processing_error TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE voice_utterances
    ADD COLUMN IF NOT EXISTS source_audio_path TEXT,
    ADD COLUMN IF NOT EXISTS source_audio_url TEXT,
    ADD COLUMN IF NOT EXISTS translated_audio_path TEXT,
    ADD COLUMN IF NOT EXISTS translated_audio_url TEXT;

ALTER TABLE voice_sessions
    ADD COLUMN IF NOT EXISTS user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS room_id UUID NULL REFERENCES rooms(id) ON DELETE CASCADE,
    ADD COLUMN IF NOT EXISTS max_utterance_seconds INTEGER NOT NULL DEFAULT 20;

ALTER TABLE rooms
    ADD COLUMN IF NOT EXISTS max_utterance_seconds INTEGER NOT NULL DEFAULT 20;

ALTER TABLE voice_utterances
    ADD COLUMN IF NOT EXISTS room_id UUID NULL REFERENCES rooms(id) ON DELETE CASCADE,
    ADD COLUMN IF NOT EXISTS user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS status VARCHAR(32) NOT NULL DEFAULT 'completed',
    ADD COLUMN IF NOT EXISTS processing_error TEXT NULL;

CREATE INDEX IF NOT EXISTS auth_sessions_token_hash_idx
    ON auth_sessions(token_hash);
CREATE INDEX IF NOT EXISTS auth_sessions_user_id_idx
    ON auth_sessions(user_id);
CREATE INDEX IF NOT EXISTS rooms_owner_id_idx
    ON rooms(owner_id);
CREATE INDEX IF NOT EXISTS rooms_updated_at_idx
    ON rooms(updated_at DESC);
CREATE INDEX IF NOT EXISTS room_members_user_id_idx
    ON room_members(user_id);
CREATE INDEX IF NOT EXISTS voice_sessions_room_id_idx
    ON voice_sessions(room_id);
CREATE INDEX IF NOT EXISTS voice_utterances_room_id_idx
    ON voice_utterances(room_id);

CREATE INDEX IF NOT EXISTS voice_utterances_session_id_idx
    ON voice_utterances(session_id);
CREATE INDEX IF NOT EXISTS voice_utterances_created_at_idx
    ON voice_utterances(created_at DESC);
