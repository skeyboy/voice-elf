CREATE TABLE system_installations (
    id UUID PRIMARY KEY,
    system_name VARCHAR(64) NOT NULL,
    organization_name VARCHAR(120) NOT NULL,
    public_url TEXT NULL,
    deployment_mode VARCHAR(16) NOT NULL,
    initialized_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    initialized_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT system_installations_mode_check
        CHECK (deployment_mode IN ('standalone', 'bus', 'tenant'))
);

INSERT INTO system_installations (
    id,
    system_name,
    organization_name,
    public_url,
    deployment_mode,
    initialized_by
)
SELECT
    '00000000-0000-0000-0000-000000000001'::UUID,
    'Voice Elf',
    '默认组织',
    NULL,
    'standalone',
    id
FROM users
ORDER BY created_at ASC, id ASC
LIMIT 1;
