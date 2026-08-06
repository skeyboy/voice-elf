DROP INDEX IF EXISTS rooms_status_updated_at_idx;

ALTER TABLE rooms
    DROP CONSTRAINT IF EXISTS rooms_status_check,
    DROP COLUMN status;

DROP INDEX IF EXISTS users_status_created_at_idx;

ALTER TABLE users
    DROP CONSTRAINT IF EXISTS users_status_check,
    DROP CONSTRAINT IF EXISTS users_role_check,
    DROP COLUMN last_login_at,
    DROP COLUMN verified_at,
    DROP COLUMN status,
    DROP COLUMN role;
