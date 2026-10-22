ALTER TABLE discs
    ADD COLUMN IF NOT EXISTS title_alt VARCHAR(512);

CREATE INDEX IF NOT EXISTS idx_discs_title_alt_trgm
    ON discs USING GIN (LOWER(title_alt) gin_trgm_ops);
