ALTER TABLE voice_sessions
    DROP CONSTRAINT IF EXISTS voice_sessions_max_utterance_seconds_check;

ALTER TABLE rooms
    DROP CONSTRAINT IF EXISTS rooms_max_utterance_seconds_check;
