CREATE TABLE voice_references (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(48) NOT NULL,
    audio_path TEXT NOT NULL,
    duration_ms BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, name)
);

CREATE INDEX voice_references_user_id_created_at_idx
    ON voice_references(user_id, created_at DESC);
