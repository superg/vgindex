CREATE INDEX idx_discs_contents_trgm
ON discs USING GIN (LOWER(contents) gin_trgm_ops);
