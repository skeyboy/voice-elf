diesel::table! {
    users (id) {
        id -> Uuid,
        username -> Varchar,
        password_hash -> Text,
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
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
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
diesel::joinable!(room_members -> rooms (room_id));
diesel::joinable!(room_members -> users (user_id));
diesel::joinable!(rooms -> users (owner_id));
diesel::joinable!(voice_sessions -> rooms (room_id));
diesel::joinable!(voice_sessions -> users (user_id));
diesel::joinable!(voice_references -> users (user_id));
diesel::joinable!(voice_utterance_speakers -> voice_utterances (utterance_id));
diesel::joinable!(voice_utterance_refinements -> voice_utterances (utterance_id));
diesel::joinable!(voice_utterances -> rooms (room_id));
diesel::joinable!(voice_utterances -> voice_sessions (session_id));
diesel::allow_tables_to_appear_in_same_query!(
    auth_sessions,
    room_members,
    rooms,
    users,
    voice_references,
    voice_sessions,
    voice_utterance_refinements,
    voice_utterance_speakers,
    voice_utterances,
);
