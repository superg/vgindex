#!/usr/bin/env bash
set -euo pipefail

# Idempotent deploy script — run on the production server.
# Called by the CD workflow over SSH.
#
# Required env vars (set by caller):
#   IMAGE_TAG   — Docker image tag to deploy (e.g. sha-abc1234)
#
# Optional env vars:
#   APP_DIR     — path to the compose project (default: /opt/app)
#   ENV_FILE    — path to runtime env file    (default: $APP_DIR/.env)

APP_DIR="${APP_DIR:-/opt/app}"
ENV_FILE="${ENV_FILE:-$APP_DIR/.env}"
RELEASE_FILE="$APP_DIR/.last_release"

if [[ -z "${IMAGE_TAG:-}" ]]; then
    echo "ERROR: IMAGE_TAG must be set" >&2
    exit 1
fi

if [[ ! -f "$ENV_FILE" ]]; then
    echo "ERROR: env file not found at $ENV_FILE" >&2
    exit 1
fi

cd "$APP_DIR"

# ── Save previous tag for rollback ──────────────────────────────────
PREV_TAG=""
if [[ -f "$RELEASE_FILE" ]]; then
    PREV_TAG=$(cat "$RELEASE_FILE")
fi

echo "Deploying IMAGE_TAG=$IMAGE_TAG (previous: ${PREV_TAG:-none})"

# ── Write new tag into env and pull ─────────────────────────────────
# Update or append IMAGE_TAG in the env file
if grep -q "^IMAGE_TAG=" "$ENV_FILE"; then
    sed -i "s|^IMAGE_TAG=.*|IMAGE_TAG=$IMAGE_TAG|" "$ENV_FILE"
else
    echo "IMAGE_TAG=$IMAGE_TAG" >> "$ENV_FILE"
fi

docker compose --env-file "$ENV_FILE" pull app phpbb mediawiki

# ── Bring up new containers ─────────────────────────────────────────
docker compose --env-file "$ENV_FILE" up -d --no-build --remove-orphans

# ── Health checks ───────────────────────────────────────────────────
echo "Waiting for services to become healthy..."
sleep 5

HEALTHY=true

check_container() {
    local svc="$1"
    if docker compose ps --format json "$svc" 2>/dev/null | grep -q '"running"'; then
        echo "  ✓ $svc running"
    else
        echo "  ✗ $svc NOT running" >&2
        HEALTHY=false
    fi
}

check_container app
check_container postgres
check_container caddy
check_container phpbb
check_container mediawiki

if [[ "$HEALTHY" == "true" ]]; then
    echo "$IMAGE_TAG" > "$RELEASE_FILE"
    echo "Deploy successful: $IMAGE_TAG"
else
    echo "ERROR: Health checks failed" >&2
    if [[ -n "$PREV_TAG" ]]; then
        echo "Rolling back to $PREV_TAG ..."
        sed -i "s|^IMAGE_TAG=.*|IMAGE_TAG=$PREV_TAG|" "$ENV_FILE"
        docker compose --env-file "$ENV_FILE" pull app phpbb mediawiki
        docker compose --env-file "$ENV_FILE" up -d --no-build --remove-orphans
        echo "Rolled back to $PREV_TAG"
    fi
    exit 1
fi
