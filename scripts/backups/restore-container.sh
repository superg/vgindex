#!/bin/sh
set -eu

: "${POSTGRES_HOST:=postgres}"
: "${POSTGRES_PORT:=5432}"
: "${POSTGRES_USER:?POSTGRES_USER must be set}"
: "${POSTGRES_PASSWORD:?POSTGRES_PASSWORD must be set}"
: "${POSTGRES_DB:?POSTGRES_DB must be set}"

archive="/input/backup.tar.gz"
work_dir="/tmp/vgindex-restore"

if [ ! -f "$archive" ]; then
    echo "ERROR: restore archive is not mounted at ${archive}" >&2
    exit 1
fi

export PGPASSWORD="$POSTGRES_PASSWORD"

rm -rf "$work_dir"
mkdir -p "$work_dir"

echo "Extracting database dumps..."
tar -xzf "$archive" -C "$work_dir" databases

recreate_database() {
    database="$1"
    echo "Recreating PostgreSQL database ${database}..."
    dropdb \
        --host="$POSTGRES_HOST" \
        --port="$POSTGRES_PORT" \
        --username="$POSTGRES_USER" \
        --maintenance-db=postgres \
        --force \
        --if-exists \
        "$database"
    createdb \
        --host="$POSTGRES_HOST" \
        --port="$POSTGRES_PORT" \
        --username="$POSTGRES_USER" \
        --maintenance-db=postgres \
        --owner="$POSTGRES_USER" \
        "$database"
}

restore_database() {
    database="$1"
    dump="$2"
    recreate_database "$database"
    echo "Restoring PostgreSQL database ${database}..."

    for section in pre-data data post-data; do
        pg_restore \
            --host="$POSTGRES_HOST" \
            --port="$POSTGRES_PORT" \
            --username="$POSTGRES_USER" \
            --dbname="$database" \
            --no-owner \
            --no-acl \
            --exit-on-error \
            --section="$section" \
            "$dump"

        if [ "$database" = "$POSTGRES_DB" ] && [ "$section" = "pre-data" ]; then
            # Older dumps contain a SQL function that refers to arr_to_str
            # without its schema. pg_restore intentionally clears search_path,
            # so give that function a safe path before its indexes are rebuilt.
            psql \
                --host="$POSTGRES_HOST" \
                --port="$POSTGRES_PORT" \
                --username="$POSTGRES_USER" \
                --dbname="$database" \
                --set=ON_ERROR_STOP=1 <<'SQL'
DO $$
DECLARE
    compact_function REGPROCEDURE :=
        to_regprocedure('public.compact_disc_array_search(text[])');
BEGIN
    IF compact_function IS NOT NULL
       AND STRPOS(
           (SELECT prosrc FROM pg_proc WHERE oid = compact_function),
           'public.arr_to_str'
       ) = 0 THEN
        ALTER FUNCTION public.compact_disc_array_search(TEXT[])
            SET search_path = pg_catalog, public;
    END IF;
END
$$;
SQL
        fi
    done
}

restore_database "$POSTGRES_DB" "$work_dir/databases/app.dump"
restore_database phpbb "$work_dir/databases/phpbb.dump"
restore_database mediawiki "$work_dir/databases/mediawiki.dump"

echo "Replacing persisted content volumes..."
rm -rf \
    /restore/phpbb_files/* /restore/phpbb_files/.[!.]* /restore/phpbb_files/..?* \
    /restore/phpbb_avatars/* /restore/phpbb_avatars/.[!.]* /restore/phpbb_avatars/..?* \
    /restore/mediawiki_uploads/* /restore/mediawiki_uploads/.[!.]* /restore/mediawiki_uploads/..?* \
    /restore/archive_cache/* /restore/archive_cache/.[!.]* /restore/archive_cache/..?*

tar -xzf "$archive" -C /restore \
    phpbb_files phpbb_avatars mediawiki_uploads

echo "Restore data import complete."
