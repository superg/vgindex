--New one-to-many table for Nintendo BCA mastering data
CREATE TABLE IF NOT EXISTS disc_bca_mastering (
    id SERIAL PRIMARY KEY,
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    data BYTEA NOT NULL CHECK (octet_length(data) = 16)
);
CREATE INDEX IF NOT EXISTS idx_disc_bca_mastering_disc ON disc_bca_mastering(disc_id);

-- Move existing BCA data to new table
INSERT INTO disc_bca_mastering (disc_id, data)
SELECT id, substring(bca FROM octet_length(bca) - 15)
FROM discs
WHERE bca IS NOT NULL
  AND octet_length(bca) >= 16
ORDER BY id;

-- Clear old BCA data
UPDATE discs
SET bca = NULL
WHERE bca IS NOT NULL;

-- Function to extract BCA data from comments
CREATE OR REPLACE FUNCTION extract_all_bca_from_comments(p_comments TEXT)
RETURNS TABLE(bca_data BYTEA) AS $$
DECLARE
    v_bca_section TEXT;
    v_lines TEXT[];
    v_line TEXT;
    v_hex_cleaned TEXT;
    i INT;
BEGIN
    v_bca_section := (regexp_matches(
        p_comments,
        '(?:^|\n)(?:(?:[-=_|<>`/\s]*\n)?)(?:<[bu]>)?Additional BCA(?: [Dd]ata)?:?(?:<\/[bu]>)?:?(?:<(?:tt|code)>)?:?(?:\n(?:(?:[-=_|<>`/\s]*\n|(?:\s*[0-9A-Fa-f]){32}\s*(?:\n|$))+))(?:<\/(?:tt|code)>)?',
        's'
    ))[1];
    
    IF v_bca_section IS NULL THEN
        RETURN;
    END IF;
    
    v_lines := regexp_split_to_array(v_bca_section, E'\n');
    
    FOR i IN 1..array_length(v_lines, 1) LOOP
        v_line := v_lines[i];
        v_hex_cleaned := upper(regexp_replace(v_line, '\s', '', 'g'));
        
        IF v_hex_cleaned ~ '^[0-9A-F]{32}$' AND v_hex_cleaned != '00000000000000000000000000000000' THEN
            bca_data := decode(v_hex_cleaned, 'hex');
            RETURN NEXT;
        END IF;
    END LOOP;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- Copy Additional BCA data extracted from comments into the new table
WITH bca_from_comments AS (
    SELECT 
        d.id,
        bca.bca_data
    FROM discs d
    CROSS JOIN LATERAL extract_all_bca_from_comments(d.comments) AS bca(bca_data)
    WHERE d.system_code IN ('GC', 'WII')
      AND d.comments IS NOT NULL
      AND LOWER(d.comments) LIKE '%additional bca%'
)
INSERT INTO disc_bca_mastering (disc_id, data)
SELECT id, bca_data
FROM bca_from_comments
WHERE bca_data IS NOT NULL;

-- Remove Additional BCA section from comments
UPDATE discs
SET comments = regexp_replace(
    comments,
    '(^|\n)(?:(?:[-=_|<>`/\s]*\n)?)(?:<[bu]>)?Additional BCA(?: [Dd]ata)?:?(?:<\/[bu]>)?:?(?:<(?:tt|code)>)?:?(?:\n(?:(?:[-=_|<>`/\s]*\n|(?:\s*[0-9A-Fa-f]){32}\s*(?:\n|$))+))(?:<\/(?:tt|code)>)?',
    E'\\1',
    'g'
)
WHERE system_code IN ('GC', 'WII')
  AND comments IS NOT NULL
  AND LOWER(comments) LIKE '%additional bca%';

DROP FUNCTION IF EXISTS extract_all_bca_from_comments(TEXT);
