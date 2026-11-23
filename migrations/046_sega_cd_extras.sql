-- Support CD+G files for MCD and SS
UPDATE systems
SET has_extras = TRUE,
    archives_dirty = TRUE
WHERE code IN ('MCD', 'SS');
