#!/bin/sh
set -eu
set -o pipefail

umask 077

: "${POSTGRES_HOST:=postgres}"
: "${POSTGRES_PORT:=5432}"
: "${POSTGRES_USER:?POSTGRES_USER must be set}"
: "${POSTGRES_PASSWORD:?POSTGRES_PASSWORD must be set}"
: "${POSTGRES_DB:?POSTGRES_DB must be set}"
: "${BACKUP_SCOPE:=full}"

case "$BACKUP_SCOPE" in
    full|mediawiki) ;;
    *)
        echo "ERROR: BACKUP_SCOPE must be 'full' or 'mediawiki'" >&2
        exit 2
        ;;
esac

export PGPASSWORD="$POSTGRES_PASSWORD"

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
if [ "$BACKUP_SCOPE" = "mediawiki" ]; then
    archive_name="mediawiki-upgrade-${timestamp}.tar.gz"
    archive_dir="/upgrade-backups"
else
    archive_name="vgindex-backup-${timestamp}.tar.gz"
    archive_dir="/backups"
fi
archive_path="${archive_dir}/${archive_name}"
partial_path="${archive_dir}/.${archive_name}.partial"
work_dir="${archive_dir}/.work-${timestamp}-$$"
bundle_dir="$work_dir/bundle"

cleanup() {
    rm -rf "$work_dir" "$partial_path"
}
trap cleanup EXIT INT TERM

mkdir -p "$archive_dir" "$bundle_dir/databases"

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

if [ "$BACKUP_SCOPE" = "mediawiki" ]; then
    dump_database mediawiki "$bundle_dir/databases/mediawiki.dump"
    pg_restore --list "$bundle_dir/databases/mediawiki.dump" >/dev/null

    {
        printf 'FORMAT_VERSION=1\n'
        printf 'SCOPE=mediawiki\n'
        printf 'CREATED_AT=%s\n' "$timestamp"
        printf 'SOURCE_IMAGE_TAG=%s\n' "${SOURCE_IMAGE_TAG:-unknown}"
        printf 'SOURCE_SCHEMA_VERSION=%s\n' "${SOURCE_SCHEMA_VERSION:-unknown}"
    } >"$bundle_dir/manifest.env"

    ln -s /source/mediawiki_uploads "$bundle_dir/mediawiki_uploads"

    echo "Packaging ${archive_name}..."
    nice -n 10 tar -chf - \
        -C "$bundle_dir" manifest.env databases/mediawiki.dump mediawiki_uploads \
        | nice -n 10 gzip -1 >"$partial_path"
    tar -tzf "$partial_path" >/dev/null
    mv "$partial_path" "$archive_path"
    (cd "$archive_dir" && sha256sum "$archive_name" >"${archive_name}.sha256")
    archive_owner=$(stat -c '%u:%g' "$archive_dir")
    chown "$archive_owner" "$archive_path" "${archive_path}.sha256"
    chmod 600 "$archive_path" "${archive_path}.sha256"
    echo "MediaWiki upgrade backup complete: ${archive_path}"
    exit 0
fi

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
