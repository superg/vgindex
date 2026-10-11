INSERT INTO media_types
    (code, name, layer_count, pic, rom_extension) VALUES
    ('xgd', 'XGD', 2, FALSE, 'iso');

UPDATE systems
SET media_types = array_append(media_types, 'xgd')
WHERE code IN ('XBOX', 'XBOX360');

UPDATE discs
SET media_type_code = 'xgd'
WHERE system_code IN ('XBOX', 'XBOX360')
  AND media_type_code = 'dvd9'
  AND sector_ranges IS NOT NULL;
