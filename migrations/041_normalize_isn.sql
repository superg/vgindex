-- Normalize ISN by adding hyphen
UPDATE discs
SET internal_serial = CASE system_code
    WHEN 'PS5' THEN regexp_replace(internal_serial, '^PPSA([0-9]{5})$', 'PPSA-\1')
    WHEN 'PS4' THEN regexp_replace(internal_serial, '^CUSA([0-9]{5})$', 'CUSA-\1')
    WHEN 'PS3' THEN regexp_replace(internal_serial, '^([A-Z]{4})([0-9]{5})$', '\1-\2')
END
WHERE (system_code = 'PS5' AND internal_serial ~ '^PPSA[0-9]{5}$')
   OR (system_code = 'PS4' AND internal_serial ~ '^CUSA[0-9]{5}$')
   OR (system_code = 'PS3' AND internal_serial ~ '^[A-Z]{4}[0-9]{5}$');

-- Migrate foreign disc titles from comments
WITH titles_from_comments AS (
    SELECT
        id,
        NULLIF(
            btrim(
                (regexp_match(
                    comments,
                    '(^|\n)(?:<b>)?Disc Title \(Non-Latin\):? *(?:</b>)? *:? *([^\n]*)',
                    'i'
                ))[2],
                E' \t\r'
            ),
            ''
        ) AS disc_title_foreign
    FROM discs
    WHERE comments ~* '(^|\n)(?:<b>)?Disc Title \(Non-Latin\):? *(?:</b>)? *:? *[^\n]*'
)
UPDATE discs AS d
SET disc_title_foreign = titles.disc_title_foreign,
    comments = NULLIF(
        btrim(
            regexp_replace(
                d.comments,
                '(^|\n)(?:<b>)?Disc Title \(Non-Latin\):? *(?:</b>)? *:? *[^\n]*(\n|$)',
                E'\\1',
                'i'
            ),
            E' \t\r\n'
        ),
        ''
    )
FROM titles_from_comments AS titles
WHERE d.id = titles.id
  AND titles.disc_title_foreign IS NOT NULL
  AND NULLIF(btrim(d.disc_title_foreign), '') IS NULL;
