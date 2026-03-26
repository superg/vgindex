#!/bin/bash
set -euo pipefail

required_vars=(
    PHPBB_DB_HOST PHPBB_DB_PORT PHPBB_DB_NAME PHPBB_DB_USER PHPBB_DB_PASSWORD
    PHPBB_TABLE_PREFIX PHPBB_ADMIN_USER PHPBB_ADMIN_PASSWORD PHPBB_ADMIN_EMAIL
    PHPBB_BOARD_NAME PHPBB_BOARD_DESCRIPTION
    PHPBB_OIDC_CLIENT_ID PHPBB_OIDC_CLIENT_SECRET
    POSTGRES_DB OIDC_ISSUER_URL DOMAIN HTTPS_PORT
)
missing=()
for var in "${required_vars[@]}"; do
    if [ -z "${!var+x}" ]; then
        missing+=("$var")
    fi
done
if [ ${#missing[@]} -gt 0 ]; then
    echo "phpBB entrypoint: ERROR - missing required environment variables: ${missing[*]}" >&2
    exit 1
fi

: "${PHPBB_SERVER_NAME:=forum.${DOMAIN}}"
: "${PHPBB_SERVER_PORT:=${HTTPS_PORT}}"
: "${PHPBB_SERVER_PROTOCOL:=https://}"

escape_php_single() {
    printf "%s" "$1" | sed "s/'/'\\\\''/g"
}

wait_for_db() {
    echo "phpBB entrypoint: waiting for PostgreSQL at ${PHPBB_DB_HOST}:${PHPBB_DB_PORT}..."
    for _ in $(seq 1 45); do
        if PGPASSWORD="$PHPBB_DB_PASSWORD" pg_isready \
            -h "$PHPBB_DB_HOST" \
            -p "$PHPBB_DB_PORT" \
            -U "$PHPBB_DB_USER" \
            -d "$PHPBB_DB_NAME" >/dev/null 2>&1; then
            echo "phpBB entrypoint: PostgreSQL is ready."
            return 0
        fi
        sleep 2
    done
    echo "phpBB entrypoint: ERROR - PostgreSQL did not become ready in time."
    exit 1
}

prepare_writable_dirs() {
    mkdir -p \
        /var/www/html/cache \
        /var/www/html/files \
        /var/www/html/store \
        /var/www/html/images/avatars/upload
    chown -R www-data:www-data \
        /var/www/html/cache \
        /var/www/html/files \
        /var/www/html/store \
        /var/www/html/images/avatars/upload
}

write_config_php() {
    local dbhost dbport dbname dbuser dbpass prefix
    dbhost="$(escape_php_single "$PHPBB_DB_HOST")"
    dbport="$(escape_php_single "$PHPBB_DB_PORT")"
    dbname="$(escape_php_single "$PHPBB_DB_NAME")"
    dbuser="$(escape_php_single "$PHPBB_DB_USER")"
    dbpass="$(escape_php_single "$PHPBB_DB_PASSWORD")"
    prefix="$(escape_php_single "$PHPBB_TABLE_PREFIX")"

    cat >/var/www/html/config.php <<EOF
<?php
\$dbms = 'phpbb\\\\db\\\\driver\\\\postgres';
\$dbhost = '${dbhost}';
\$dbport = '${dbport}';
\$dbname = '${dbname}';
\$dbuser = '${dbuser}';
\$dbpasswd = '${dbpass}';
\$table_prefix = '${prefix}';
\$phpbb_adm_relative_path = 'adm/';
\$acm_type = 'phpbb\\\\cache\\\\driver\\\\file';
@define('PHPBB_INSTALLED', true);
?>
EOF
    chown www-data:www-data /var/www/html/config.php
}

db_initialized() {
    local config_table
    config_table="${PHPBB_TABLE_PREFIX}config"
    PGPASSWORD="$PHPBB_DB_PASSWORD" psql \
        -h "$PHPBB_DB_HOST" \
        -p "$PHPBB_DB_PORT" \
        -U "$PHPBB_DB_USER" \
        -d "$PHPBB_DB_NAME" \
        -tAc "SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = '${config_table}' LIMIT 1" 2>/dev/null | grep -q 1
}

forum_public_base() {
    if [ "$PHPBB_SERVER_PORT" = "443" ]; then
        printf "%s%s" "$PHPBB_SERVER_PROTOCOL" "$PHPBB_SERVER_NAME"
    else
        printf "%s%s:%s" "$PHPBB_SERVER_PROTOCOL" "$PHPBB_SERVER_NAME" "$PHPBB_SERVER_PORT"
    fi
}

configure_oidc_sso() {
    echo "phpBB entrypoint: configuring OIDC SSO..."
    cd /var/www/html

    php bin/phpbbcli.php extension:enable vgindex/oidc --safe-mode >/dev/null 2>&1 || true
    php bin/phpbbcli.php config:set auth_oauth_vgindex_key "$PHPBB_OIDC_CLIENT_ID" >/dev/null
    php bin/phpbbcli.php config:set auth_oauth_vgindex_secret "$PHPBB_OIDC_CLIENT_SECRET" >/dev/null
    php bin/phpbbcli.php config:set auth_method oauth >/dev/null
    # Disable local self-registration permanently; users must come from app SSO.
    php bin/phpbbcli.php config:set require_activation 3 >/dev/null

    local redirect_uri client_id_sql client_secret_sql redirect_uri_sql
    redirect_uri="$(forum_public_base)/ucp.php?mode=login&login=external&oauth_service=vgindex"
    client_id_sql="$(printf "%s" "$PHPBB_OIDC_CLIENT_ID" | sed "s/'/''/g")"
    client_secret_sql="$(printf "%s" "$PHPBB_OIDC_CLIENT_SECRET" | sed "s/'/''/g")"
    redirect_uri_sql="$(printf "%s" "$redirect_uri" | sed "s/'/''/g")"

    if ! PGPASSWORD="$PHPBB_DB_PASSWORD" psql \
        -h "$PHPBB_DB_HOST" \
        -p "$PHPBB_DB_PORT" \
        -U "$PHPBB_DB_USER" \
        -d "$POSTGRES_DB" \
        -v ON_ERROR_STOP=1 \
        -c "INSERT INTO oauth_clients (client_id, client_secret, redirect_uri, name) VALUES ('$client_id_sql', '$client_secret_sql', '$redirect_uri_sql', 'phpBB Forum')
            ON CONFLICT (client_id) DO UPDATE SET
              client_secret = EXCLUDED.client_secret,
              redirect_uri = EXCLUDED.redirect_uri,
              name = EXCLUDED.name;" >/dev/null 2>&1; then
        echo "phpBB entrypoint: warning - could not upsert oauth client in app DB (${POSTGRES_DB})."
    fi
}

write_install_config() {
    local cookie_secure
    cookie_secure=true
    if [ "$PHPBB_SERVER_PROTOCOL" != "https://" ]; then
        cookie_secure=false
    fi

    mkdir -p /var/www/html/install
    cat >/var/www/html/install/install-config.yml <<EOF
installer:
  admin:
    name: ${PHPBB_ADMIN_USER}
    password: ${PHPBB_ADMIN_PASSWORD}
    email: ${PHPBB_ADMIN_EMAIL}

  board:
    lang: en
    name: ${PHPBB_BOARD_NAME}
    description: ${PHPBB_BOARD_DESCRIPTION}

  database:
    dbms: postgres
    dbhost: ${PHPBB_DB_HOST}
    dbport: ${PHPBB_DB_PORT}
    dbuser: ${PHPBB_DB_USER}
    dbpasswd: ${PHPBB_DB_PASSWORD}
    dbname: ${PHPBB_DB_NAME}
    table_prefix: ${PHPBB_TABLE_PREFIX}

  email:
    enabled: false

  server:
    cookie_secure: ${cookie_secure}
    server_protocol: ${PHPBB_SERVER_PROTOCOL}
    force_server_vars: true
    server_name: ${PHPBB_SERVER_NAME}
    server_port: ${PHPBB_SERVER_PORT}
    script_path: /

  extensions: []
EOF
    chown www-data:www-data /var/www/html/install/install-config.yml
}

wait_for_db
prepare_writable_dirs

if ! db_initialized; then
    echo "phpBB entrypoint: database not initialized, running CLI installer..."
    write_install_config
    cd /var/www/html
    php install/phpbbcli.php install --no-interaction --safe-mode /var/www/html/install/install-config.yml
    rm -f /var/www/html/install/install-config.yml
    echo "phpBB entrypoint: install completed."
fi

rm -rf /var/www/html/install

write_config_php
configure_oidc_sso

chown -R www-data:www-data /var/www/html/cache

exec apache2-foreground
