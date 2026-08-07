CREATE TABLE tts_voice_aliases (
    provider_id VARCHAR(64) NOT NULL,
    voice_id VARCHAR(64) NOT NULL,
    alias VARCHAR(64) NOT NULL,
    updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (provider_id, voice_id)
);
