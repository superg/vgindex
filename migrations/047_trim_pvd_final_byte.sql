-- Drop the unnecessary final byte (stored values are wrong anyway)
UPDATE discs
SET pvd = substring(pvd FROM 1 FOR 81)
WHERE octet_length(pvd) = 82;
