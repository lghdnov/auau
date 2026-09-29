
CREATE TABLE users (
    id           uuid PRIMARY KEY,
    oidc_subject text NOT NULL UNIQUE,
    name         text NOT NULL,
    email        text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE projects (
    id          uuid PRIMARY KEY,
    name        text NOT NULL UNIQUE,
    description text,
    created_by  uuid NOT NULL REFERENCES users (id),
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE labels (
    id         uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects (id),
    name       text NOT NULL,
    color      text NOT NULL,
    hotkey     text,
    sort_order int  NOT NULL DEFAULT 0,
    archived   bool NOT NULL DEFAULT false,
    UNIQUE (project_id, name)
);

CREATE UNIQUE INDEX labels_project_hotkey_key
    ON labels (project_id, hotkey)
    WHERE hotkey IS NOT NULL;

CREATE INDEX labels_project_id_idx ON labels (project_id);

CREATE TABLE audio_files (
    id               uuid PRIMARY KEY,
    project_id       uuid NOT NULL REFERENCES projects (id),
    original_name    text NOT NULL,
    storage_key      text NOT NULL,
    sha256           text NOT NULL,
    duration_ms      int,
    sample_rate      int,
    channels         int,
    mime             text NOT NULL,
    size_bytes       bigint NOT NULL,
    status           text NOT NULL DEFAULT 'processing'
                     CHECK (status IN ('processing', 'new', 'in_progress', 'done', 'error')),
    error_message    text,
    claimed_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    claim_expires_at timestamptz,
    uploaded_by      uuid NOT NULL REFERENCES users (id),
    created_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, sha256)
);

CREATE INDEX audio_files_project_status_idx ON audio_files (project_id, status);

CREATE TABLE annotations (
    id         uuid PRIMARY KEY,
    file_id    uuid NOT NULL REFERENCES audio_files (id) ON DELETE CASCADE,
    label_id   uuid NOT NULL REFERENCES labels (id),
    start_ms   int  NOT NULL CHECK (start_ms >= 0),
    end_ms     int  NOT NULL CHECK (end_ms > start_ms),
    created_by uuid NOT NULL REFERENCES users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX annotations_file_id_idx ON annotations (file_id);
