DROP INDEX IF EXISTS voice_references_active_user_created_at_idx;
DROP INDEX IF EXISTS voice_references_active_user_name_idx;

ALTER TABLE voice_references
    DROP COLUMN deleted_at;

ALTER TABLE voice_references
    ADD CONSTRAINT voice_references_user_id_name_key UNIQUE (user_id, name);

DROP INDEX IF EXISTS rooms_active_owner_updated_at_idx;

ALTER TABLE rooms
    DROP COLUMN deleted_at;
