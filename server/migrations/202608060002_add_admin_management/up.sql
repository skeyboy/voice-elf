ALTER TABLE users
    ADD COLUMN role VARCHAR(16) NOT NULL DEFAULT 'member',
    ADD COLUMN status VARCHAR(16) NOT NULL DEFAULT 'active',
    ADD COLUMN verified_at TIMESTAMPTZ NULL,
    ADD COLUMN last_login_at TIMESTAMPTZ NULL;

UPDATE users
SET verified_at = created_at
WHERE verified_at IS NULL;

UPDATE users
SET role = 'admin'
WHERE id = (
    SELECT id
    FROM users
    ORDER BY created_at ASC, id ASC
    LIMIT 1
);

ALTER TABLE users
    ADD CONSTRAINT users_role_check CHECK (role IN ('admin', 'member')),
    ADD CONSTRAINT users_status_check CHECK (status IN ('pending', 'active', 'suspended'));

CREATE INDEX users_status_created_at_idx
    ON users(status, created_at DESC);

ALTER TABLE rooms
    ADD COLUMN status VARCHAR(16) NOT NULL DEFAULT 'active';

ALTER TABLE rooms
    ADD CONSTRAINT rooms_status_check CHECK (status IN ('active', 'ended', 'archived'));

CREATE INDEX rooms_status_updated_at_idx
    ON rooms(status, updated_at DESC)
    WHERE deleted_at IS NULL;
