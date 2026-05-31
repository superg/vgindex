#!/bin/bash
set -euo pipefail

required_vars=(
    PHPBB_DB_HOST PHPBB_DB_PORT PHPBB_DB_NAME PHPBB_DB_USER PHPBB_DB_PASSWORD
    PHPBB_TABLE_PREFIX PHPBB_ADMIN_USER PHPBB_ADMIN_PASSWORD PHPBB_ADMIN_EMAIL
    PHPBB_BOARD_NAME PHPBB_BOARD_DESCRIPTION
    APP_OIDC_CLIENT_ID APP_OIDC_CLIENT_SECRET
    MEDIAWIKI_OIDC_CLIENT_ID MEDIAWIKI_OIDC_CLIENT_SECRET
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

: "${DOMAIN:=localhost}"
: "${HTTPS_PORT:=8443}"
: "${APP_PUBLIC_URL:=}"
: "${PHPBB_SERVER_NAME:=forum.${DOMAIN}}"
: "${PHPBB_SERVER_PORT:=${HTTPS_PORT}}"
: "${PHPBB_SERVER_PROTOCOL:=https://}"
: "${PHPBB_COOKIE_DOMAIN:=}"
: "${PHPBB_EMAIL_ENABLE:=false}"
: "${PHPBB_BOARD_EMAIL:=${PHPBB_ADMIN_EMAIL}}"
: "${PHPBB_CONTACT_EMAIL:=${PHPBB_ADMIN_EMAIL}}"
: "${PHPBB_SMTP_HOST:=}"
: "${PHPBB_SMTP_PORT:=25}"
: "${PHPBB_SMTP_USER:=}"
: "${PHPBB_SMTP_PASSWORD:=}"
: "${PHPBB_SMTP_AUTH_METHOD:=}"
: "${PHPBB_SMTP_SECURE:=}"
: "${PHPBB_OIDC_ISSUER_URL:=http://phpbb/app.php/oidc}"
: "${PHPBB_OIDC_AUTHORIZE_URL:=}"

escape_php_single() {
    printf "%s" "$1" | sed "s/'/'\\\\''/g"
}

escape_sql_single() {
    printf "%s" "$1" | sed "s/'/''/g"
}

bool_01() {
    case "$(printf "%s" "$1" | tr '[:upper:]' '[:lower:]')" in
        1|true|yes|on|enabled) printf "1" ;;
        *) printf "0" ;;
    esac
}

bool_word() {
    if [ "$(bool_01 "$1")" = "1" ]; then
        printf "true"
    else
        printf "false"
    fi
}

smtp_host_for_phpbb() {
    if [ -z "$PHPBB_SMTP_HOST" ]; then
        return
    fi

    local smtp_secure
    smtp_secure="$(printf "%s" "$PHPBB_SMTP_SECURE" | tr '[:upper:]' '[:lower:]')"

    case "$PHPBB_SMTP_HOST" in
        *://*) printf "%s" "$PHPBB_SMTP_HOST" ;;
        *)
            case "$smtp_secure" in
                ssl|tls) printf "%s://%s" "$smtp_secure" "$PHPBB_SMTP_HOST" ;;
                *) printf "%s" "$PHPBB_SMTP_HOST" ;;
            esac
            ;;
    esac
}

url_with_optional_port() {
    local protocol host port path default_port port_suffix
    protocol="$1"
    host="$2"
    port="$3"
    path="$4"
    default_port=""
    case "$protocol" in
        http://) default_port="80" ;;
        https://) default_port="443" ;;
    esac

    port_suffix=""
    if [ -n "$port" ] && [ "$port" != "$default_port" ]; then
        port_suffix=":${port}"
    fi

    printf "%s%s%s%s" "$protocol" "$host" "$port_suffix" "$path"
}

phpbb_oidc_authorize_url() {
    if [ -n "$PHPBB_OIDC_AUTHORIZE_URL" ]; then
        printf "%s" "$PHPBB_OIDC_AUTHORIZE_URL"
        return
    fi

    url_with_optional_port \
        "$PHPBB_SERVER_PROTOCOL" \
        "$PHPBB_SERVER_NAME" \
        "$PHPBB_SERVER_PORT" \
        "/app.php/oidc/authorize"
}

