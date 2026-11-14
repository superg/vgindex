ALTER TABLE systems
ADD COLUMN IF NOT EXISTS has_extras BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE systems
SET has_extras = TRUE,
    archives_dirty = TRUE
WHERE code = 'AUDIO-CD';

ALTER TABLE media_types
ADD COLUMN IF NOT EXISTS has_ss BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE media_types
SET has_ss = TRUE
WHERE code = 'xgd';
