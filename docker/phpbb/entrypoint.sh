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

: "${APP_PUBLIC_URL:=http://redump.test:18000}"
: "${PHPBB_PUBLIC_URL:=http://forum.redump.test:18000}"
: "${MEDIAWIKI_PUBLIC_URL:=http://wiki.redump.test:18000}"
: "${OIDC_PROVIDER_URL:=${PHPBB_PUBLIC_URL%/}/app.php/oidc}"
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
: "${PHPBB_BOOTSTRAP_MODE:=auto}"
: "${PHPBB_REQUIRE_ACTIVATION:=3}"
: "${PHPBB_ALLOW_PASSWORD_RESET:=true}"
: "${PHPBB_FEED_ENABLE:=true}"
: "${PHPBB_FEED_LIMIT_TOPIC:=5}"
: "${PHPBB_REMOTE_IP_INTERNAL_PROXIES:=10.0.0.0/8,172.16.0.0/12,192.168.0.0/16}"

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

validate_bootstrap_config() {
    case "$PHPBB_BOOTSTRAP_MODE" in
        auto|force|never) ;;
        *)
            echo "phpBB entrypoint: ERROR - PHPBB_BOOTSTRAP_MODE must be auto, force, or never." >&2
            exit 1
            ;;
    esac

    case "$PHPBB_REQUIRE_ACTIVATION" in
        0|1|2|3) ;;
        *)
            echo "phpBB entrypoint: ERROR - PHPBB_REQUIRE_ACTIVATION must be 0, 1, 2, or 3." >&2
            exit 1
            ;;
    esac

    case "$PHPBB_FEED_LIMIT_TOPIC" in
        ''|*[!0-9]*)
            echo "phpBB entrypoint: ERROR - PHPBB_FEED_LIMIT_TOPIC must be a non-negative integer." >&2
            exit 1
            ;;
    esac
}

url_part() {
    local url part
    url="$1"
    part="$2"
    php -r '$p = parse_url($argv[1]); if ($p === false || !isset($p[$argv[2]]) || $p[$argv[2]] === "") { exit(1); } echo $p[$argv[2]];' "$url" "$part"
}

url_scheme_protocol() {
    printf "%s://" "$(url_part "$1" scheme)"
}

url_port_or_default() {
    php -r '$p = parse_url($argv[1]); if ($p === false || !isset($p["scheme"])) { exit(1); } if (isset($p["port"])) { echo $p["port"]; } else { echo strtolower($p["scheme"]) === "https" ? "443" : "80"; }' "$1"
}

APP_PUBLIC_URL="${APP_PUBLIC_URL%/}"
PHPBB_PUBLIC_URL="${PHPBB_PUBLIC_URL%/}"
MEDIAWIKI_PUBLIC_URL="${MEDIAWIKI_PUBLIC_URL%/}"
OIDC_PROVIDER_URL="${OIDC_PROVIDER_URL%/}"

phpbb_public_protocol="$(url_scheme_protocol "$PHPBB_PUBLIC_URL")"
phpbb_public_host="$(url_part "$PHPBB_PUBLIC_URL" host)"
phpbb_public_port="$(url_port_or_default "$PHPBB_PUBLIC_URL")"

validate_bootstrap_config

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

smtp_auth_method_for_phpbb() {
    if [ -z "$PHPBB_SMTP_USER" ] && [ -z "$PHPBB_SMTP_PASSWORD" ]; then
        return
    fi

    if [ -z "$PHPBB_SMTP_USER" ] || [ -z "$PHPBB_SMTP_PASSWORD" ]; then
        echo "phpBB entrypoint: ERROR - PHPBB_SMTP_USER and PHPBB_SMTP_PASSWORD must both be set, or both be blank" >&2
        exit 1
    fi

    if [ -n "$PHPBB_SMTP_AUTH_METHOD" ]; then
        printf "%s" "$PHPBB_SMTP_AUTH_METHOD"
    else
        printf "PLAIN"
    fi
}

phpbb_oidc_authorize_url() {
    printf "%s/authorize" "$OIDC_PROVIDER_URL"
}

wiki_redirect_uri() {
    printf "%s/Special:PluggableAuthLogin" "$MEDIAWIKI_PUBLIC_URL"
}