wiki_redirect_uri() {
    if [ "$HTTPS_PORT" = "443" ]; then
        printf "https://wiki.%s/Special:PluggableAuthLogin" "$DOMAIN"
    else
        printf "https://wiki.%s:%s/Special:PluggableAuthLogin" "$DOMAIN" "$HTTPS_PORT"
    fi
}

app_redirect_uri() {
    if [ -n "$APP_PUBLIC_URL" ]; then
        printf "%s/auth/oidc/callback" "${APP_PUBLIC_URL%/}"
        return
    fi

    if [ "$HTTPS_PORT" = "443" ]; then
        printf "https://www.%s/auth/oidc/callback" "$DOMAIN"
    else
        printf "https://www.%s:%s/auth/oidc/callback" "$DOMAIN" "$HTTPS_PORT"
    fi
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

sync_server_config() {
    echo "phpBB entrypoint: syncing public server URL..."
    cd /var/www/html

    local cookie_secure
    cookie_secure=1
    if [ "$PHPBB_SERVER_PROTOCOL" != "https://" ]; then
        cookie_secure=0
    fi

    php bin/phpbbcli.php config:set server_protocol "$PHPBB_SERVER_PROTOCOL" >/dev/null
    php bin/phpbbcli.php config:set force_server_vars 1 >/dev/null
    php bin/phpbbcli.php config:set server_name "$PHPBB_SERVER_NAME" >/dev/null
    php bin/phpbbcli.php config:set server_port "$PHPBB_SERVER_PORT" >/dev/null
    php bin/phpbbcli.php config:set cookie_secure "$cookie_secure" >/dev/null
    php bin/phpbbcli.php config:set cookie_domain "$PHPBB_COOKIE_DOMAIN" >/dev/null
}

sync_auth_config() {
    echo "phpBB entrypoint: syncing native auth settings..."
    cd /var/www/html

    php bin/phpbbcli.php config:set auth_method db >/dev/null
    # Keep public self-registration disabled; imported users recover access through password reset.
    php bin/phpbbcli.php config:set require_activation 3 >/dev/null
    php bin/phpbbcli.php config:set allow_password_reset 1 >/dev/null
}

sync_email_config() {
    echo "phpBB entrypoint: syncing email settings..."
    cd /var/www/html

    local smtp_delivery smtp_host
    smtp_delivery=0
    smtp_host="$(smtp_host_for_phpbb)"
    if [ -n "$smtp_host" ]; then
        smtp_delivery=1
    fi

    php bin/phpbbcli.php config:set email_enable "$(bool_01 "$PHPBB_EMAIL_ENABLE")" >/dev/null
    php bin/phpbbcli.php config:set board_email "$PHPBB_BOARD_EMAIL" >/dev/null
    php bin/phpbbcli.php config:set board_contact "$PHPBB_CONTACT_EMAIL" >/dev/null
    php bin/phpbbcli.php config:set smtp_delivery "$smtp_delivery" >/dev/null
    php bin/phpbbcli.php config:set smtp_host "$smtp_host" >/dev/null
    php bin/phpbbcli.php config:set smtp_port "$PHPBB_SMTP_PORT" >/dev/null
    php bin/phpbbcli.php config:set smtp_auth_method "$PHPBB_SMTP_AUTH_METHOD" >/dev/null
    php bin/phpbbcli.php config:set smtp_username "$PHPBB_SMTP_USER" >/dev/null
    php bin/phpbbcli.php config:set smtp_password "$PHPBB_SMTP_PASSWORD" >/dev/null
}

disable_legacy_oidc_extension() {
    local config_table ext_table
    config_table="${PHPBB_TABLE_PREFIX}config"
    ext_table="${PHPBB_TABLE_PREFIX}ext"

    PGPASSWORD="$PHPBB_DB_PASSWORD" psql \
        -h "$PHPBB_DB_HOST" \
        -p "$PHPBB_DB_PORT" \
        -U "$PHPBB_DB_USER" \
        -d "$PHPBB_DB_NAME" \
        -v ON_ERROR_STOP=1 \
        -c "UPDATE ${ext_table} SET ext_active = 0 WHERE ext_name = 'vgindex/oidc';" \
        -c "DELETE FROM ${config_table} WHERE config_name LIKE 'auth_oauth_vgindex_%';" \
        >/dev/null 2>&1 || true
}

oidc_provider_enabled() {
    local ext_table
    ext_table="${PHPBB_TABLE_PREFIX}ext"

    PGPASSWORD="$PHPBB_DB_PASSWORD" psql \
        -h "$PHPBB_DB_HOST" \
        -p "$PHPBB_DB_PORT" \
        -U "$PHPBB_DB_USER" \
        -d "$PHPBB_DB_NAME" \
        -tAc "SELECT ext_active FROM ${ext_table} WHERE ext_name = 'vgindex/oidcprovider' LIMIT 1" 2>/dev/null | grep -q 1
}

enable_oidc_provider() {
    echo "phpBB entrypoint: enabling VGIndex OIDC provider extension..."
    cd /var/www/html

    if oidc_provider_enabled; then
        echo "phpBB entrypoint: OIDC provider extension already enabled."
        return
    fi

    php bin/phpbbcli.php extension:enable vgindex/oidcprovider --no-interaction
}

sync_oidc_provider_config() {
    echo "phpBB entrypoint: syncing OIDC provider settings..."
    cd /var/www/html

    php bin/phpbbcli.php config:set vgindex_oidc_issuer_url "$PHPBB_OIDC_ISSUER_URL" >/dev/null
    php bin/phpbbcli.php config:set vgindex_oidc_authorize_url "$(phpbb_oidc_authorize_url)" >/dev/null
}

ensure_oidc_signing_key() {
    local key_path
    key_path="/var/www/html/store/vgindex_oidc_private_key.pem"

    if [ ! -s "$key_path" ]; then
        echo "phpBB entrypoint: generating OIDC signing key..."
        php -r '$p = "/var/www/html/store/vgindex_oidc_private_key.pem"; if (is_file($p) && filesize($p) > 0) { exit(0); } $key = openssl_pkey_new(["private_key_bits" => 2048, "private_key_type" => OPENSSL_KEYTYPE_RSA]); if ($key === false || !openssl_pkey_export($key, $pem)) { fwrite(STDERR, "Could not generate OIDC signing key\n"); exit(1); } file_put_contents($p, $pem);'
    fi

    chown www-data:www-data "$key_path"
    chmod 600 "$key_path"
}

seed_mediawiki_oidc_client() {
    echo "phpBB entrypoint: seeding MediaWiki OIDC client..."

    local clients_table redirect_uri redirect_uris_json secret_hash now
    local client_id_sql secret_hash_sql redirect_uris_sql
    clients_table="${PHPBB_TABLE_PREFIX}vgindex_oidc_clients"
    redirect_uri="$(wiki_redirect_uri)"
    redirect_uris_json="$(php -r 'echo json_encode([$argv[1]], JSON_UNESCAPED_SLASHES);' "$redirect_uri")"
    secret_hash="$(php -r 'echo password_hash($argv[1], PASSWORD_DEFAULT);' "$MEDIAWIKI_OIDC_CLIENT_SECRET")"
    now="$(date +%s)"

    client_id_sql="$(escape_sql_single "$MEDIAWIKI_OIDC_CLIENT_ID")"
    secret_hash_sql="$(escape_sql_single "$secret_hash")"
    redirect_uris_sql="$(escape_sql_single "$redirect_uris_json")"

    PGPASSWORD="$PHPBB_DB_PASSWORD" psql \
        -h "$PHPBB_DB_HOST" \
        -p "$PHPBB_DB_PORT" \
        -U "$PHPBB_DB_USER" \
        -d "$PHPBB_DB_NAME" \
        -v ON_ERROR_STOP=1 \
        -c "INSERT INTO ${clients_table} (client_id, client_secret_hash, redirect_uris, active, first_party, created_at, updated_at)
            VALUES ('${client_id_sql}', '${secret_hash_sql}', '${redirect_uris_sql}', 1, 1, ${now}, ${now})
            ON CONFLICT (client_id) DO UPDATE SET
              client_secret_hash = EXCLUDED.client_secret_hash,
              redirect_uris = EXCLUDED.redirect_uris,
              active = 1,
              first_party = 1,
              updated_at = ${now};" \
        >/dev/null
}

seed_app_oidc_client() {
    echo "phpBB entrypoint: seeding app OIDC client..."

    local clients_table redirect_uri redirect_uris_json secret_hash now
    local client_id_sql secret_hash_sql redirect_uris_sql
    clients_table="${PHPBB_TABLE_PREFIX}vgindex_oidc_clients"
    redirect_uri="$(app_redirect_uri)"
    redirect_uris_json="$(php -r 'echo json_encode([$argv[1]], JSON_UNESCAPED_SLASHES);' "$redirect_uri")"
    secret_hash="$(php -r 'echo password_hash($argv[1], PASSWORD_DEFAULT);' "$APP_OIDC_CLIENT_SECRET")"
    now="$(date +%s)"

    client_id_sql="$(escape_sql_single "$APP_OIDC_CLIENT_ID")"
    secret_hash_sql="$(escape_sql_single "$secret_hash")"
    redirect_uris_sql="$(escape_sql_single "$redirect_uris_json")"

    PGPASSWORD="$PHPBB_DB_PASSWORD" psql \
        -h "$PHPBB_DB_HOST" \
        -p "$PHPBB_DB_PORT" \
        -U "$PHPBB_DB_USER" \
        -d "$PHPBB_DB_NAME" \
        -v ON_ERROR_STOP=1 \
        -c "INSERT INTO ${clients_table} (client_id, client_secret_hash, redirect_uris, active, first_party, created_at, updated_at)
            VALUES ('${client_id_sql}', '${secret_hash_sql}', '${redirect_uris_sql}', 1, 1, ${now}, ${now})
            ON CONFLICT (client_id) DO UPDATE SET
              client_secret_hash = EXCLUDED.client_secret_hash,
              redirect_uris = EXCLUDED.redirect_uris,
              active = 1,
              first_party = 1,
              updated_at = ${now};" \
        >/dev/null
}

write_install_config() {
    local cookie_secure
    cookie_secure=true
    if [ "$PHPBB_SERVER_PROTOCOL" != "https://" ]; then
        cookie_secure=false
    fi

    local email_enabled smtp_delivery smtp_host
    email_enabled="$(bool_word "$PHPBB_EMAIL_ENABLE")"
    smtp_delivery=false
    smtp_host="$(smtp_host_for_phpbb)"
    if [ -n "$smtp_host" ]; then
        smtp_delivery=true
    fi

    mkdir -p /var/www/html/install
    cat >/var/www/html/install/install-config.yml <<EOF
installer:
  admin:
    name: "${PHPBB_ADMIN_USER}"
    password: "${PHPBB_ADMIN_PASSWORD}"
    email: "${PHPBB_ADMIN_EMAIL}"

  board:
    lang: en
    name: "${PHPBB_BOARD_NAME}"
    description: "${PHPBB_BOARD_DESCRIPTION}"

  database:
    dbms: postgres
    dbhost: "${PHPBB_DB_HOST}"
    dbport: ${PHPBB_DB_PORT}
    dbuser: "${PHPBB_DB_USER}"
    dbpasswd: "${PHPBB_DB_PASSWORD}"
    dbname: "${PHPBB_DB_NAME}"
    table_prefix: "${PHPBB_TABLE_PREFIX}"

  email:
    enabled: ${email_enabled}
    smtp_delivery: ${smtp_delivery}
    smtp_host: "${smtp_host}"
    smtp_port: "${PHPBB_SMTP_PORT}"
    smtp_auth: "${PHPBB_SMTP_AUTH_METHOD}"
    smtp_user: "${PHPBB_SMTP_USER}"
    smtp_pass: "${PHPBB_SMTP_PASSWORD}"

  server:
    cookie_secure: ${cookie_secure}
    server_protocol: "${PHPBB_SERVER_PROTOCOL}"
    force_server_vars: true
    server_name: "${PHPBB_SERVER_NAME}"
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
    echo "phpBB entrypoint: generated install config:"
    cat /var/www/html/install/install-config.yml
    php /var/www/html/install/phpbbcli.php install /var/www/html/install/install-config.yml --no-interaction
    rm -f /var/www/html/install/install-config.yml

    if ! db_initialized; then
        echo "phpBB entrypoint: ERROR — installer exited 0 but tables were not created" >&2
        exit 1
    fi
    echo "phpBB entrypoint: install completed, removing install dir."
    rm -rf /var/www/html/install
fi

# phpBB serves a "board unavailable" page to non-admins while /install exists.
# Images include this directory by default, so always remove it at startup.
rm -rf /var/www/html/install

write_config_php
disable_legacy_oidc_extension
sync_server_config
sync_auth_config
sync_email_config
enable_oidc_provider
sync_oidc_provider_config
ensure_oidc_signing_key
seed_mediawiki_oidc_client
seed_app_oidc_client

chown -R www-data:www-data /var/www/html/cache

exec apache2-foreground
