-- Add sort order for tags, NULL means no category override
ALTER TABLE IF EXISTS tags
ADD COLUMN IF NOT EXISTS sort_order SMALLINT DEFAULT NULL;

-- Add 7 new tags
INSERT INTO tags (name)
VALUES
    ('Preproduction'),
    ('Bonus Discs'),
    ('Mini (8cm)'),
    ('PC/Mac Hybrid'),
    ('Recordable Disc'),
    ('Stub/Key Disc'),
    ('Unlicensed')
ON CONFLICT DO NOTHING;

-- Retain Preproduction as category in datfile, with highest priority
UPDATE tags
SET sort_order = 1
WHERE LOWER(name) = LOWER('Preproduction');

-- Retain Bonus Discs as category in datfile, with 2nd highest priority
UPDATE tags
SET sort_order = 2
WHERE LOWER(name) = LOWER('Bonus Discs');

-- Migrate Preproduction categories to tags
INSERT INTO disc_tags (disc_id, tag_id)
SELECT d.id, t.id
FROM discs d
JOIN categories c ON c.id = d.category_id
JOIN tags t ON LOWER(t.name) = LOWER('Preproduction')
WHERE c.name = 'Preproduction'
ON CONFLICT DO NOTHING;

-- Migrate Bonus Disc categories to tags
INSERT INTO disc_tags (disc_id, tag_id)
SELECT d.id, t.id
FROM discs d
JOIN categories c ON c.id = d.category_id
JOIN tags t ON LOWER(t.name) = LOWER('Bonus Discs')
WHERE c.name = 'Bonus Discs'
ON CONFLICT DO NOTHING;

-- Set Unlicensed tag for unlicensed-edition discs
INSERT INTO disc_tags (disc_id, tag_id)
SELECT d.id, t.id
FROM discs d
JOIN tags t ON LOWER(t.name) = LOWER('Unlicensed')
WHERE EXISTS (
    SELECT 1
    FROM unnest(d.edition) AS edition_value
    WHERE edition_value ILIKE '%Unlicensed%'
)
ON CONFLICT DO NOTHING;

-- Set Mini (8cm) tag for discs with it mentioned in comments
INSERT INTO disc_tags (disc_id, tag_id)
SELECT d.id, t.id
FROM discs d
JOIN tags t ON LOWER(t.name) = LOWER('Mini (8cm)')
WHERE (
    d.comments ILIKE '%8cm%'
    OR d.comments ILIKE '%Mini CD%'
    OR d.comments ILIKE '%Mini DVD%'
)
AND d.id NOT IN (96389, 12321, 1904, 86237, 48746)
AND NOT (
    d.system_code = 'PS2'
    AND d.title ILIKE '%Oretachi Geesen Zoku%'
)
ON CONFLICT DO NOTHING;

-- Set PC/Mac Hybrid tag for discs with it mentioned in comments
INSERT INTO disc_tags (disc_id, tag_id)
SELECT d.id, t.id
FROM discs d
JOIN tags t ON LOWER(t.name) = LOWER('PC/Mac Hybrid')
WHERE d.comments ILIKE '%PC/Mac Hybrid%'
ON CONFLICT DO NOTHING;

-- Remove standalone PC/Mac Hybrid tags from comments
UPDATE discs
SET comments = regexp_replace(
    comments,
    '^PC/Mac Hybrid[.]?(\r?\n|$)',
    '',
    'gn'
)
WHERE comments ~ '(^|\n)PC/Mac Hybrid[.]?(\r?\n|$)';

-- Set Recordable Disc tag for CD-R/DVD-R
INSERT INTO disc_tags (disc_id, tag_id)
SELECT d.id, t.id
FROM discs d
JOIN tags t ON LOWER(t.name) = LOWER('Recordable Disc')
WHERE (
    (d.comments ILIKE '%CD-R%' AND d.comments NOT ILIKE '%CD-ROM%')
    OR (d.comments ILIKE '%DVD-R%' AND d.comments NOT ILIKE '%DVD-ROM%')
    OR d.comments ILIKE '%DVD+R%'
    OR d.comments ILIKE '%BD-R%'
    OR d.comments ILIKE '%Recordable%'
)
AND d.comments NOT ILIKE '%Pressed%'
ON CONFLICT DO NOTHING;

-- Reset category for bonus discs
UPDATE discs
SET category_id = CASE
    WHEN system_code IN ('AUDIO-CD', 'MP3') THEN (
        SELECT id
        FROM categories
        WHERE name = 'Audio'
    )
    WHEN system_code IN ('DVD-VIDEO', 'BD-VIDEO', 'HVNC') THEN (
        SELECT id
        FROM categories
        WHERE name = 'Video'
    )
    ELSE (
        SELECT id
        FROM categories
        WHERE name = 'Multimedia'
    )
END
WHERE category_id = (
    SELECT id
    FROM categories
    WHERE name = 'Bonus Discs'
);

-- Reset category for Preproduction discs
UPDATE discs
SET category_id = CASE
    WHEN filename_suffix ILIKE '%Demo%' THEN (
        SELECT id
        FROM categories
        WHERE name = 'Demos'
    )
    WHEN system_code IN ('AUDIO-CD', 'MP3') THEN (
        SELECT id
        FROM categories
        WHERE name = 'Audio'
    )
    WHEN system_code IN ('MAC', 'PHOTO-CD') THEN (
        SELECT id
        FROM categories
        WHERE name = 'Multimedia'
    )
    ELSE (
        SELECT id
        FROM categories
        WHERE name = 'Games'
    )
END
WHERE category_id = (
    SELECT id
    FROM categories
    WHERE name = 'Preproduction'
);

-- Delete Preproduction and Bonus Discs categories
DELETE FROM categories
WHERE name IN ('Preproduction', 'Bonus Discs');
