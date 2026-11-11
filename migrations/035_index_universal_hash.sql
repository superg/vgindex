CREATE INDEX idx_discs_universal_hash
ON discs (universal_hash)
WHERE universal_hash IS NOT NULL;
