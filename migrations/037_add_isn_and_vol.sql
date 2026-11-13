ALTER TABLE discs
ADD COLUMN IF NOT EXISTS internal_serial TEXT,
ADD COLUMN IF NOT EXISTS volume_label TEXT;

UPDATE discs
SET internal_serial = substring(comments FROM '(?:^|\n)\[T:ISN\] ?([^\n]*)'),
    volume_label = substring(comments FROM '(?:^|\n)\[T:VOL\] ?([^\n]*)'),
    comments = regexp_replace(
        regexp_replace(comments, '(^|\n)\[T:ISN\][^\n]*(\n|$)', E'\\1'),
        '(^|\n)\[T:VOL\][^\n]*(\n|$)', E'\\1'
    )
WHERE comments LIKE '%[T:ISN]%'
   OR comments LIKE '%[T:VOL]%';
