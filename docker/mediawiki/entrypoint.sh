#!/bin/bash
set -euo pipefail

required_vars=(
    MEDIAWIKI_DB_HOST MEDIAWIKI_DB_PORT MEDIAWIKI_DB_NAME
    MEDIAWIKI_DB_USER MEDIAWIKI_DB_PASSWORD
    MEDIAWIKI_ADMIN_USER MEDIAWIKI_ADMIN_PASSWORD
    SITE_DOMAIN HTTPS_PORT
    MEDIAWIKI_OIDC_CLIENT_ID MEDIAWIKI_OIDC_CLIENT_SECRET
    POSTGRES_DB
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

if [ "$HTTPS_PORT" = "443" ]; then
    MEDIAWIKI_SERVER="https://wiki.${SITE_DOMAIN}"
else
    MEDIAWIKI_SERVER="https://wiki.${SITE_DOMAIN}:${HTTPS_PORT}"
fi
WIKI_SITE_NAME="${SITE_DOMAIN} Wiki"

wiki_redirect_uri() {
    if [ "$HTTPS_PORT" = "443" ]; then
        printf "https://wiki.%s/Special:PluggableAuthLogin" "$SITE_DOMAIN"
    else
        printf "https://wiki.%s:%s/Special:PluggableAuthLogin" "$SITE_DOMAIN" "$HTTPS_PORT"
    fi
}

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

configure_oidc_client() {
    echo "MediaWiki entrypoint: syncing OIDC client in app DB..."

    local redirect_uri client_id_sql client_secret_sql redirect_uri_sql
    redirect_uri="$(wiki_redirect_uri)"
    client_id_sql="$(printf "%s" "$MEDIAWIKI_OIDC_CLIENT_ID" | sed "s/'/''/g")"
    client_secret_sql="$(printf "%s" "$MEDIAWIKI_OIDC_CLIENT_SECRET" | sed "s/'/''/g")"
    redirect_uri_sql="$(printf "%s" "$redirect_uri" | sed "s/'/''/g")"

    if ! PGPASSWORD="$MEDIAWIKI_DB_PASSWORD" psql \
        -h "$MEDIAWIKI_DB_HOST" \
        -p "$MEDIAWIKI_DB_PORT" \
        -U "$MEDIAWIKI_DB_USER" \
        -d "$POSTGRES_DB" \
        -v ON_ERROR_STOP=1 \
        -c "INSERT INTO oauth_clients (client_id, client_secret, redirect_uri, name) VALUES ('$client_id_sql', '$client_secret_sql', '$redirect_uri_sql', 'MediaWiki')
            ON CONFLICT (client_id) DO UPDATE SET
              client_secret = EXCLUDED.client_secret,
              redirect_uri = EXCLUDED.redirect_uri,
              name = EXCLUDED.name;" >/dev/null 2>&1; then
        echo "MediaWiki entrypoint: warning - could not upsert oauth client in app DB (${POSTGRES_DB})."
    fi
}

wait_for_db

if ! db_initialized; then
    echo "MediaWiki entrypoint: database not initialized, running install.php..."
    php maintenance/install.php \
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
    echo "MediaWiki entrypoint: install.php completed."
fi

cp /etc/mediawiki/LocalSettings.php /var/www/html/LocalSettings.php
chown www-data:www-data /var/www/html/LocalSettings.php

echo "MediaWiki entrypoint: running update.php..."
php maintenance/update.php --quick 2>&1 || true
echo "MediaWiki entrypoint: update.php completed."

configure_oidc_client

exec apache2-foreground
