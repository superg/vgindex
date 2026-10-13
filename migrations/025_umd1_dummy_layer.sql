ALTER TABLE media_types ADD COLUMN has_dummy_layer BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE media_types SET layer_count = 2, has_dummy_layer = TRUE WHERE code = 'umd1';
