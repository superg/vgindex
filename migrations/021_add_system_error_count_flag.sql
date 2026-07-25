ALTER TABLE systems
ADD COLUMN has_error_count BOOLEAN NOT NULL DEFAULT TRUE;

UPDATE systems
SET has_error_count = FALSE
WHERE code = 'AUDIO-CD';
