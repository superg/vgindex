# phpBB + PostgreSQL

This stack uses a repo-managed phpBB image (not Bitnami) and keeps phpBB on
PostgreSQL.

## How it works

The container entrypoint automates first boot:

1. Waits for PostgreSQL to become ready.
2. Checks for phpBB table `${PHPBB_TABLE_PREFIX}config`.
3. If missing, runs `php bin/phpbbcli.php install --no-interaction`.
4. Writes `config.php` from environment variables.
5. Enables `vgindex/oidc` extension and configures phpBB OAuth auth method.
6. Upserts phpBB OAuth client in app DB (`oauth_clients`) with current callback URL.
7. Starts Apache.

Subsequent restarts skip install and regenerate `config.php` to stay resilient
after container recreation.

## Usage

```bash
docker compose up -d postgres app phpbb caddy
```

With default config, forum is available at `https://forum.localhost:8443/`.

## Configuration

Main variables:

- `PHPBB_DB_HOST` / `PHPBB_DB_PORT` / `PHPBB_DB_NAME`
- `PHPBB_DB_USER` / `PHPBB_DB_PASSWORD`
- `PHPBB_TABLE_PREFIX` (default: `phpbb_`)
- `PHPBB_ADMIN_USER` / `PHPBB_ADMIN_PASSWORD` / `PHPBB_ADMIN_EMAIL`
- `PHPBB_OIDC_CLIENT_ID` / `PHPBB_OIDC_CLIENT_SECRET`
- `OIDC_ISSUER_URL` (internal OIDC URL, default `http://app:3000`)
- `APP_DB_NAME` (app DB where `oauth_clients` lives, default `vgindex`)
- `DOMAIN` + `HTTPS_PORT` (used to derive forum server URL)

## Persisted data

Only phpBB mutable data is persisted:

- `/var/www/html/files`
- `/var/www/html/store`
- `/var/www/html/images/avatars/upload`

Application code stays in the image so version bumps are predictable.

## Upgrade flow

1. Update `PHPBB_VERSION` build argument in `docker/phpbb/Dockerfile`.
2. Rebuild and restart phpBB:
   - `docker compose build phpbb`
   - `docker compose up -d phpbb`
3. If phpBB requires schema updates, run the built-in updater from phpBB admin.

## OAuth2/OIDC integration

The image ships an in-repo extension (`vgindex/oidc`) and configures SSO
automatically on startup:

- enables extension `vgindex/oidc`
- sets `auth_method` to `oauth` (OIDC-first login flow)
- disables local self-registration (`require_activation = 3`)
- writes OAuth key/secret from env vars
- syncs callback URL to `https://forum.<domain>:<port>/ucp.php?mode=login`

No ACP-side provider setup is required for baseline SSO.

### Auto-provisioning

First-time SSO users are automatically created in phpBB — no manual "link or
create account" screen is shown. The extension listens on
`core.oauth_login_after_check_if_provider_id_has_match` and, when no linked
account exists:

1. Fetches `preferred_username` and `email` from the OIDC userinfo response.
2. Creates a normal phpBB user in the `REGISTERED` group.
3. Inserts the OAuth account link so subsequent logins are direct.

**Username collision policy**: if `preferred_username` is already taken, a
numeric suffix is appended (`name2`, `name3`, …). As a final fallback, a
6-character hex hash of the OIDC `sub` is used.

The user's phpBB password is set to a random value — login is only possible
via the OIDC provider.

## News category

Create a forum called `News` (or keep forum ID `1`) for homepage news widgets that
query phpBB topics directly from PostgreSQL.
