-- Indexable normalization used by the advanced array-entry filters.  The
-- unit-separator keeps adjacent array entries from forming a false match;
-- callers still recheck individual entries to preserve exact semantics.
CREATE OR REPLACE FUNCTION compact_disc_array_search(TEXT[]) RETURNS TEXT
    LANGUAGE SQL IMMUTABLE PARALLEL SAFE STRICT
    AS $$
        SELECT LOWER(REGEXP_REPLACE(arr_to_str($1, CHR(31)), '[[:space:]]+', '', 'g'))
    $$;

CREATE OR REPLACE FUNCTION ringcode_layer_search_text(TEXT, TEXT) RETURNS TEXT
    LANGUAGE SQL IMMUTABLE PARALLEL SAFE
    AS $$
        SELECT LOWER(
            REGEXP_REPLACE(COALESCE($1, ''), '[[:blank:]]{2,}', CHR(9), 'g')
            || CHR(31) ||
            REGEXP_REPLACE(COALESCE($2, ''), '[[:blank:]]{2,}', CHR(9), 'g')
        )
    $$;

CREATE OR REPLACE FUNCTION disc_display_title_sort_key(
    disc_title_base TEXT,
    disc_number_value TEXT,
    disc_title_value TEXT,
    filename_suffix_value TEXT,
    include_disc_number BOOLEAN,
    include_disc_title BOOLEAN
) RETURNS TEXT
    LANGUAGE SQL IMMUTABLE PARALLEL SAFE
    AS $$
        SELECT LOWER(
            COALESCE($1, '') ||
            CASE
                WHEN $5 AND NULLIF($2, '') IS NOT NULL THEN ' (Disc ' || $2 || ')'
                ELSE ''
            END ||
            CASE
                WHEN $6 AND NULLIF($3, '') IS NOT NULL THEN ' (' || $3 || ')'
                ELSE ''
            END ||
            CASE
                WHEN NULLIF($4, '') IS NOT NULL THEN ' (' || $4 || ')'
                ELSE ''
            END
        )
    $$;

ALTER TABLE discs ADD COLUMN display_title_sort_key TEXT;

UPDATE discs d
SET display_title_sort_key = disc_display_title_sort_key(
    d.title,
    d.disc_number,
    d.disc_title,
    d.filename_suffix,
    s.has_disc_number,
    s.has_disc_title
)
FROM systems s
WHERE s.code = d.system_code;

ALTER TABLE discs ALTER COLUMN display_title_sort_key SET NOT NULL;

CREATE OR REPLACE FUNCTION set_disc_display_title_sort_key() RETURNS TRIGGER
    LANGUAGE plpgsql
    AS $$
    DECLARE
        include_disc_number BOOLEAN;
        include_disc_title BOOLEAN;
    BEGIN
        SELECT s.has_disc_number, s.has_disc_title
        INTO STRICT include_disc_number, include_disc_title
        FROM systems s
        WHERE s.code = NEW.system_code;

        NEW.display_title_sort_key := disc_display_title_sort_key(
            NEW.title,
            NEW.disc_number,
            NEW.disc_title,
            NEW.filename_suffix,
            include_disc_number,
            include_disc_title
        );
        RETURN NEW;
    END
    $$;

CREATE TRIGGER discs_display_title_sort_key
BEFORE INSERT OR UPDATE OF system_code, title, disc_number, disc_title, filename_suffix
ON discs
FOR EACH ROW EXECUTE FUNCTION set_disc_display_title_sort_key();

CREATE OR REPLACE FUNCTION refresh_system_disc_display_title_sort_keys() RETURNS TRIGGER
    LANGUAGE plpgsql
    AS $$
    BEGIN
        IF NEW.has_disc_number IS DISTINCT FROM OLD.has_disc_number
           OR NEW.has_disc_title IS DISTINCT FROM OLD.has_disc_title THEN
            UPDATE discs d
            SET display_title_sort_key = disc_display_title_sort_key(
                d.title,
                d.disc_number,
                d.disc_title,
                d.filename_suffix,
                NEW.has_disc_number,
                NEW.has_disc_title
            )
            WHERE d.system_code = NEW.code;
        END IF;
        RETURN NEW;
    END
    $$;

