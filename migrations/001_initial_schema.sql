CREATE TABLE media_types (
    code VARCHAR(8) PRIMARY KEY,
    name VARCHAR(32) UNIQUE NOT NULL,
    layer_count INT NOT NULL DEFAULT 1,
    pic BOOLEAN NOT NULL DEFAULT FALSE,
    rom_extension VARCHAR(8) NOT NULL DEFAULT 'iso'
);

CREATE TABLE categories (
    id SERIAL PRIMARY KEY,
    name VARCHAR(16) UNIQUE NOT NULL
);

CREATE TABLE regions (
    code CHAR(2) PRIMARY KEY,
    name VARCHAR(32) UNIQUE NOT NULL,
    flag_code CHAR(2) NOT NULL,
    sort_order INT NOT NULL DEFAULT 0
);

CREATE TABLE languages (
    code CHAR(2) PRIMARY KEY,
    name VARCHAR(16) NOT NULL,
    flag_code CHAR(2) NOT NULL,
    sort_order INT NOT NULL DEFAULT 0
);

CREATE TABLE systems (
    code VARCHAR(16) PRIMARY KEY,
    type VARCHAR(8) NOT NULL,
    manufacturer VARCHAR(32) NOT NULL,
    name VARCHAR(64) NOT NULL,
    short_name VARCHAR(32) NOT NULL DEFAULT '',
    media_types TEXT[] NOT NULL DEFAULT '{}',

    -- optional fields
    has_title_foreign BOOLEAN NOT NULL DEFAULT FALSE,
    has_disc_number BOOLEAN NOT NULL DEFAULT FALSE,
    has_disc_title BOOLEAN NOT NULL DEFAULT FALSE,
    has_serial BOOLEAN NOT NULL DEFAULT FALSE,
    has_edition BOOLEAN NOT NULL DEFAULT FALSE,
    has_barcode BOOLEAN NOT NULL DEFAULT FALSE,
    has_version BOOLEAN NOT NULL DEFAULT FALSE,
    has_exe_date BOOLEAN NOT NULL DEFAULT FALSE,
    has_edc BOOLEAN NOT NULL DEFAULT FALSE,
    has_disc_id BOOLEAN NOT NULL DEFAULT FALSE,
    has_key BOOLEAN NOT NULL DEFAULT FALSE,
    has_protection BOOLEAN NOT NULL DEFAULT FALSE,
    has_sector_ranges BOOLEAN NOT NULL DEFAULT FALSE,
    has_sbi BOOLEAN NOT NULL DEFAULT FALSE,
    has_pvd BOOLEAN NOT NULL DEFAULT FALSE,
    has_header BOOLEAN NOT NULL DEFAULT FALSE,
    has_bca BOOLEAN NOT NULL DEFAULT FALSE,
    -- ring code
    has_sample_start BOOLEAN NOT NULL DEFAULT FALSE,
    has_offset_extra BOOLEAN NOT NULL DEFAULT FALSE
);

-- discs (main catalog)
CREATE TABLE discs (
    id SERIAL PRIMARY KEY,
    system_code VARCHAR(16) NOT NULL REFERENCES systems(code),
    media_type_code VARCHAR(8) NOT NULL REFERENCES media_types(code),
    category_id INT NOT NULL REFERENCES categories(id) DEFAULT 1,
    title VARCHAR(512) NOT NULL,
    title_foreign VARCHAR(512),
    disc_number VARCHAR(64),
    disc_title VARCHAR(512),
    filename_suffix VARCHAR(255),
    serial TEXT[] NOT NULL DEFAULT '{}',
    edition TEXT[] NOT NULL DEFAULT '{}',
    barcode TEXT[] NOT NULL DEFAULT '{}',
    version VARCHAR(64),
    error_count INT,
    exe_date VARCHAR(16),
    edc BOOLEAN NOT NULL DEFAULT FALSE,
    layerbreaks INT[],
    disc_id TEXT,
    disc_key BYTEA,
    comments TEXT,
    contents TEXT,
    protection TEXT,
    sector_ranges INT4RANGE[],
    sbi TEXT,
    pvd BYTEA,
    header BYTEA,
    bca BYTEA,
    pic BYTEA,
    cue TEXT,

    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    questionable BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX idx_discs_system ON discs(system_code);
CREATE INDEX idx_discs_title ON discs(title);
CREATE INDEX idx_discs_enabled ON discs(enabled) WHERE NOT enabled;

-- immutable wrapper for array_to_string (needed for generated columns)
CREATE FUNCTION arr_to_str(TEXT[], TEXT) RETURNS TEXT
    LANGUAGE SQL IMMUTABLE PARALLEL SAFE
    AS $$ SELECT array_to_string($1, $2) $$;

-- full-text search
ALTER TABLE discs ADD COLUMN search_vector tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(title_foreign, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(disc_title, '')), 'B') ||
        setweight(to_tsvector('english', coalesce(arr_to_str(serial, ' '), '')), 'B') ||
        setweight(to_tsvector('english', coalesce(arr_to_str(barcode, ' '), '')), 'B') ||
        setweight(to_tsvector('english', coalesce(comments, '')), 'C') ||
        setweight(to_tsvector('english', coalesce(contents, '')), 'C') ||
        setweight(to_tsvector('english', coalesce(protection, '')), 'D')
    ) STORED;
