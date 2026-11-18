-- Remove leading/trailing whitespace on ringcodes
UPDATE disc_ring_code_layers
SET mastering_code = BTRIM(mastering_code, E' \t\n\r\f')
WHERE mastering_code IS NOT NULL AND mastering_code ~ '^[ \t\n\r\f]|[ \t\n\r\f]$';

UPDATE disc_ring_code_layers
SET mastering_sid = BTRIM(mastering_sid, E' \t\n\r\f')
WHERE mastering_sid IS NOT NULL AND mastering_sid ~ '^[ \t\n\r\f]|[ \t\n\r\f]$';

UPDATE disc_ring_code_layers
SET mould_sids = BTRIM(mould_sids, E' \t\n\r\f')
WHERE mould_sids != '' AND mould_sids ~ '^[ \t\n\r\f]|[ \t\n\r\f]$';

UPDATE disc_ring_code_layers
SET toolstamps = BTRIM(toolstamps, E' \t\n\r\f')
WHERE toolstamps != '' AND toolstamps ~ '^[ \t\n\r\f]|[ \t\n\r\f]$';

UPDATE disc_ring_code_layers
SET additional_moulds = BTRIM(additional_moulds, E' \t\n\r\f')
WHERE additional_moulds != '' AND additional_moulds ~ '^[ \t\n\r\f]|[ \t\n\r\f]$';

-- Update pseudotag definitions
UPDATE discs
SET contents = regexp_replace(
    contents,
    '(^|\n)(?:<b>)?Games:?(?:</b>)?:?[ \t]*(?=\r?\n|$)',
    E'\\1[T:GA]',
    'g'
)
WHERE contents ~ '(^|\n)(?:<b>)?Games:?(?:</b>)?:?[ \t]*(?=\r?\n|$)';

UPDATE discs
SET contents = regexp_replace(
    contents,
    '(^|\n)(?:<b>)?Applications:?(?:</b>)?:?[ \t]*(?=\r?\n|$)',
    E'\\1[T:APP]',
    'g'
)
WHERE contents ~ '(^|\n)(?:<b>)?Applications:?(?:</b>)?:?[ \t]*(?=\r?\n|$)';

UPDATE discs
SET contents = regexp_replace(
    contents,
    '(^|\n)(?:<b>)?Steam Depot ID \(\.sis/\.sim/\.sid\):?(?:</b>)?:?[ \t]*(?=\r?\n|$)',
    E'\\1[T:SIM]',
    'g'
)
WHERE contents ~ '(^|\n)(?:<b>)?Steam Depot ID \(\.sis/\.sim/\.sid\):?(?:</b>)?:?[ \t]*(?=\r?\n|$)';

UPDATE discs
SET contents = regexp_replace(
    contents,
    '(^|\n)(?:<b>)?Steam Depot ID \(\.sis/\.csm/\.csd\):?(?:</b>)?:?[ \t]*(?=\r?\n|$)',
    E'\\1[T:CSM]',
    'g'
)
WHERE contents ~ '(^|\n)(?:<b>)?Steam Depot ID \(\.sis/\.csm/\.csd\):?(?:</b>)?:?[ \t]*(?=\r?\n|$)';

UPDATE discs
SET comments = regexp_replace(
    comments,
    '(^|\n)\[T:PT2\](?=\r?\n|$)',
    E'\\1<b>Postgap type</b>: Form 2',
    'g'
)
WHERE comments ~ '(^|\n)\[T:PT2\](?=\r?\n|$)';

UPDATE discs
SET comments = regexp_replace(
    comments,
    '(^|\n)\[T:VFC\](?=\r?\n|$)',
    E'\\1[T:BBFC]',
    'g'
)
WHERE comments ~ '(^|\n)\[T:VFC\](?=\r?\n|$)';

CREATE TABLE IF NOT EXISTS alternative_disc_title_types (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) NOT NULL UNIQUE
);

INSERT INTO alternative_disc_title_types (name) VALUES
    ('Internal'),
    ('Transliterated')
ON CONFLICT (name) DO NOTHING;

CREATE TABLE IF NOT EXISTS disc_alternative_titles (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    title_type_id INT NOT NULL REFERENCES alternative_disc_title_types(id),
    title VARCHAR(512) NOT NULL,
    PRIMARY KEY (disc_id, title_type_id)
);
