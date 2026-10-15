ALTER TABLE discs DROP COLUMN disc_id;

ALTER TABLE systems DROP COLUMN has_disc_id;

UPDATE discs
SET comments = btrim(
        regexp_replace(
            comments,
            '[\r\n]*[[:space:]]*<b>Additional Disc ID:?</b>:?[[:space:]]*00000000000000FF0002000.(?:x{8})[[:space:]]*[\r\n]*',
            E'\n',
            'gi'
        ),
        E' \t\r\n'
    )
WHERE system_code = 'PS3' AND media_type_code = 'bd25'
  AND comments ~* '<b>Additional Disc ID:?</b>:?[[:space:]]*00000000000000FF0002000.(?:x{8})';
