CREATE TABLE disc_extras (
    id SERIAL PRIMARY KEY,
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    file_type VARCHAR(16) NOT NULL,
    size BIGINT NOT NULL,
    crc32 VARCHAR(8) NOT NULL,
    md5 VARCHAR(32) NOT NULL,
    sha1 VARCHAR(40) NOT NULL
);

CREATE INDEX disc_extras_disc_id_idx ON disc_extras (disc_id);

UPDATE systems
SET archives_dirty = TRUE
WHERE 'xgd' = ANY(media_types);
