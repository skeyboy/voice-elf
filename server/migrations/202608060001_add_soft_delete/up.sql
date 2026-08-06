ALTER TABLE rooms
    ADD COLUMN deleted_at TIMESTAMPTZ NULL;

CREATE INDEX rooms_active_owner_updated_at_idx
    ON rooms(owner_id, updated_at DESC)
    WHERE deleted_at IS NULL;

ALTER TABLE voice_references
    ADD COLUMN deleted_at TIMESTAMPTZ NULL;

ALTER TABLE voice_references
    DROP CONSTRAINT IF EXISTS voice_references_user_id_name_key;

CREATE UNIQUE INDEX voice_references_active_user_name_idx
    ON voice_references(user_id, name)
    WHERE deleted_at IS NULL;

CREATE INDEX voice_references_active_user_created_at_idx
    ON voice_references(user_id, created_at DESC)
    WHERE deleted_at IS NULL;