CREATE TRIGGER systems_disc_display_title_sort_keys
AFTER UPDATE OF has_disc_number, has_disc_title
ON systems
FOR EACH ROW EXECUTE FUNCTION refresh_system_disc_display_title_sort_keys();

CREATE INDEX idx_discs_active_display_title
ON discs (display_title_sort_key, id)
WHERE status <> 'Disabled';

CREATE INDEX idx_discs_active_system_title
ON discs (system_code, display_title_sort_key, id)
WHERE status <> 'Disabled';

CREATE INDEX idx_discs_active_media_title
ON discs (media_type_code, display_title_sort_key, id)
WHERE status <> 'Disabled';

CREATE INDEX idx_discs_active_category_title
ON discs (category_id, display_title_sort_key, id)
WHERE status <> 'Disabled';

CREATE INDEX idx_discs_status_title
ON discs (status, display_title_sort_key, id);

CREATE INDEX idx_discs_active_initial_title
ON discs (UPPER(LEFT(title, 1)), display_title_sort_key, id)
WHERE status <> 'Disabled';

CREATE INDEX idx_discs_active_error_count
ON discs (error_count)
WHERE status <> 'Disabled' AND error_count IS NOT NULL;

CREATE INDEX idx_discs_active_sort_version
ON discs (LOWER(version), display_title_sort_key, id)
WHERE status <> 'Disabled';

CREATE INDEX idx_discs_active_sort_edition
ON discs (LOWER(arr_to_str(edition, ', ')), display_title_sort_key, id)
WHERE status <> 'Disabled';

CREATE INDEX idx_discs_active_sort_serial
ON discs (LOWER(arr_to_str(serial, ', ')), display_title_sort_key, id)
WHERE status <> 'Disabled';

CREATE INDEX idx_discs_active_sort_status
ON discs (
    (CASE status
        WHEN 'Verified' THEN 1
        WHEN 'Unverified' THEN 2
        WHEN 'Questionable' THEN 3
        ELSE 4
    END),
    display_title_sort_key,
    id
)
WHERE status <> 'Disabled';

CREATE INDEX idx_disc_regions_region_disc
ON disc_regions (region_code, disc_id);

CREATE INDEX idx_disc_languages_language_disc
ON disc_languages (language_code, disc_id);

CREATE INDEX idx_disc_dumpers_user_disc
ON disc_dumpers (user_id, disc_id);

CREATE INDEX idx_files_indexed_disc
ON files (disc_id)
WHERE track_number IS NOT NULL;

CREATE INDEX idx_ring_entries_offset_disc
ON disc_ring_code_entries (offset_value, disc_id)
WHERE offset_value IS NOT NULL;

CREATE INDEX idx_ring_entries_extra_offset_disc
ON disc_ring_code_entries (offset_extra_value, disc_id)
WHERE offset_extra_value IS NOT NULL;

CREATE INDEX idx_discs_protection_trgm
ON discs USING GIN (LOWER(protection) gin_trgm_ops);

CREATE INDEX idx_ring_layers_search_trgm
ON disc_ring_code_layers USING GIN (
    ringcode_layer_search_text(mastering_code, mastering_sid) gin_trgm_ops
);

CREATE INDEX idx_discs_serial_compact_trgm
ON discs USING GIN (compact_disc_array_search(serial) gin_trgm_ops);

CREATE INDEX idx_discs_edition_compact_trgm
ON discs USING GIN (compact_disc_array_search(edition) gin_trgm_ops);

CREATE INDEX idx_discs_barcode_compact_trgm
ON discs USING GIN (compact_disc_array_search(barcode) gin_trgm_ops);

DROP INDEX idx_discs_serial_trgm;
DROP INDEX idx_discs_edition_trgm;

-- This generated full-text column was never queried.  Removing it avoids
-- maintaining a large GIN index and generated value on every disc write.
ALTER TABLE discs DROP COLUMN search_vector;
