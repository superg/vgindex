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
LOCK_FILE="$APP_DIR/.operation.lock"

if [[ -z "${IMAGE_TAG:-}" ]]; then
    echo "ERROR: IMAGE_TAG must be set" >&2
    exit 1
fi

if [[ ! -f "$ENV_FILE" ]]; then
    echo "ERROR: env file not found at $ENV_FILE" >&2
    exit 1
fi

exec 9>"$LOCK_FILE"
flock 9

cd "$APP_DIR"

echo "Validating Caddy configuration..."
docker compose --env-file "$ENV_FILE" run --rm --no-deps caddy \
    caddy adapt --config /etc/caddy/Caddyfile --validate >/dev/null

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

echo "Reloading Caddy configuration..."
RELOAD_OK=true
if ! docker compose --env-file "$ENV_FILE" exec -T caddy \
    caddy reload --config /etc/caddy/Caddyfile; then
    echo "ERROR: Caddy reload failed" >&2
    RELOAD_OK=false
fi

# ── Health checks (retry up to 60s) ────────────────────────────────
SERVICES=(app postgres caddy phpbb mediawiki)
MAX_ATTEMPTS=12
INTERVAL=5

echo "Waiting for services to become healthy..."

for attempt in $(seq 1 $MAX_ATTEMPTS); do
    HEALTHY=true
    for svc in "${SERVICES[@]}"; do
        if ! docker compose ps --format json "$svc" 2>/dev/null | grep -q '"running"'; then
            HEALTHY=false
            break
        fi
    done

    if [[ "$HEALTHY" == "true" ]]; then
        break
    fi

    echo "  attempt $attempt/$MAX_ATTEMPTS — not all services ready, retrying in ${INTERVAL}s..."
    sleep "$INTERVAL"
done

for svc in "${SERVICES[@]}"; do
    if docker compose ps --format json "$svc" 2>/dev/null | grep -q '"running"'; then
        echo "  ✓ $svc running"
    else
        echo "  ✗ $svc NOT running" >&2
        HEALTHY=false
    fi
done

if [[ "$RELOAD_OK" != "true" ]]; then
    HEALTHY=false
fi

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
