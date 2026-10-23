ALTER TABLE discs
    ADD COLUMN IF NOT EXISTS title_alt VARCHAR(512);

UPDATE discs
SET title_alt = title_foreign
WHERE title_foreign IS NOT NULL
  AND title_foreign ~ '^[\x20-\x7E]+$';

UPDATE discs
SET title_foreign = NULL
WHERE title_foreign IS NOT NULL
  AND title_foreign ~ '^[\x20-\x7E]+$';

CREATE INDEX IF NOT EXISTS idx_discs_title_alt_trgm
    ON discs USING GIN (LOWER(title_alt) gin_trgm_ops);
