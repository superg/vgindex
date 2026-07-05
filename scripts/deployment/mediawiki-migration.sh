#!/usr/bin/env bash
set -Eeuo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$script_dir/common.sh"

ACTION="${MEDIAWIKI_MIGRATION_ACTION:-}"
LOCK_FILE="$APP_DIR/.operation.lock"
STATE_FILE="$APP_DIR/.mediawiki_migration_state"
RELEASE_FILE="$APP_DIR/.last_release"
SCHEMA_MARKER="$APP_DIR/.mediawiki_schema_version"
BACKUP_DIR="${MEDIAWIKI_UPGRADE_BACKUP_DIR:-$APP_DIR/data/mediawiki-upgrade-backups}"
MIN_FREE_KB="${MEDIAWIKI_UPGRADE_MIN_FREE_KB:-1048576}"

STATUS=""
PREV_TAG=""
PREV_SCHEMA=""
TARGET_TAG=""
TARGET_SCHEMA=""
BACKUP_PATH=""
CONFIG_SNAPSHOT=""
MIGRATION_LOG=""
ROLLBACK_RUNNING=false
PREPARE_ACTIVE=false

usage() {
    echo "Usage: MEDIAWIKI_MIGRATION_ACTION={prepare|finalize|rollback} $0" >&2
}

state_get() {
    local key="$1"
    sed -n "s/^${key}=//p" "$STATE_FILE" | head -n 1
}

write_state() {
    local temporary
    temporary=$(mktemp "$APP_DIR/.mediawiki_migration_state.XXXXXX")
    {
        printf 'STATUS=%s\n' "$STATUS"
        printf 'PREV_TAG=%s\n' "$PREV_TAG"
        printf 'PREV_SCHEMA=%s\n' "$PREV_SCHEMA"
        printf 'TARGET_TAG=%s\n' "$TARGET_TAG"
        printf 'TARGET_SCHEMA=%s\n' "$TARGET_SCHEMA"
        printf 'BACKUP_PATH=%s\n' "$BACKUP_PATH"
        printf 'CONFIG_SNAPSHOT=%s\n' "$CONFIG_SNAPSHOT"
        printf 'MIGRATION_LOG=%s\n' "$MIGRATION_LOG"
    } >"$temporary"
    chmod 600 "$temporary"
    mv "$temporary" "$STATE_FILE"
}

load_state() {
    if [[ ! -f "$STATE_FILE" ]]; then
        echo "ERROR: no MediaWiki migration is in progress" >&2
        return 1
    fi
    STATUS=$(state_get STATUS)
    PREV_TAG=$(state_get PREV_TAG)
    PREV_SCHEMA=$(state_get PREV_SCHEMA)
    TARGET_TAG=$(state_get TARGET_TAG)
    TARGET_SCHEMA=$(state_get TARGET_SCHEMA)
    BACKUP_PATH=$(state_get BACKUP_PATH)
    CONFIG_SNAPSHOT=$(state_get CONFIG_SNAPSHOT)
    MIGRATION_LOG=$(state_get MIGRATION_LOG)

    if [[ ! "$PREV_TAG" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]] ||
       [[ ! "$TARGET_TAG" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]] ||
       [[ ! "$TARGET_SCHEMA" =~ ^[0-9]+\.[0-9]+$ ]] ||
       [[ ! "$PREV_SCHEMA" =~ ^(none|[0-9]+\.[0-9]+)$ ]]; then
        echo "ERROR: migration state contains invalid values" >&2
        return 1
    fi
}

archive_state() {
    local outcome="$1"
    local history_dir="$APP_DIR/.mediawiki-migration-history"
    mkdir -p "$history_dir"
    mv "$STATE_FILE" "$history_dir/$(date -u +%Y%m%dT%H%M%SZ)-${outcome}.env"
}

verify_backup() {
    local archive="$1"
    local checksum="${archive}.sha256"
    if [[ ! -f "$archive" || ! -f "$checksum" ]]; then
        echo "ERROR: MediaWiki upgrade archive or checksum is missing" >&2
        return 1
    fi
    (cd "$(dirname "$archive")" && sha256sum --check "$(basename "$checksum")")
    tar -xOzf "$archive" manifest.env | grep -qx 'SCOPE=mediawiki'
    tar -tzf "$archive" >/dev/null
}

