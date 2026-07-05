#!/usr/bin/env bash
set -Eeuo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$script_dir/common.sh"

RELEASE_FILE="$APP_DIR/.last_release"
LOCK_FILE="$APP_DIR/.operation.lock"

if [[ -z "${IMAGE_TAG:-}" ]]; then
    echo "ERROR: IMAGE_TAG must be set" >&2
    exit 1
fi
if [[ ! "$IMAGE_TAG" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
    echo "ERROR: IMAGE_TAG contains unsupported characters" >&2
    exit 1
fi
if [[ ! -f "$ENV_FILE" ]]; then
    echo "ERROR: env file not found at $ENV_FILE" >&2
    exit 1
fi

exec 9>"$LOCK_FILE"
flock 9

validate_release_payload
TARGET_SCHEMA=$(schema_version_from_release)
CURRENT_SCHEMA=$(installed_schema_version)

if [[ "$TARGET_SCHEMA" != "$CURRENT_SCHEMA" ]]; then
    cat >&2 <<EOF
ERROR: MediaWiki schema migration required ($CURRENT_SCHEMA -> $TARGET_SCHEMA).
Production was not changed. Run the manual MediaWiki migration workflow with
the 'prepare' action, validate the migrated wiki, and then run 'finalize'.
EOF
    exit 42
fi

PREV_TAG=""
if [[ -f "$RELEASE_FILE" ]]; then
    PREV_TAG=$(tr -d '[:space:]' <"$RELEASE_FILE")
fi
if [[ -z "$PREV_TAG" ]]; then
    PREV_TAG=$(sed -n 's/^IMAGE_TAG=//p' "$ENV_FILE" | tail -n 1)
fi
if [[ -n "$PREV_TAG" && ! "$PREV_TAG" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
    echo "ERROR: previous image tag contains unsupported characters" >&2
    exit 1
fi

echo "Deploying IMAGE_TAG=$IMAGE_TAG (previous: ${PREV_TAG:-none}, schema: $TARGET_SCHEMA)"
CONFIG_SNAPSHOT=$(snapshot_runtime_config "${PREV_TAG:-initial}")
ROLLBACK_NEEDED=true
ROLLBACK_RUNNING=false

rollback_deploy() {
    local exit_code="$1"
    if [[ "$ROLLBACK_NEEDED" != "true" || "$ROLLBACK_RUNNING" == "true" ]]; then
        return
    fi
    ROLLBACK_RUNNING=true
    trap - ERR
    echo "Deployment failed; restoring the previous release..." >&2
    restore_runtime_config "$CONFIG_SNAPSHOT"
    if [[ -n "$PREV_TAG" ]]; then
        set_env_value IMAGE_TAG "$PREV_TAG"
        export IMAGE_TAG="$PREV_TAG"
        compose_active pull app phpbb mediawiki || true
        compose_active up -d --no-build --remove-orphans || true
        reload_caddy || true
    fi
    echo "Previous release restoration attempted after exit code $exit_code." >&2
}

on_error() {
    local exit_code=$?
    rollback_deploy "$exit_code"
    exit "$exit_code"
}
trap on_error ERR

promote_release_payload
compose_active run --rm --no-deps caddy \
    caddy adapt --config /etc/caddy/Caddyfile --validate >/dev/null

set_env_value IMAGE_TAG "$IMAGE_TAG"
compose_active pull app phpbb mediawiki
compose_active up -d --no-build --remove-orphans
reload_caddy

echo "Waiting for services to become ready..."
wait_for_services app postgres caddy phpbb mediawiki

printf '%s\n' "$IMAGE_TAG" >"$RELEASE_FILE"
ROLLBACK_NEEDED=false
trap - ERR
echo "Deploy successful: $IMAGE_TAG"
