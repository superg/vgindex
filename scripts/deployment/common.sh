#!/usr/bin/env bash

set -euo pipefail

: "${APP_DIR:=/opt/app}"
: "${ENV_FILE:=$APP_DIR/.env}"
: "${RELEASE_DIR:=$APP_DIR}"
: "${COMPOSE_PROJECT_NAME:=$(basename "$APP_DIR")}"

compose_active() {
    docker compose \
        --project-name "$COMPOSE_PROJECT_NAME" \
        --env-file "$ENV_FILE" \
        --project-directory "$APP_DIR" \
        -f "$APP_DIR/docker-compose.yml" \
        "$@"
}

compose_release() {
    docker compose \
        --project-name "$COMPOSE_PROJECT_NAME" \
        --env-file "$ENV_FILE" \
        --project-directory "$RELEASE_DIR" \
        -f "$RELEASE_DIR/docker-compose.yml" \
        "$@"
}

validate_release_payload() {
    local required
    for required in \
        docker-compose.yml \
        Caddyfile \
        docker/mediawiki/LocalSettings.php \
        docker/mediawiki/schema-version \
        scripts/deployment/deploy.sh; do
        if [[ ! -f "$RELEASE_DIR/$required" ]]; then
            echo "ERROR: release payload is missing $required" >&2
            return 1
        fi
    done

    compose_release config --quiet
}

schema_version_from_release() {
    local version
    version=$(tr -d '[:space:]' <"$RELEASE_DIR/docker/mediawiki/schema-version")
    if [[ ! "$version" =~ ^[0-9]+\.[0-9]+$ ]]; then
        echo "ERROR: invalid MediaWiki schema version '$version'" >&2
        return 1
    fi
    printf '%s\n' "$version"
}

installed_schema_version() {
    local marker="$APP_DIR/.mediawiki_schema_version"
    if [[ -f "$marker" ]]; then
        tr -d '[:space:]' <"$marker"
    else
        printf 'none\n'
    fi
}

set_env_value() {
    local key="$1"
    local value="$2"
    local temporary

    temporary=$(mktemp "$APP_DIR/.env.XXXXXX")
    awk -v key="$key" -v value="$value" '
        BEGIN { updated = 0 }
        index($0, key "=") == 1 {
            if (!updated) {
                print key "=" value
                updated = 1
            }
            next
        }
        { print }
        END {
            if (!updated) {
                print key "=" value
            }
        }
    ' "$ENV_FILE" >"$temporary"
    chmod --reference="$ENV_FILE" "$temporary"
    mv "$temporary" "$ENV_FILE"
}

snapshot_runtime_config() {
    local label="$1"
    local snapshot_dir="$APP_DIR/.release-config"
    local snapshot="$snapshot_dir/$(date -u +%Y%m%dT%H%M%SZ)-${label}.tar.gz"

    mkdir -p "$snapshot_dir"
    tar -czf "$snapshot" -C "$APP_DIR" docker-compose.yml Caddyfile docker scripts
    printf '%s\n' "$snapshot"
}

restore_runtime_config() {
    local snapshot="$1"
    if [[ ! -f "$snapshot" ]]; then
        echo "ERROR: runtime configuration snapshot not found: $snapshot" >&2
        return 1
    fi
    tar -xzf "$snapshot" -C "$APP_DIR"
}

promote_release_payload() {
    if [[ "$(realpath "$RELEASE_DIR")" == "$(realpath "$APP_DIR")" ]]; then
        return 0
    fi

    cp "$RELEASE_DIR/docker-compose.yml" "$APP_DIR/docker-compose.yml"
    cp "$RELEASE_DIR/Caddyfile" "$APP_DIR/Caddyfile"
    mkdir -p "$APP_DIR/docker" "$APP_DIR/scripts"
    cp -a "$RELEASE_DIR/docker/." "$APP_DIR/docker/"
    cp -a "$RELEASE_DIR/scripts/." "$APP_DIR/scripts/"
}

service_container_id() {
    compose_active ps --quiet "$1"
}

service_is_ready() {
    local service="$1"
    local container_id status health

    container_id=$(service_container_id "$service")
    [[ -n "$container_id" ]] || return 1
    status=$(docker inspect --format '{{.State.Status}}' "$container_id")
    [[ "$status" == "running" ]] || return 1
    health=$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{end}}' "$container_id")
    [[ -z "$health" || "$health" == "healthy" ]]
}

wait_for_services() {
    local max_attempts="${MAX_ATTEMPTS:-18}"
    local interval="${INTERVAL:-5}"
    local attempt service ready
    local services=("$@")

    for attempt in $(seq 1 "$max_attempts"); do
        ready=true
        for service in "${services[@]}"; do
            if ! service_is_ready "$service"; then
                ready=false
                break
            fi
        done
        if [[ "$ready" == "true" ]]; then
            return 0
        fi
        echo "  attempt $attempt/$max_attempts - services not ready; retrying in ${interval}s..."
        sleep "$interval"
    done

    for service in "${services[@]}"; do
        if service_is_ready "$service"; then
            echo "  OK $service"
        else
            echo "  FAILED $service" >&2
        fi
    done
    return 1
}

reload_caddy() {
    compose_active exec -T caddy caddy reload --config /etc/caddy/Caddyfile
}
