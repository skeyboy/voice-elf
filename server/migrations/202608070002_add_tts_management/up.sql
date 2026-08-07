CREATE TABLE tts_system_settings (
    id UUID PRIMARY KEY,
    backend_id VARCHAR(64) NOT NULL,
    updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE authority_tenants
    ADD COLUMN tts_backend_id VARCHAR(64) NULL;
