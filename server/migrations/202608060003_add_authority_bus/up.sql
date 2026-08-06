CREATE TABLE authority_tenants (
    id UUID PRIMARY KEY,
    name VARCHAR(120) NOT NULL,
    slug VARCHAR(48) NOT NULL UNIQUE,
    status VARCHAR(16) NOT NULL DEFAULT 'active',
    license_expires_at TIMESTAMPTZ NOT NULL,
    grace_ends_at TIMESTAMPTZ NOT NULL,
    warning_days INTEGER NOT NULL DEFAULT 30,
    offline_lease_minutes INTEGER NOT NULL DEFAULT 1440,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT authority_tenants_status_check
        CHECK (status IN ('active', 'suspended', 'revoked')),
    CONSTRAINT authority_tenants_warning_days_check
        CHECK (warning_days BETWEEN 1 AND 180),
    CONSTRAINT authority_tenants_offline_lease_check
        CHECK (offline_lease_minutes BETWEEN 5 AND 10080),
    CONSTRAINT authority_tenants_grace_check
        CHECK (grace_ends_at >= license_expires_at)
);

CREATE INDEX authority_tenants_status_expiry_idx
    ON authority_tenants(status, license_expires_at);

CREATE TABLE authority_instances (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES authority_tenants(id) ON DELETE CASCADE,
    name VARCHAR(120) NOT NULL,
    client_id VARCHAR(80) NOT NULL UNIQUE,
    secret_hash TEXT NOT NULL,
    status VARCHAR(16) NOT NULL DEFAULT 'active',
    last_seen_at TIMESTAMPTZ NULL,
    last_authorized_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT authority_instances_status_check
        CHECK (status IN ('active', 'revoked'))
);

CREATE INDEX authority_instances_tenant_status_idx
    ON authority_instances(tenant_id, status, created_at DESC);

CREATE TABLE authority_access_tokens (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL REFERENCES authority_instances(id) ON DELETE CASCADE,
    token_hash VARCHAR(64) NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX authority_access_tokens_instance_expiry_idx
    ON authority_access_tokens(instance_id, expires_at);

CREATE TABLE authority_audit_events (
    id UUID PRIMARY KEY,
    tenant_id UUID NULL REFERENCES authority_tenants(id) ON DELETE SET NULL,
    instance_id UUID NULL REFERENCES authority_instances(id) ON DELETE SET NULL,
    event_type VARCHAR(48) NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX authority_audit_events_tenant_created_idx
    ON authority_audit_events(tenant_id, created_at DESC);
