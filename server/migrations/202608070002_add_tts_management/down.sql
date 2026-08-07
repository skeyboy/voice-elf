ALTER TABLE authority_tenants
    DROP COLUMN IF EXISTS tts_backend_id;

DROP TABLE IF EXISTS tts_system_settings;
