ALTER TABLE IF EXISTS tags
ADD COLUMN IF NOT EXISTS sort_order SMALLINT DEFAULT NULL;

INSERT INTO tags (name)
VALUES ('Preproduction')
ON CONFLICT DO NOTHING;

UPDATE tags
SET sort_order = 1
WHERE LOWER(name) = LOWER('Preproduction');

INSERT INTO tags (name)
VALUES ('Bonus Discs')
ON CONFLICT DO NOTHING;

UPDATE tags
SET sort_order = 2
WHERE LOWER(name) = LOWER('Bonus Discs');

INSERT INTO tags (name)
VALUES ('Unlicensed')
ON CONFLICT DO NOTHING;

INSERT INTO disc_tags (disc_id, tag_id)
SELECT d.id, t.id
FROM discs d
JOIN categories c ON c.id = d.category_id
JOIN tags t ON LOWER(t.name) = LOWER('Preproduction')
WHERE c.name = 'Preproduction'
ON CONFLICT DO NOTHING;

INSERT INTO disc_tags (disc_id, tag_id)
SELECT d.id, t.id
FROM discs d
JOIN categories c ON c.id = d.category_id
JOIN tags t ON LOWER(t.name) = LOWER('Bonus Discs')
WHERE c.name = 'Bonus Discs'
ON CONFLICT DO NOTHING;

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

UPDATE discs
SET category_id = CASE
    WHEN system_code = 'AUDIO-CD' THEN (
        SELECT id
        FROM categories
        WHERE name = 'Audio'
    )
    WHEN system_code IN ('DVD-VIDEO', 'BD-VIDEO') THEN (
        SELECT id
        FROM categories
        WHERE name = 'Video'
    )
    WHEN system_code = 'ENHANCED-CD' THEN (
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
    WHERE name = 'Bonus Discs'
);

UPDATE discs
SET category_id = CASE
    WHEN EXISTS (
        SELECT 1
        FROM unnest(edition) AS edition_value
        WHERE edition_value ILIKE '%Demo%'
    ) THEN (
        SELECT id
        FROM categories
        WHERE name = 'Demos'
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

DELETE FROM categories
WHERE name IN ('Preproduction', 'Bonus Discs');
