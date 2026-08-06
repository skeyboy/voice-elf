ALTER TABLE authority_tenants
    DROP COLUMN IF EXISTS asr_backend_id;

DROP TABLE IF EXISTS asr_system_settings;
