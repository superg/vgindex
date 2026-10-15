ALTER TABLE discs
    ADD COLUMN IF NOT EXISTS disc_title_foreign VARCHAR(512);

ALTER TABLE systems
    ADD COLUMN IF NOT EXISTS has_disc_title_foreign BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_discs_disc_title_foreign_trgm
    ON discs USING GIN (LOWER(disc_title_foreign) gin_trgm_ops);

UPDATE systems
SET has_disc_title_foreign = TRUE
WHERE has_disc_title = TRUE;
