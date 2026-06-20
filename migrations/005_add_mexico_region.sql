-- Add Mexico to the region reference table between Lithuania and Netherlands.
BEGIN;

UPDATE regions
SET sort_order = sort_order + 1
WHERE sort_order >= 34
  AND code <> 'mx'
  AND NOT EXISTS (SELECT 1 FROM regions WHERE code = 'mx');

INSERT INTO regions (code, name, flag_code, sort_order)
VALUES ('mx', 'Mexico', 'mx', 34)
ON CONFLICT (code) DO UPDATE
SET name = EXCLUDED.name,
    flag_code = EXCLUDED.flag_code,
    sort_order = EXCLUDED.sort_order;

COMMIT;
