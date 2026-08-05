CREATE TABLE voice_utterance_refinements (
    id UUID PRIMARY KEY,
    utterance_id UUID NOT NULL REFERENCES voice_utterances(id) ON DELETE CASCADE,
    engine VARCHAR(64) NOT NULL,
    text TEXT NOT NULL DEFAULT '',
    language VARCHAR(16) NOT NULL DEFAULT 'auto',
    segments_json TEXT NOT NULL DEFAULT '[]',
    status VARCHAR(32) NOT NULL DEFAULT 'processing',
    processing_error TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ NULL,
    UNIQUE (utterance_id, engine)
);

CREATE INDEX voice_utterance_refinements_utterance_idx
ON voice_utterance_refinements (utterance_id, created_at);
