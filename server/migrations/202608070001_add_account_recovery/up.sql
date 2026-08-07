ALTER TABLE users
    ADD COLUMN email VARCHAR(254) NULL;

CREATE UNIQUE INDEX users_email_lower_unique_idx
    ON users (LOWER(email))
    WHERE email IS NOT NULL;

CREATE TABLE password_reset_tokens (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash VARCHAR(64) NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX password_reset_tokens_user_created_idx
    ON password_reset_tokens(user_id, created_at DESC);

CREATE INDEX password_reset_tokens_active_idx
    ON password_reset_tokens(token_hash, expires_at)
    WHERE consumed_at IS NULL;
