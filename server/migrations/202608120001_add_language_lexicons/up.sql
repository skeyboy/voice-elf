CREATE TABLE terminology_dictionaries (
    id UUID PRIMARY KEY,
    name VARCHAR(120) NOT NULL,
    industry VARCHAR(80) NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    source_language VARCHAR(16) NOT NULL DEFAULT 'auto',
    target_language VARCHAR(16) NOT NULL DEFAULT 'zh',
    status VARCHAR(16) NOT NULL DEFAULT 'active',
    updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ NULL,
    CONSTRAINT terminology_dictionaries_status_check CHECK (status IN ('active', 'disabled', 'deleted'))
);

CREATE UNIQUE INDEX terminology_dictionaries_name_active_idx
    ON terminology_dictionaries (LOWER(name)) WHERE deleted_at IS NULL;

CREATE TABLE terminology_entries (
    id UUID PRIMARY KEY,
    dictionary_id UUID NOT NULL REFERENCES terminology_dictionaries(id) ON DELETE RESTRICT,
    source_term VARCHAR(240) NOT NULL,
    aliases TEXT[] NOT NULL DEFAULT '{}',
    target_term VARCHAR(240) NOT NULL,
    priority INTEGER NOT NULL DEFAULT 100,
    status VARCHAR(16) NOT NULL DEFAULT 'active',
    updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ NULL,
    CONSTRAINT terminology_entries_status_check CHECK (status IN ('active', 'disabled', 'deleted')),
    CONSTRAINT terminology_entries_priority_check CHECK (priority BETWEEN 0 AND 1000)
);

CREATE UNIQUE INDEX terminology_entries_source_active_idx
    ON terminology_entries (dictionary_id, LOWER(source_term)) WHERE deleted_at IS NULL;
CREATE INDEX terminology_entries_dictionary_idx
    ON terminology_entries (dictionary_id, status, priority DESC) WHERE deleted_at IS NULL;

CREATE TABLE blocked_words (
    id UUID PRIMARY KEY,
    word VARCHAR(240) NOT NULL,
    replacement VARCHAR(240) NOT NULL DEFAULT '***',
    match_mode VARCHAR(16) NOT NULL DEFAULT 'substring',
    case_sensitive BOOLEAN NOT NULL DEFAULT FALSE,
    status VARCHAR(16) NOT NULL DEFAULT 'active',
    note TEXT NOT NULL DEFAULT '',
    updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ NULL,
    CONSTRAINT blocked_words_match_mode_check CHECK (match_mode IN ('substring', 'word')),
    CONSTRAINT blocked_words_status_check CHECK (status IN ('active', 'disabled', 'deleted'))
);

CREATE UNIQUE INDEX blocked_words_word_active_idx
    ON blocked_words (LOWER(word), match_mode) WHERE deleted_at IS NULL;

CREATE TABLE room_terminology_bindings (
    room_id UUID PRIMARY KEY REFERENCES rooms(id) ON DELETE RESTRICT,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    dictionary_id UUID NOT NULL REFERENCES terminology_dictionaries(id) ON DELETE RESTRICT,
    status VARCHAR(16) NOT NULL DEFAULT 'active',
    updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ NULL,
    CONSTRAINT room_terminology_bindings_status_check CHECK (status IN ('active', 'deleted'))
);

DO $$
DECLARE
    audited_table TEXT;
BEGIN
    FOREACH audited_table IN ARRAY ARRAY[
        'terminology_dictionaries',
        'terminology_entries',
        'blocked_words',
        'room_terminology_bindings'
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
