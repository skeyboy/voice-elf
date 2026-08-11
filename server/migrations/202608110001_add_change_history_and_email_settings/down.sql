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
        EXECUTE format('DROP TRIGGER IF EXISTS %I_change_history ON %I', audited_table, audited_table);
    END LOOP;
END;
$$;

DROP FUNCTION IF EXISTS voice_elf_capture_change_history();
DROP TABLE IF EXISTS data_change_history;
DROP TABLE IF EXISTS system_email_settings;
ALTER TABLE tts_voice_aliases DROP COLUMN IF EXISTS record_status;
