ALTER TABLE systems
ADD COLUMN IF NOT EXISTS has_extras BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE systems
SET has_extras = TRUE,
    archives_dirty = TRUE
WHERE code = 'AUDIO-CD';

ALTER TABLE media_types
ADD COLUMN IF NOT EXISTS has_ss BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE media_types
SET has_ss = TRUE
WHERE code = 'xgd';

-- Import existing lead-in and lead-out CD extras from comments.
WITH audio_cd_extras AS (
    SELECT DISTINCT
        d.id AS disc_id,
        CASE LOWER(rx.captures[1])
            WHEN 'leadin' THEN CASE WHEN track_count.normal_track_count >= 10 THEN '00' ELSE '0' END
            WHEN 'leadout' THEN CASE WHEN track_count.normal_track_count >= 10 THEN 'AA' ELSE 'A' END
        END AS file_type,
        rx.captures[2]::BIGINT AS size,
        LOWER(rx.captures[3]) AS crc32,
        LOWER(rx.captures[4]) AS md5,
        LOWER(rx.captures[5]) AS sha1
    FROM discs d
    CROSS JOIN LATERAL (
        SELECT COUNT(*) AS normal_track_count
        FROM files f
        WHERE f.disc_id = d.id
          AND f.track_number IS NOT NULL
    ) AS track_count
    CROSS JOIN LATERAL regexp_matches(
        d.comments,
        '<rom +name="(leadin|leadout)\.bin" +size="([0-9]+)" +crc="([0-9A-Fa-f]{8})" +md5="([0-9A-Fa-f]{32})" +sha1="([0-9A-Fa-f]{40})" */>',
        'gi'
    ) AS rx(captures)
    WHERE d.system_code = 'AUDIO-CD'
      AND d.comments IS NOT NULL
      AND (d.comments ILIKE '%leadin.bin%' OR d.comments ILIKE '%leadout.bin%')
)
INSERT INTO disc_extras (disc_id, file_type, size, crc32, md5, sha1)
SELECT e.disc_id, e.file_type, e.size, e.crc32, e.md5, e.sha1
FROM audio_cd_extras e
WHERE NOT EXISTS (
    SELECT 1
    FROM disc_extras existing
    WHERE existing.disc_id = e.disc_id
      AND existing.file_type = e.file_type
      AND existing.size = e.size
      AND existing.crc32 = e.crc32
      AND existing.md5 = e.md5
      AND existing.sha1 = e.sha1
);

-- Remove standalone imported entries.
UPDATE discs
SET comments = NULLIF(
    regexp_replace(
        comments,
        E'^ *(<(xmp|code|tt)> *)?<rom +name="(leadin|leadout)\\.bin" +size="[0-9]+" +crc="[0-9A-Fa-f]{8}" +md5="[0-9A-Fa-f]{32}" +sha1="[0-9A-Fa-f]{40}" */> *(</\\2>)? *(\\n|$)',
        '',
        'gim'
    ),
    ''
)
WHERE system_code = 'AUDIO-CD'
  AND comments IS NOT NULL
  AND (comments ILIKE '%leadin.bin%' OR comments ILIKE '%leadout.bin%');
