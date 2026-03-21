-- Enums
CREATE TYPE disc_status_enum AS ENUM ('Verified', 'Good', 'Questionable', 'Bad');
CREATE TYPE user_role_enum AS ENUM ('User', 'UserPlus', 'Moderator', 'Admin');
CREATE TYPE submission_type_enum AS ENUM ('New Dump', 'Verification', 'Edit');
CREATE TYPE submission_status_enum AS ENUM ('Pending', 'Approved', 'Denied');

-- Lookup tables
CREATE TABLE media_types (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) UNIQUE NOT NULL,
    display_order INT NOT NULL DEFAULT 0
);

CREATE TABLE categories (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) UNIQUE NOT NULL,
    display_order INT NOT NULL DEFAULT 0
);

-- Regions / countries
CREATE TABLE regions (
    id SERIAL PRIMARY KEY,
    name VARCHAR(128) UNIQUE NOT NULL,
    flag_code VARCHAR(8) NOT NULL,
    display_order INT NOT NULL DEFAULT 0
);

-- Languages
CREATE TABLE languages (
    id SERIAL PRIMARY KEY,
    code VARCHAR(8) UNIQUE NOT NULL,
    name VARCHAR(128) NOT NULL,
    flag_code VARCHAR(8) NOT NULL,
    display_order INT NOT NULL DEFAULT 0
);

-- Title types
CREATE TABLE title_types (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) UNIQUE NOT NULL,
    display_order INT NOT NULL DEFAULT 0
);

-- Serial types
CREATE TABLE serial_types (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) UNIQUE NOT NULL,
    display_order INT NOT NULL DEFAULT 0
);

-- Systems / Platforms
CREATE TABLE systems (
    id SERIAL PRIMARY KEY,
    short_code VARCHAR(16) UNIQUE NOT NULL,
    full_name VARCHAR(255) NOT NULL,
    allowed_media INT[] NOT NULL DEFAULT '{}',
    has_date_field BOOLEAN NOT NULL DEFAULT FALSE,
    has_sbi BOOLEAN NOT NULL DEFAULT FALSE,
    has_pvd BOOLEAN NOT NULL DEFAULT FALSE,
    has_edc_field BOOLEAN NOT NULL DEFAULT FALSE,
    has_pic BOOLEAN NOT NULL DEFAULT FALSE,
    has_security_ranges BOOLEAN NOT NULL DEFAULT FALSE,
    has_header BOOLEAN NOT NULL DEFAULT FALSE,
    has_bca BOOLEAN NOT NULL DEFAULT FALSE,
    has_universal_hash BOOLEAN NOT NULL DEFAULT FALSE,
    display_order INT NOT NULL DEFAULT 0
);

-- Users
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

-- Sessions
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

-- Discs (main catalog)
CREATE TABLE discs (
    id SERIAL PRIMARY KEY,
    system_id INT NOT NULL REFERENCES systems(id),
    media_type_id INT NOT NULL REFERENCES media_types(id),
    title VARCHAR(512) NOT NULL,
    category_id INT NOT NULL REFERENCES categories(id) DEFAULT 1,
    version VARCHAR(255),
    edition VARCHAR(512),
    barcode VARCHAR(255),
    comments TEXT,
    filename_suffix VARCHAR(255),
    error_count INT,
    exe_date DATE,
    edc BOOLEAN,
    protection VARCHAR(255),
    sbi_data BYTEA,
    pvd_data BYTEA,
    pic_data BYTEA,
    security_ranges INT4RANGE[],
    header_data BYTEA,
    bca_data BYTEA,
    universal_hash VARCHAR(40),
    status disc_status_enum NOT NULL DEFAULT 'Good',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_discs_system ON discs(system_id);
CREATE INDEX idx_discs_status ON discs(status);
CREATE INDEX idx_discs_title ON discs(title);
CREATE INDEX idx_discs_created ON discs(created_at DESC);

-- Full-text search
ALTER TABLE discs ADD COLUMN search_vector tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(comments, '')), 'C') ||
        setweight(to_tsvector('english', coalesce(barcode, '')), 'B') ||
        setweight(to_tsvector('english', coalesce(version, '')), 'D') ||
        setweight(to_tsvector('english', coalesce(edition, '')), 'D')
    ) STORED;
CREATE INDEX idx_discs_search ON discs USING GIN(search_vector);

-- Disc <-> regions junction
CREATE TABLE disc_regions (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    region_id INT NOT NULL REFERENCES regions(id),
    PRIMARY KEY (disc_id, region_id)
);

-- Disc <-> languages junction
CREATE TABLE disc_languages (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    language_id INT NOT NULL REFERENCES languages(id),
    PRIMARY KEY (disc_id, language_id)
);

-- Alternative titles
CREATE TABLE disc_alt_titles (
    id SERIAL PRIMARY KEY,
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    title_type_id INT NOT NULL REFERENCES title_types(id),
    title TEXT NOT NULL
);
CREATE INDEX idx_disc_alt_titles_disc ON disc_alt_titles(disc_id);

-- Ring code entries
CREATE TABLE disc_ring_code_entries (
    id SERIAL PRIMARY KEY,
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE
);
CREATE INDEX idx_ring_entries_disc ON disc_ring_code_entries(disc_id);

-- Ring code layers
CREATE TABLE disc_ring_code_layers (
    id SERIAL PRIMARY KEY,
    entry_id INT NOT NULL REFERENCES disc_ring_code_entries(id) ON DELETE CASCADE,
    layer INT NOT NULL,
    mastering_code VARCHAR(255),
    mastering_sid VARCHAR(255),
    mould_sids TEXT[] NOT NULL DEFAULT '{}',
    toolstamps TEXT[] NOT NULL DEFAULT '{}',
    additional_moulds TEXT[] NOT NULL DEFAULT '{}',
    offset_value VARCHAR(16),
    sample_data_start VARCHAR(16),
    comment TEXT,
    UNIQUE(entry_id, layer)
);

-- Serials
CREATE TABLE disc_serials (
    id SERIAL PRIMARY KEY,
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    serial_type_id INT NOT NULL REFERENCES serial_types(id),
    serial VARCHAR(255) NOT NULL
);
CREATE INDEX idx_disc_serials_disc ON disc_serials(disc_id);

-- Files / tracks
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

-- Dumper credits
CREATE TABLE disc_dumpers (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    user_id INT NOT NULL REFERENCES users(id),
    PRIMARY KEY (disc_id, user_id)
);

-- Disc submissions (queue + edit history)
CREATE TABLE disc_submissions (
    id SERIAL PRIMARY KEY,
    submission_type submission_type_enum NOT NULL,
    submitter_id INT NOT NULL REFERENCES users(id),
    target_disc_id INT REFERENCES discs(id),
    data JSONB NOT NULL,
    dump_log TEXT,
    extra_files_path VARCHAR(512),
    status submission_status_enum NOT NULL DEFAULT 'Pending',
    reviewer_id INT REFERENCES users(id),
    review_comment TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    reviewed_at TIMESTAMPTZ
);
CREATE INDEX idx_submissions_submitter ON disc_submissions(submitter_id);
CREATE INDEX idx_submissions_status ON disc_submissions(status);
CREATE INDEX idx_submissions_created ON disc_submissions(created_at DESC);

-- OIDC clients (for phpBB and MediaWiki)
CREATE TABLE oauth_clients (
    id SERIAL PRIMARY KEY,
    client_id VARCHAR(128) UNIQUE NOT NULL,
    client_secret VARCHAR(255) NOT NULL,
    redirect_uri TEXT NOT NULL,
    name VARCHAR(128) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
