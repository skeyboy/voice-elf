diesel::table! {
    authority_tenants (id) {
        id -> Uuid,
        name -> Varchar,
        slug -> Varchar,
        status -> Varchar,
        license_expires_at -> Timestamptz,
        grace_ends_at -> Timestamptz,
        warning_days -> Int4,
        offline_lease_minutes -> Int4,
        asr_backend_id -> Nullable<Varchar>,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    asr_system_settings (id) {
        id -> Uuid,
        backend_id -> Varchar,
        updated_by -> Nullable<Uuid>,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    authority_instances (id) {
        id -> Uuid,
        tenant_id -> Uuid,
        name -> Varchar,
        client_id -> Varchar,
        secret_hash -> Text,
        status -> Varchar,
        last_seen_at -> Nullable<Timestamptz>,
        last_authorized_at -> Nullable<Timestamptz>,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    authority_access_tokens (id) {
        id -> Uuid,
        instance_id -> Uuid,
        token_hash -> Varchar,
        expires_at -> Timestamptz,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    authority_audit_events (id) {
        id -> Uuid,
        tenant_id -> Nullable<Uuid>,
        instance_id -> Nullable<Uuid>,
        event_type -> Varchar,
        detail -> Text,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    system_installations (id) {
        id -> Uuid,
        system_name -> Varchar,
        organization_name -> Varchar,
        public_url -> Nullable<Text>,
        deployment_mode -> Varchar,
        initialized_by -> Uuid,
        initialized_at -> Timestamptz,
    }
}

diesel::table! {
    users (id) {
        id -> Uuid,
        username -> Varchar,
        password_hash -> Text,
        role -> Varchar,
        status -> Varchar,
        verified_at -> Nullable<Timestamptz>,
        last_login_at -> Nullable<Timestamptz>,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    auth_sessions (id) {
        id -> Uuid,
        user_id -> Uuid,
        token_hash -> Varchar,
        expires_at -> Timestamptz,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    rooms (id) {
        id -> Uuid,
        owner_id -> Uuid,
        name -> Varchar,
        source_language -> Varchar,
        target_language -> Varchar,
        max_utterance_seconds -> Int4,
        status -> Varchar,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
        deleted_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    room_members (room_id, user_id) {
        room_id -> Uuid,
        user_id -> Uuid,
        is_muted -> Bool,
        joined_at -> Timestamptz,
    }
}

diesel::table! {
    voice_sessions (id) {
        id -> Uuid,
        user_id -> Nullable<Uuid>,
        room_id -> Nullable<Uuid>,
        backend -> Varchar,
        source_language -> Varchar,
        target_language -> Varchar,
        voice -> Varchar,
        max_utterance_seconds -> Int4,
        started_at -> Timestamptz,
        ended_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    voice_references (id) {
        id -> Uuid,
        user_id -> Uuid,
        name -> Varchar,
        audio_path -> Text,
        duration_ms -> Int8,
        created_at -> Timestamptz,
        deleted_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    voice_utterance_speakers (id) {
        id -> Uuid,
        utterance_id -> Uuid,
        user_id -> Nullable<Uuid>,
        username -> Varchar,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    voice_utterance_refinements (id) {
        id -> Uuid,
        utterance_id -> Uuid,
        engine -> Varchar,
        text -> Text,
        language -> Varchar,
        segments_json -> Text,
        status -> Varchar,
        processing_error -> Nullable<Text>,
        created_at -> Timestamptz,
        completed_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    voice_utterances (id) {
        id -> Uuid,
        session_id -> Uuid,
        user_id -> Nullable<Uuid>,
        room_id -> Nullable<Uuid>,
        source_text -> Text,
        translated_text -> Text,
        source_language -> Varchar,
        target_language -> Varchar,
        source_audio_path -> Nullable<Text>,
        source_audio_url -> Nullable<Text>,
        translated_audio_path -> Nullable<Text>,
        translated_audio_url -> Nullable<Text>,
        audio_ms -> Int8,
        vad_ms -> Int8,
        stt_ms -> Int8,
        translation_ms -> Int8,
        tts_ms -> Int8,
        total_ms -> Int8,
        t0_unix_ms -> Int8,
        t1_unix_ms -> Int8,
        t2_unix_ms -> Int8,
        t3_unix_ms -> Int8,
        t4_unix_ms -> Int8,
        status -> Varchar,
        processing_error -> Nullable<Text>,
        created_at -> Timestamptz,
    }
}

diesel::joinable!(auth_sessions -> users (user_id));
diesel::joinable!(asr_system_settings -> users (updated_by));
diesel::joinable!(authority_access_tokens -> authority_instances (instance_id));
diesel::joinable!(authority_instances -> authority_tenants (tenant_id));
diesel::joinable!(room_members -> rooms (room_id));
diesel::joinable!(room_members -> users (user_id));
diesel::joinable!(rooms -> users (owner_id));
diesel::joinable!(system_installations -> users (initialized_by));
diesel::joinable!(voice_sessions -> rooms (room_id));
diesel::joinable!(voice_sessions -> users (user_id));
diesel::joinable!(voice_references -> users (user_id));
diesel::joinable!(voice_utterance_speakers -> voice_utterances (utterance_id));
diesel::joinable!(voice_utterance_refinements -> voice_utterances (utterance_id));
diesel::joinable!(voice_utterances -> rooms (room_id));
diesel::joinable!(voice_utterances -> voice_sessions (session_id));
diesel::allow_tables_to_appear_in_same_query!(
    auth_sessions,
    asr_system_settings,
    authority_access_tokens,
    authority_audit_events,
    authority_instances,
    authority_tenants,
    room_members,
    rooms,
    system_installations,
    users,
    voice_references,
    voice_sessions,
    voice_utterance_refinements,
    voice_utterance_speakers,
    voice_utterances,
);
