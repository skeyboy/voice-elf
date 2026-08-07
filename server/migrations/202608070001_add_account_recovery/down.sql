DROP TABLE IF EXISTS password_reset_tokens;
DROP INDEX IF EXISTS users_email_lower_unique_idx;
ALTER TABLE users DROP COLUMN IF EXISTS email;
