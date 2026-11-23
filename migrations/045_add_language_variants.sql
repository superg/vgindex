-- Store the BCP 47 suffix separately; the UI composes it with language_code.
CREATE TABLE language_variants (
    code VARCHAR(4) NOT NULL,
    language_code CHAR(2) NOT NULL REFERENCES languages(code) ON UPDATE CASCADE ON DELETE CASCADE,
    name VARCHAR(32) NOT NULL,
    flag_code VARCHAR(16) NOT NULL,
    PRIMARY KEY (language_code, code),
    CHECK (code <> '' AND code = BTRIM(code))
);

CREATE TABLE disc_language_variants (
    disc_id INT NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    language_code CHAR(2) NOT NULL,
    variant_code VARCHAR(4) NOT NULL,
    PRIMARY KEY (disc_id, language_code, variant_code),
    FOREIGN KEY (language_code, variant_code)
        REFERENCES language_variants(language_code, code) ON UPDATE CASCADE
);

CREATE INDEX idx_disc_language_variants_variant_disc
ON disc_language_variants (language_code, variant_code, disc_id);

INSERT INTO language_variants (code, language_code, name, flag_code) VALUES
    ('Hans', 'zh', 'Chinese (Simplified)', 'cn'),
    ('Hant', 'zh', 'Chinese (Traditional)', 'tw'),
    ('BE', 'nl', 'Dutch (Belgium)', 'be'),
    ('NL', 'nl', 'Dutch (Netherlands)', 'nl'),
    ('BR', 'pt', 'Portuguese (Brazil)', 'br'),
    ('PT', 'pt', 'Portuguese (Portugal)', 'pt'),
    ('419', 'es', 'Spanish (Latin America)', 'xl'),
    ('ES', 'es', 'Spanish (Spain)', 'es'),
    ('BE', 'fr', 'French (Belgium)', 'be'),
    ('CA', 'fr', 'French (Canada)', 'ca'),
    ('FR', 'fr', 'French (France)', 'fr');
