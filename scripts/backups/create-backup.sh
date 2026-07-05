#!/bin/sh
set -eu
set -o pipefail

umask 077

: "${POSTGRES_HOST:=postgres}"
: "${POSTGRES_PORT:=5432}"
: "${POSTGRES_USER:?POSTGRES_USER must be set}"
: "${POSTGRES_PASSWORD:?POSTGRES_PASSWORD must be set}"
: "${POSTGRES_DB:?POSTGRES_DB must be set}"

export PGPASSWORD="$POSTGRES_PASSWORD"

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
archive_name="vgindex-backup-${timestamp}.tar.gz"
archive_path="/backups/${archive_name}"
partial_path="/backups/.${archive_name}.partial"
work_dir="/backups/.work-${timestamp}-$$"
bundle_dir="$work_dir/bundle"

cleanup() {
    rm -rf "$work_dir" "$partial_path"
}
trap cleanup EXIT INT TERM

mkdir -p "$bundle_dir/databases"

dump_database() {
    database="$1"
    output="$2"
    echo "Backing up PostgreSQL database ${database}..."
    nice -n 10 pg_dump \
        --host="$POSTGRES_HOST" \
        --port="$POSTGRES_PORT" \
        --username="$POSTGRES_USER" \
        --format=custom \
        --compress=none \
        --file="$output" \
        "$database"
}

dump_database "$POSTGRES_DB" "$bundle_dir/databases/app.dump"
dump_database phpbb "$bundle_dir/databases/phpbb.dump"
dump_database mediawiki "$bundle_dir/databases/mediawiki.dump"

ln -s /source/phpbb_files "$bundle_dir/phpbb_files"
ln -s /source/phpbb_avatars "$bundle_dir/phpbb_avatars"
ln -s /source/mediawiki_uploads "$bundle_dir/mediawiki_uploads"

echo "Packaging ${archive_name}..."
nice -n 10 tar -chf - \
    -C "$bundle_dir" databases phpbb_files phpbb_avatars mediawiki_uploads \
    | nice -n 10 gzip -1 >"$partial_path"

mv "$partial_path" "$archive_path"

count=0
for backup in $(ls -1 /backups/vgindex-backup-*.tar.gz 2>/dev/null | sort -r); do
    count=$((count + 1))
    if [ "$count" -gt 7 ]; then
        rm -f "$backup"
    fi
done

echo "Backup complete: ${archive_path}"
