CREATE OR REPLACE FUNCTION voice_elf_capture_change_history()
RETURNS TRIGGER AS $$
DECLARE
    before_row JSONB;
    after_row JSONB;
    identity_row JSONB;
    resolved_entity_id TEXT;
    resolved_actor_id UUID;
    is_soft_delete BOOLEAN := FALSE;
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

    IF TG_OP <> 'DELETE' THEN
        is_soft_delete := COALESCE(after_row->>'record_status', '') = 'deleted'
            OR NULLIF(after_row->>'deleted_at', '') IS NOT NULL;
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
        CASE
            WHEN TG_OP = 'DELETE' OR is_soft_delete THEN 'delete'
            WHEN TG_OP = 'INSERT' THEN 'create'
            ELSE 'update'
        END,
        CASE WHEN TG_OP = 'DELETE' OR is_soft_delete THEN 'deleted' ELSE 'current' END,
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
