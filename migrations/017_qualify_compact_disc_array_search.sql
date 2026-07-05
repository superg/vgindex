-- pg_restore clears search_path while rebuilding indexes.  Keep the helper
-- reference schema-qualified so restoring the compact array indexes does not
-- depend on the caller's search path.
CREATE OR REPLACE FUNCTION public.compact_disc_array_search(TEXT[]) RETURNS TEXT
    LANGUAGE SQL IMMUTABLE PARALLEL SAFE STRICT
    AS $$
        SELECT LOWER(REGEXP_REPLACE(public.arr_to_str($1, CHR(31)), '[[:space:]]+', '', 'g'))
    $$;

-- Older backups are restored with a temporary function-level search path.
ALTER FUNCTION public.compact_disc_array_search(TEXT[]) RESET search_path;
