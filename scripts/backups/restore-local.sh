#!/usr/bin/env bash
set -Eeuo pipefail

phase="initialization"

on_error() {
    local exit_code=$?
    echo "ERROR: restore failed during ${phase} (exit code ${exit_code})" >&2
    exit "$exit_code"
}
trap on_error ERR

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 <vgindex-backup-YYYYMMDDTHHMMSSZ.tar.gz>" >&2
    exit 2
fi

archive="$1"
if [[ ! -f "$archive" ]]; then
    echo "ERROR: backup archive not found: ${archive}" >&2
    exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
archive="$(realpath "$archive")"

cd "$repo_root"

phase="stopping local web services"
docker compose stop caddy app phpbb mediawiki

phase="restoring databases and content volumes"
docker compose --profile backup run --rm --no-deps -T \
    -v "${archive}:/input/backup.tar.gz:ro" \
    restore

phase="refreshing local phpBB configuration"
docker compose run --rm --no-deps -T \
    -e PHPBB_BOOTSTRAP_MODE=force \
    phpbb true

phase="starting local services"
docker compose up -d app phpbb mediawiki caddy

trap - ERR
echo "Local restore complete: ${archive}"
