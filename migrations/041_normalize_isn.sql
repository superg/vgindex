UPDATE discs
SET internal_serial = CASE system_code
    WHEN 'PS5' THEN regexp_replace(internal_serial, '^PPSA([0-9]{5})$', 'PPSA-\1')
    WHEN 'PS4' THEN regexp_replace(internal_serial, '^CUSA([0-9]{5})$', 'CUSA-\1')
    WHEN 'PS3' THEN regexp_replace(internal_serial, '^([A-Z]{4})([0-9]{5})$', '\1-\2')
END
WHERE (system_code = 'PS5' AND internal_serial ~ '^PPSA[0-9]{5}$')
   OR (system_code = 'PS4' AND internal_serial ~ '^CUSA[0-9]{5}$')
   OR (system_code = 'PS3' AND internal_serial ~ '^[A-Z]{4}[0-9]{5}$');