drain_mediawiki_jobs() {
    local attempt count
    echo "Draining pending MediaWiki jobs..."
    for attempt in $(seq 1 20); do
        count=$(compose_active exec -T postgres \
            sh -c 'psql -U "$POSTGRES_USER" -d mediawiki -tAc "SELECT COUNT(*) FROM job;"')
        count=${count//[[:space:]]/}
        if [[ "$count" == "0" ]]; then
            echo "MediaWiki job queue is empty."
            return 0
        fi
        echo "  processing $count queued jobs (pass $attempt/20)..."
        compose_active exec -T mediawiki \
            php maintenance/run.php runJobs --maxjobs 1000
    done
    echo "ERROR: MediaWiki job queue did not drain" >&2
    return 1
}

restore_previous_schema_marker() {
    if [[ "$PREV_SCHEMA" == "none" ]]; then
        rm -f "$SCHEMA_MARKER"
    else
        printf '%s\n' "$PREV_SCHEMA" >"$SCHEMA_MARKER"
    fi
}

rollback_internal() {
    local reason="$1"
    if [[ "$ROLLBACK_RUNNING" == "true" ]]; then
        return 1
    fi
    ROLLBACK_RUNNING=true
    trap - ERR

    echo "Rolling back MediaWiki migration ($reason)..." >&2
    export IMAGE_TAG="$TARGET_TAG"
    compose_active stop mediawiki || true

    if [[ -n "$BACKUP_PATH" ]]; then
        verify_backup "$BACKUP_PATH"
        export RESTORE_SCOPE=mediawiki
        compose_active --profile backup run --rm --no-deps -T \
            -v "$BACKUP_PATH:/input/backup.tar.gz:ro" \
            restore
        unset RESTORE_SCOPE
    fi

    set_env_value IMAGE_TAG "$PREV_TAG"
    export IMAGE_TAG="$PREV_TAG"
    restore_runtime_config "$CONFIG_SNAPSHOT"
    restore_previous_schema_marker
    compose_active up -d --no-build --no-deps mediawiki
    reload_caddy || true
    wait_for_services mediawiki
    archive_state rolled-back
    PREPARE_ACTIVE=false
    echo "Rollback complete. The upgrade archive was retained at ${BACKUP_PATH:-not-created}."
}

on_prepare_error() {
    local exit_code=$?
    if [[ "$PREPARE_ACTIVE" == "true" && -f "$STATE_FILE" ]]; then
        load_state || true
        rollback_internal "prepare failed with exit code $exit_code" || true
    fi
    exit "$exit_code"
}

prepare_migration() {
    local current_free_kb backup_output backup_name

    if [[ -f "$STATE_FILE" ]]; then
        echo "ERROR: a MediaWiki migration is already in progress" >&2
        exit 1
    fi
    if [[ -z "${IMAGE_TAG:-}" ]] || [[ ! "$IMAGE_TAG" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
        echo "ERROR: prepare requires a valid IMAGE_TAG" >&2
        exit 1
    fi

    validate_release_payload
    TARGET_TAG="$IMAGE_TAG"
    TARGET_SCHEMA=$(schema_version_from_release)
    PREV_SCHEMA=$(installed_schema_version)
    if [[ "$TARGET_SCHEMA" == "$PREV_SCHEMA" ]]; then
        echo "ERROR: schema generation $TARGET_SCHEMA is already installed" >&2
        exit 1
    fi

    PREV_TAG=""
    if [[ -f "$RELEASE_FILE" ]]; then
        PREV_TAG=$(tr -d '[:space:]' <"$RELEASE_FILE")
    fi
    if [[ -z "$PREV_TAG" ]]; then
        PREV_TAG=$(sed -n 's/^IMAGE_TAG=//p' "$ENV_FILE" | tail -n 1)
    fi
    if [[ ! "$PREV_TAG" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
        echo "ERROR: no valid previous image tag is available for rollback" >&2
        exit 1
    fi

    mkdir -p "$BACKUP_DIR"
    chmod 700 "$BACKUP_DIR"
    current_free_kb=$(df -Pk "$BACKUP_DIR" | awk 'NR == 2 { print $4 }')
    if [[ ! "$current_free_kb" =~ ^[0-9]+$ ]] || (( current_free_kb < MIN_FREE_KB )); then
        echo "ERROR: less than ${MIN_FREE_KB} KiB is free for the upgrade snapshot" >&2
        exit 1
    fi

    echo "Ensuring rollback and target MediaWiki images are available..."
    export IMAGE_TAG="$PREV_TAG"
    compose_active pull --policy missing mediawiki
    export IMAGE_TAG="$TARGET_TAG"
    compose_release pull --policy missing mediawiki

    CONFIG_SNAPSHOT=$(snapshot_runtime_config "$PREV_TAG")
    STATUS=preparing
    MIGRATION_LOG="$BACKUP_DIR/migration-$(date -u +%Y%m%dT%H%M%SZ)-${TARGET_TAG}.log"
    write_state
    PREPARE_ACTIVE=true
    trap on_prepare_error ERR

    promote_release_payload
    compose_active run --rm --no-deps caddy \
        caddy adapt --config /etc/caddy/Caddyfile --validate >/dev/null
    reload_caddy

    export IMAGE_TAG="$PREV_TAG"
    drain_mediawiki_jobs
    compose_active stop mediawiki

    echo "Creating the final MediaWiki-only upgrade snapshot..."
    export BACKUP_SCOPE=mediawiki
    export SOURCE_IMAGE_TAG="$PREV_TAG"
    export SOURCE_SCHEMA_VERSION="$PREV_SCHEMA"
    export MEDIAWIKI_UPGRADE_BACKUP_DIR="$BACKUP_DIR"
    backup_output=$(compose_active --profile backup run --rm --no-deps -T backup)
    printf '%s\n' "$backup_output"
    unset BACKUP_SCOPE SOURCE_IMAGE_TAG SOURCE_SCHEMA_VERSION
    backup_name=$(printf '%s\n' "$backup_output" | sed -n 's|.*MediaWiki upgrade backup complete: /upgrade-backups/||p' | tail -n 1)
    if [[ -z "$backup_name" || "$backup_name" != "$(basename "$backup_name")" ]]; then
        echo "ERROR: could not determine the upgrade backup filename" >&2
        return 1
    fi
    BACKUP_PATH="$BACKUP_DIR/$backup_name"
    verify_backup "$BACKUP_PATH"
    STATUS=backed_up
    write_state

    set_env_value IMAGE_TAG "$TARGET_TAG"
    export IMAGE_TAG="$TARGET_TAG"
    echo "Running MediaWiki $TARGET_SCHEMA schema migration..."
    compose_active --profile migration run --rm --no-deps -T mediawiki-migrate \
        2>&1 | tee "$MIGRATION_LOG"

    # Start the migrated wiki writable so SSO can persist its session and user
    # mapping data during validation. A rollback from this point restores the
    # pre-migration snapshot and therefore discards any intervening wiki writes.
    export MEDIAWIKI_READ_ONLY_REASON=
    compose_active up -d --no-build --no-deps --force-recreate mediawiki
    unset MEDIAWIKI_READ_ONLY_REASON
    wait_for_services mediawiki

    STATUS=validating
    write_state
    PREPARE_ACTIVE=false
    trap - ERR
    cat <<EOF
MediaWiki $TARGET_SCHEMA is running writable and ready for validation.
Migration log: $MIGRATION_LOG
Rollback archive: $BACKUP_PATH
After browser and SSO checks, run the migration workflow with 'finalize'.
Run 'rollback' instead if validation fails.
WARNING: rollback restores the pre-migration snapshot and discards wiki edits
and uploads made after prepare completed.
EOF
}

finalize_migration() {
    load_state
    if [[ "$STATUS" != "validating" && "$STATUS" != "finalizing" ]]; then
        echo "ERROR: migration cannot be finalized from state $STATUS" >&2
        exit 1
    fi

    export IMAGE_TAG="$TARGET_TAG"
    set_env_value IMAGE_TAG "$TARGET_TAG"

    if [[ "$STATUS" == "validating" ]]; then
        wait_for_services mediawiki
        STATUS=finalizing
        write_state
    fi
    wait_for_services mediawiki

    printf '%s\n' "$TARGET_SCHEMA" >"$SCHEMA_MARKER"
    printf '%s\n' "$TARGET_TAG" >"$RELEASE_FILE"
    archive_state finalized
    echo "MediaWiki migration finalized at schema $TARGET_SCHEMA with image $TARGET_TAG."
    echo "Automatic database rollback is now disabled because the release is finalized."
}

rollback_migration() {
    load_state
    if [[ "$STATUS" == "finalizing" ]]; then
        echo "ERROR: rollback is disabled after finalization begins; use a forward fix" >&2
        exit 1
    fi
    rollback_internal "operator requested rollback from $STATUS"
}

if [[ ! -f "$ENV_FILE" ]]; then
    echo "ERROR: env file not found at $ENV_FILE" >&2
    exit 1
fi
if [[ ! "$MIN_FREE_KB" =~ ^[0-9]+$ ]]; then
    echo "ERROR: MEDIAWIKI_UPGRADE_MIN_FREE_KB must be numeric" >&2
    exit 1
fi

exec 9>"$LOCK_FILE"
flock 9

case "$ACTION" in
    prepare) prepare_migration ;;
    finalize) finalize_migration ;;
    rollback) rollback_migration ;;
    *) usage; exit 2 ;;
esac
