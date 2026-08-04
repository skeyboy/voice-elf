UPDATE rooms
SET max_utterance_seconds = 20
WHERE max_utterance_seconds > 20;

UPDATE voice_sessions
SET max_utterance_seconds = 20
WHERE max_utterance_seconds > 20;

ALTER TABLE rooms
    ADD CONSTRAINT rooms_max_utterance_seconds_check
    CHECK (max_utterance_seconds BETWEEN 5 AND 20);

ALTER TABLE voice_sessions
    ADD CONSTRAINT voice_sessions_max_utterance_seconds_check
    CHECK (max_utterance_seconds BETWEEN 5 AND 20);
