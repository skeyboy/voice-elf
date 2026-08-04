ALTER TABLE room_members
ADD COLUMN is_muted BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE voice_utterance_speakers (
    id UUID PRIMARY KEY,
    utterance_id UUID NOT NULL REFERENCES voice_utterances(id) ON DELETE CASCADE,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    username VARCHAR(64) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (utterance_id, user_id)
);

CREATE INDEX voice_utterance_speakers_utterance_idx
ON voice_utterance_speakers (utterance_id, created_at);
