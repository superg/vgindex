#!/bin/bash
set -euo pipefail

required_vars=(
    MEDIAWIKI_DB_HOST MEDIAWIKI_DB_PORT MEDIAWIKI_DB_NAME
    MEDIAWIKI_DB_USER MEDIAWIKI_DB_PASSWORD
    MEDIAWIKI_ADMIN_USER MEDIAWIKI_ADMIN_PASSWORD
    MEDIAWIKI_PUBLIC_URL
    MEDIAWIKI_OIDC_CLIENT_ID MEDIAWIKI_OIDC_CLIENT_SECRET
)
missing=()
for var in "${required_vars[@]}"; do
    if [ -z "${!var+x}" ]; then
        missing+=("$var")
    fi
done
if [ ${#missing[@]} -gt 0 ]; then
    echo "MediaWiki entrypoint: ERROR - missing required environment variables: ${missing[*]}" >&2
    exit 1
fi

MEDIAWIKI_SERVER="${MEDIAWIKI_PUBLIC_URL%/}"
if [ -z "${MEDIAWIKI_SITE_NAME:-}" ]; then
    WIKI_SITE_NAME="$(php -r '$host = parse_url($argv[1], PHP_URL_HOST); echo ($host ?: "Wiki") . " Wiki";' "$MEDIAWIKI_SERVER")"
else
    WIKI_SITE_NAME="$MEDIAWIKI_SITE_NAME"
fi

wait_for_db() {
    echo "MediaWiki entrypoint: waiting for PostgreSQL at ${MEDIAWIKI_DB_HOST}:${MEDIAWIKI_DB_PORT}..."
    for i in $(seq 1 30); do
        if PGPASSWORD="$MEDIAWIKI_DB_PASSWORD" pg_isready \
            -h "$MEDIAWIKI_DB_HOST" \
            -p "$MEDIAWIKI_DB_PORT" \
            -U "$MEDIAWIKI_DB_USER" \
            -d "$MEDIAWIKI_DB_NAME" >/dev/null 2>&1; then
            echo "MediaWiki entrypoint: PostgreSQL is ready."
            return 0
        fi
        sleep 2
    done
    echo "MediaWiki entrypoint: ERROR - PostgreSQL did not become ready in time."
    exit 1
}

db_initialized() {
    PGPASSWORD="$MEDIAWIKI_DB_PASSWORD" psql \
        -h "$MEDIAWIKI_DB_HOST" \
        -p "$MEDIAWIKI_DB_PORT" \
        -U "$MEDIAWIKI_DB_USER" \
        -d "$MEDIAWIKI_DB_NAME" \
        -tAc "SELECT 1 FROM pg_tables WHERE tablename = 'page' LIMIT 1" 2>/dev/null | grep -q 1
}

wait_for_db

install_config() {
    cp /etc/mediawiki/LocalSettings.php /var/www/html/LocalSettings.php
    chown www-data:www-data /var/www/html/LocalSettings.php
}

install_wiki() {
    echo "MediaWiki entrypoint: database not initialized, running install.php..."
    php maintenance/run.php install \
        --dbtype postgres \
        --dbserver "$MEDIAWIKI_DB_HOST" \
        --dbport "$MEDIAWIKI_DB_PORT" \
        --dbschema public \
        --dbname "$MEDIAWIKI_DB_NAME" \
        --dbuser "$MEDIAWIKI_DB_USER" \
        --dbpass "$MEDIAWIKI_DB_PASSWORD" \
        --confpath /tmp \
        --scriptpath "" \
        --server "$MEDIAWIKI_SERVER" \
        --pass "$MEDIAWIKI_ADMIN_PASSWORD" \
        "$WIKI_SITE_NAME" \
        "$MEDIAWIKI_ADMIN_USER"
    echo "MediaWiki entrypoint: install completed."
}

initialize_schema() {
    echo "MediaWiki entrypoint: initializing extension schema..."
    php maintenance/run.php update --quick
    echo "MediaWiki entrypoint: extension schema initialized."
}

freshly_installed=false
if ! db_initialized; then
    install_wiki
    freshly_installed=true
fi

install_config
if [[ "$freshly_installed" == "true" ]]; then
    initialize_schema
fi

exec "$@"
