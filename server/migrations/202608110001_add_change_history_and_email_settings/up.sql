CREATE TABLE system_email_settings (
    id UUID PRIMARY KEY,
    version BIGINT NOT NULL UNIQUE,
    record_status VARCHAR(16) NOT NULL DEFAULT 'current',
    enabled BOOLEAN NOT NULL,
    host VARCHAR(255) NOT NULL,
    port INTEGER NOT NULL,
    security VARCHAR(16) NOT NULL,
    username VARCHAR(255) NOT NULL,
    password_secret TEXT NULL,
    from_address VARCHAR(254) NOT NULL,
    from_name VARCHAR(128) NOT NULL,
    public_url TEXT NULL,
    reset_expiry_minutes INTEGER NOT NULL,
    updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT system_email_settings_status_check
        CHECK (record_status IN ('current', 'historical', 'deleted')),
    CONSTRAINT system_email_settings_security_check
        CHECK (security IN ('wrapper', 'starttls', 'none')),
    CONSTRAINT system_email_settings_port_check CHECK (port BETWEEN 1 AND 65535),
    CONSTRAINT system_email_settings_expiry_check
        CHECK (reset_expiry_minutes BETWEEN 5 AND 1440)
);

CREATE UNIQUE INDEX system_email_settings_current_idx
    ON system_email_settings(record_status)
    WHERE record_status = 'current';

CREATE INDEX system_email_settings_created_idx
    ON system_email_settings(created_at DESC);

ALTER TABLE tts_voice_aliases
    ADD COLUMN record_status VARCHAR(16) NOT NULL DEFAULT 'current';

ALTER TABLE tts_voice_aliases
    ADD CONSTRAINT tts_voice_aliases_record_status_check
        CHECK (record_status IN ('current', 'deleted'));

CREATE TABLE data_change_history (
    id UUID PRIMARY KEY,
    entity_type VARCHAR(64) NOT NULL,
    entity_id VARCHAR(255) NOT NULL,
    action VARCHAR(16) NOT NULL,
    record_status VARCHAR(16) NOT NULL,
    actor_user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    before_state JSONB NULL,
    after_state JSONB NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT data_change_history_action_check
        CHECK (action IN ('create', 'update', 'delete')),
    CONSTRAINT data_change_history_status_check
        CHECK (record_status IN ('current', 'historical', 'deleted'))
);

CREATE INDEX data_change_history_entity_idx
    ON data_change_history(entity_type, entity_id, created_at DESC);

CREATE INDEX data_change_history_created_idx
    ON data_change_history(created_at DESC);

CREATE INDEX data_change_history_actor_idx
    ON data_change_history(actor_user_id, created_at DESC)
    WHERE actor_user_id IS NOT NULL;

CREATE OR REPLACE FUNCTION voice_elf_capture_change_history()
RETURNS TRIGGER AS $$
DECLARE
    before_row JSONB;
    after_row JSONB;
    identity_row JSONB;
    resolved_entity_id TEXT;
    resolved_actor_id UUID;
BEGIN
    IF TG_OP = 'INSERT' THEN
        before_row := NULL;
        after_row := to_jsonb(NEW);
        identity_row := after_row;
    ELSIF TG_OP = 'UPDATE' THEN
        before_row := to_jsonb(OLD);
        after_row := to_jsonb(NEW);
        identity_row := after_row;
        IF before_row = after_row THEN
            RETURN NEW;
        END IF;
    ELSE
        before_row := to_jsonb(OLD);
        after_row := NULL;
        identity_row := before_row;
    END IF;

    resolved_entity_id := CASE TG_TABLE_NAME
        WHEN 'room_members' THEN concat_ws(':', identity_row->>'room_id', identity_row->>'user_id')
        WHEN 'tts_voice_aliases' THEN concat_ws(':', identity_row->>'provider_id', identity_row->>'voice_id')
        ELSE COALESCE(identity_row->>'id', md5(identity_row::TEXT))
    END;

    IF COALESCE(identity_row->>'updated_by', '') ~
        '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$'
    THEN
        resolved_actor_id := (identity_row->>'updated_by')::UUID;
    ELSE
        resolved_actor_id := NULL;
    END IF;

    UPDATE data_change_history
    SET record_status = 'historical'
    WHERE entity_type = TG_TABLE_NAME
      AND entity_id = resolved_entity_id
      AND record_status = 'current';

    INSERT INTO data_change_history (
        id,
        entity_type,
        entity_id,
        action,
        record_status,
        actor_user_id,
        before_state,
        after_state
    ) VALUES (
        gen_random_uuid(),
        TG_TABLE_NAME,
        resolved_entity_id,
        CASE TG_OP WHEN 'INSERT' THEN 'create' WHEN 'UPDATE' THEN 'update' ELSE 'delete' END,
        CASE TG_OP WHEN 'DELETE' THEN 'deleted' ELSE 'current' END,
        resolved_actor_id,
        before_row - ARRAY['password_hash', 'secret_hash', 'token_hash', 'password_secret'],
        after_row - ARRAY['password_hash', 'secret_hash', 'token_hash', 'password_secret']
    );

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DO $$
DECLARE
    audited_table TEXT;
BEGIN
    FOREACH audited_table IN ARRAY ARRAY[
        'users',
        'rooms',
        'room_members',
        'voice_sessions',
        'voice_utterances',
        'voice_utterance_speakers',
        'voice_utterance_refinements',
        'voice_references',
        'system_installations',
        'system_email_settings',
        'asr_system_settings',
        'tts_system_settings',
        'tts_voice_aliases',
        'authority_tenants',
        'authority_instances'
    ] LOOP
        EXECUTE format(
            'CREATE TRIGGER %I_change_history AFTER INSERT OR UPDATE OR DELETE ON %I '
            'FOR EACH ROW EXECUTE FUNCTION voice_elf_capture_change_history()',
            audited_table,
            audited_table
        );
    END LOOP;
END;
$$;
