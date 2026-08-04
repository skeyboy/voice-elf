DROP TABLE IF EXISTS voice_utterance_speakers;

ALTER TABLE room_members
DROP COLUMN IF EXISTS is_muted;
