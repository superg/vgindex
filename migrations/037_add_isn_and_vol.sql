ALTER TABLE systems
ADD COLUMN IF NOT EXISTS has_internal_serial BOOLEAN NOT NULL DEFAULT FALSE,
ADD COLUMN IF NOT EXISTS has_volume_label BOOLEAN NOT NULL DEFAULT TRUE;

UPDATE systems
SET has_internal_serial = FALSE;

UPDATE systems
SET has_internal_serial = TRUE
WHERE code IN (
    'GC', 'WII', 'WIIU', 'PSX', 'PS2', 'PS3', 'PS4', 'PS5', 'PSP',
    'MCD', 'SS', 'DC', 'TRF', 'CHIHIRO', 'VFLASH', 'NAOMI', 'BD-VIDEO',
    'FMT', 'NAOMI2', 'PHOTO-CD', 'LINDBERGH', 'KS573', 'SRE', 'KP2'
);

UPDATE systems
SET has_volume_label = FALSE
WHERE code IN ('AUDIO-CD', 'AJCD', 'HVN', 'HVNC', 'HVNJR', 'HVNXP', 'ION');

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
