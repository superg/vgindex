CREATE TABLE tags (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) NOT NULL CHECK (name <> '' AND name = BTRIM(name))
);

CREATE UNIQUE INDEX idx_tags_name_lower ON tags (LOWER(name));

CREATE TABLE disc_tags (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    tag_id INT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (disc_id, tag_id)
);

INSERT INTO tags (name)
VALUES
    ('Aftermarket'),
    ('Key Disc'),
    ('Unlicensed');
