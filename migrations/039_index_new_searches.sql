CREATE INDEX idx_discs_volume_label_trgm
ON discs USING GIN (LOWER(volume_label) gin_trgm_ops);

CREATE INDEX idx_discs_internal_serial_trgm
ON discs USING GIN (LOWER(internal_serial) gin_trgm_ops);
