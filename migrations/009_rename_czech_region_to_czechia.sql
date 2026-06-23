-- Keep existing databases in sync with the updated region seed value.
UPDATE regions
SET name = 'Czechia'
WHERE code = 'cz';