app_redirect_uri() {
    printf "%s/auth/oidc/callback" "$APP_PUBLIC_URL"
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

configure_remote_ip() {
    local proxy proxy_list
    proxy_list="$(printf "%s" "$PHPBB_REMOTE_IP_INTERNAL_PROXIES" | tr ',' ' ')"
    set -- $proxy_list

    if [ "$#" -eq 0 ]; then
        a2disconf remoteip >/dev/null 2>&1 || true
        return
    fi

    {
        printf "%s\n" "RemoteIPHeader X-Forwarded-For"
        for proxy in "$@"; do
            printf "RemoteIPInternalProxy %s\n" "$proxy"
        done
    } >/etc/apache2/conf-available/remoteip.conf

    a2enconf remoteip >/dev/null
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
    if [ "$phpbb_public_protocol" != "https://" ]; then
        cookie_secure=0
    fi

    php bin/phpbbcli.php config:set server_protocol "$phpbb_public_protocol" >/dev/null
    php bin/phpbbcli.php config:set force_server_vars 1 >/dev/null
    php bin/phpbbcli.php config:set server_name "$phpbb_public_host" >/dev/null
    php bin/phpbbcli.php config:set server_port "$phpbb_public_port" >/dev/null
    php bin/phpbbcli.php config:set cookie_secure "$cookie_secure" >/dev/null
    php bin/phpbbcli.php config:set cookie_domain "$PHPBB_COOKIE_DOMAIN" >/dev/null
}

sync_auth_config() {
    echo "phpBB entrypoint: syncing native auth settings..."
    cd /var/www/html

    php bin/phpbbcli.php config:set auth_method db >/dev/null
    php bin/phpbbcli.php config:set require_activation "$PHPBB_REQUIRE_ACTIVATION" >/dev/null
    php bin/phpbbcli.php config:set allow_password_reset "$(bool_01 "$PHPBB_ALLOW_PASSWORD_RESET")" >/dev/null
}

sync_email_config() {
    echo "phpBB entrypoint: syncing email settings..."
    cd /var/www/html

    local smtp_delivery smtp_host smtp_auth_method
    smtp_delivery=0
    smtp_host="$(smtp_host_for_phpbb)"
    if [ -n "$smtp_host" ]; then
        smtp_delivery=1
    fi
    smtp_auth_method="$(smtp_auth_method_for_phpbb)"

    php bin/phpbbcli.php config:set email_enable "$(bool_01 "$PHPBB_EMAIL_ENABLE")" >/dev/null
    php bin/phpbbcli.php config:set board_email "$PHPBB_BOARD_EMAIL" >/dev/null
    php bin/phpbbcli.php config:set board_contact "$PHPBB_CONTACT_EMAIL" >/dev/null
    php bin/phpbbcli.php config:set smtp_delivery "$smtp_delivery" >/dev/null
    php bin/phpbbcli.php config:set smtp_host "$smtp_host" >/dev/null
    php bin/phpbbcli.php config:set smtp_port "$PHPBB_SMTP_PORT" >/dev/null
    php bin/phpbbcli.php config:set smtp_auth_method "$smtp_auth_method" >/dev/null
    php bin/phpbbcli.php config:set smtp_username "$PHPBB_SMTP_USER" >/dev/null
    php bin/phpbbcli.php config:set smtp_password "$PHPBB_SMTP_PASSWORD" >/dev/null
}

sync_feed_config() {
    echo "phpBB entrypoint: syncing feed settings..."
    cd /var/www/html

    php bin/phpbbcli.php config:set feed_enable "$(bool_01 "$PHPBB_FEED_ENABLE")" >/dev/null
    php bin/phpbbcli.php config:set feed_limit_topic "$PHPBB_FEED_LIMIT_TOPIC" >/dev/null
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

    php bin/phpbbcli.php config:set vgindex_oidc_issuer_url "$OIDC_PROVIDER_URL" >/dev/null
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

run_bootstrap() {
    echo "phpBB entrypoint: applying bootstrap settings..."
    disable_legacy_oidc_extension
    sync_server_config
    sync_auth_config
    sync_email_config
    sync_feed_config
    enable_oidc_provider
    sync_oidc_provider_config
    seed_mediawiki_oidc_client
    seed_app_oidc_client
}

write_install_config() {
    local cookie_secure
    cookie_secure=true
    if [ "$phpbb_public_protocol" != "https://" ]; then
        cookie_secure=false
    fi

    local email_enabled smtp_delivery smtp_host smtp_auth_method
    email_enabled="$(bool_word "$PHPBB_EMAIL_ENABLE")"
    smtp_delivery=false
    smtp_host="$(smtp_host_for_phpbb)"
    if [ -n "$smtp_host" ]; then
        smtp_delivery=true
    fi
    smtp_auth_method="$(smtp_auth_method_for_phpbb)"

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
    smtp_auth: "${smtp_auth_method}"
    smtp_user: "${PHPBB_SMTP_USER}"
    smtp_pass: "${PHPBB_SMTP_PASSWORD}"

  server:
    cookie_secure: ${cookie_secure}
    server_protocol: "${phpbb_public_protocol}"
    force_server_vars: true
    server_name: "${phpbb_public_host}"
    server_port: ${phpbb_public_port}
    script_path: /

  extensions: []
EOF
    chown www-data:www-data /var/www/html/install/install-config.yml
}

wait_for_db
configure_remote_ip
prepare_writable_dirs

installed_now=0
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
    installed_now=1
fi

# phpBB serves a "board unavailable" page to non-admins while /install exists.
# Images include this directory by default, so always remove it at startup.
rm -rf /var/www/html/install

write_config_php
ensure_oidc_signing_key

case "$PHPBB_BOOTSTRAP_MODE" in
    auto)
        if [ "$installed_now" = "1" ]; then
            run_bootstrap
        else
            echo "phpBB entrypoint: skipping bootstrap settings; phpBB database already exists."
        fi
        ;;
    force)
        run_bootstrap
        ;;
    never)
        echo "phpBB entrypoint: skipping bootstrap settings; PHPBB_BOOTSTRAP_MODE=never."
        ;;
    *)
        echo "phpBB entrypoint: ERROR - unexpected PHPBB_BOOTSTRAP_MODE after validation." >&2
        exit 1
        ;;
esac

chown -R www-data:www-data /var/www/html/cache

exec apache2-foreground