CREATE INDEX idx_discs_search ON discs USING GIN(search_vector);

-- disc <-> regions junction
CREATE TABLE disc_regions (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    region_code CHAR(2) NOT NULL REFERENCES regions(code),
    PRIMARY KEY (disc_id, region_code)
);

-- disc <-> languages junction
CREATE TABLE disc_languages (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    language_code CHAR(2) NOT NULL REFERENCES languages(code),
    PRIMARY KEY (disc_id, language_code)
);

-- ring code entries
CREATE TABLE disc_ring_code_entries (
    id SERIAL PRIMARY KEY,
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    offset_value INT,
    offset_extra_value INT,
    sample_data_start INT,
    comment TEXT
);
CREATE INDEX idx_ring_entries_disc ON disc_ring_code_entries(disc_id);

-- ring code layers
CREATE TABLE disc_ring_code_layers (
    id SERIAL PRIMARY KEY,
    entry_id INT NOT NULL REFERENCES disc_ring_code_entries(id) ON DELETE CASCADE,
    layer INT NOT NULL,
    mastering_code VARCHAR(255),
    mastering_sid VARCHAR(255),
    toolstamps TEXT NOT NULL DEFAULT '',
    mould_sids TEXT NOT NULL DEFAULT '',
    additional_moulds TEXT NOT NULL DEFAULT '',
    UNIQUE(entry_id, layer)
);

-- files / tracks
CREATE TABLE files (
    id SERIAL PRIMARY KEY,
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    track_number VARCHAR(16),
    size BIGINT NOT NULL,
    crc32 VARCHAR(8) NOT NULL,
    md5 VARCHAR(32) NOT NULL,
    sha1 VARCHAR(40) NOT NULL,
    UNIQUE(disc_id, track_number)
);
CREATE INDEX idx_files_disc ON files(disc_id);
CREATE UNIQUE INDEX files_disc_cue_unique ON files (disc_id) WHERE track_number IS NULL;

-- enums
CREATE TYPE user_role_enum AS ENUM ('User', 'User+', 'Moderator', 'Admin');
CREATE TYPE submission_type_enum AS ENUM ('Disc', 'Edit');
CREATE TYPE submission_status_enum AS ENUM ('Pending', 'Approved', 'Rejected', 'Legacy');

-- users
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    username VARCHAR(64) UNIQUE NOT NULL,
    email VARCHAR(255) UNIQUE NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    role user_role_enum NOT NULL DEFAULT 'User',
    email_verified BOOLEAN NOT NULL DEFAULT FALSE,
    email_verify_token VARCHAR(128),
    email_verify_expires_at TIMESTAMPTZ,
    password_reset_token VARCHAR(128),
    password_reset_expires_at TIMESTAMPTZ,
    failed_login_attempts INT NOT NULL DEFAULT 0,
    locked_until TIMESTAMPTZ,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at TIMESTAMPTZ
);

-- sessions
CREATE TABLE sessions (
    id VARCHAR(128) PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE CASCADE,
    ip_address VARCHAR(45),
    user_agent TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_active_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);

-- dumper credits
CREATE TABLE disc_dumpers (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    user_id INT NOT NULL REFERENCES users(id),
    PRIMARY KEY (disc_id, user_id)
);

-- disc submissions (queue + edit history)
CREATE TABLE disc_submissions (
    id SERIAL PRIMARY KEY,
    submission_type submission_type_enum NOT NULL,
    submitter_id INT NOT NULL REFERENCES users(id),
    submission_comment TEXT,
    target_disc_id INT REFERENCES discs(id),
    changes JSONB NOT NULL,
    dump_log TEXT,
    extra_upload_url VARCHAR(512),
    status submission_status_enum NOT NULL DEFAULT 'Pending',
    reviewer_id INT REFERENCES users(id),
    review_comment TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    reviewed_at TIMESTAMPTZ
);
CREATE INDEX idx_submissions_submitter ON disc_submissions(submitter_id);
CREATE INDEX idx_submissions_status ON disc_submissions(status);
CREATE INDEX idx_submissions_created ON disc_submissions(created_at DESC);
CREATE INDEX idx_submissions_target_disc ON disc_submissions(target_disc_id);

-- OIDC clients (for phpBB and MediaWiki)
CREATE TABLE oauth_clients (
    id SERIAL PRIMARY KEY,
    client_id VARCHAR(128) UNIQUE NOT NULL,
    client_secret VARCHAR(255) NOT NULL,
    redirect_uri TEXT NOT NULL,
    name VARCHAR(128) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
