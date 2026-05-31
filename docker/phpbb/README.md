# phpBB + PostgreSQL

This stack uses a repo-managed phpBB image and PostgreSQL. phpBB uses native
database authentication and exposes a first-party OpenID Connect provider for
MediaWiki.

## How it works

The container entrypoint automates first boot:

1. Waits for PostgreSQL.
2. Installs phpBB if `${PHPBB_TABLE_PREFIX}config` does not exist.
3. Writes `config.php` from environment variables.
4. Forces `auth_method = db`.
5. Syncs public URL and email/SMTP settings.
6. Disables any legacy `vgindex/oidc` phpBB extension rows if they exist.
7. Enables `vgindex/oidcprovider`, runs its migrations, and seeds the MediaWiki
   OIDC client.
8. Generates one RSA signing key in the persisted phpBB `store` volume if it is
   missing.
9. Starts Apache.

Subsequent restarts skip install and regenerate runtime config so container
recreation stays predictable.

## Usage

```bash
docker compose up -d postgres app phpbb
```

With defaults, phpBB is exposed directly at:

```text
http://localhost:18080/
```

To serve through Caddy instead, set:

```env
PHPBB_SERVER_NAME=forum.localhost
PHPBB_SERVER_PORT=8443
PHPBB_SERVER_PROTOCOL=https://
```

Then start Caddy and use `https://forum.localhost:8443/`.

## Configuration

Main variables:

- `PHPBB_DB_HOST` / `PHPBB_DB_PORT` / `PHPBB_DB_NAME`
- `PHPBB_DB_USER` / `PHPBB_DB_PASSWORD`
- `PHPBB_TABLE_PREFIX` (default: `phpbb_`)
- `PHPBB_ADMIN_USER` / `PHPBB_ADMIN_PASSWORD` / `PHPBB_ADMIN_EMAIL`
- `PHPBB_HTTP_PORT` (host port for direct phpBB access, default `18080`)
- `PHPBB_SERVER_NAME` / `PHPBB_SERVER_PORT` / `PHPBB_SERVER_PROTOCOL`
- `PHPBB_COOKIE_DOMAIN`
- `PHPBB_OIDC_ISSUER_URL` (default: `http://phpbb/app.php/oidc`)
- `PHPBB_OIDC_AUTHORIZE_URL` (optional browser-facing authorize endpoint)
- `MEDIAWIKI_OIDC_CLIENT_ID` / `MEDIAWIKI_OIDC_CLIENT_SECRET`

Email/password reset variables:

- `PHPBB_EMAIL_ENABLE` (default: `false`)
- `PHPBB_BOARD_EMAIL`
- `PHPBB_CONTACT_EMAIL`
- `PHPBB_SMTP_HOST`
- `PHPBB_SMTP_PORT`
- `PHPBB_SMTP_USER`
- `PHPBB_SMTP_PASSWORD`
- `PHPBB_SMTP_AUTH_METHOD`
- `PHPBB_SMTP_SECURE` (`ssl` or `tls` when the host does not already include a
  scheme)

Public self-registration is disabled, but active imported users can recover
access through phpBB password reset once email is configured.

## OpenID Connect Provider

The phpBB extension is served under `/app.php/oidc`:

- `/app.php/oidc/.well-known/openid-configuration`
- `/app.php/oidc/authorize`
- `/app.php/oidc/token`
- `/app.php/oidc/userinfo`
- `/app.php/oidc/jwks`

Local development defaults use `http://phpbb/app.php/oidc` as the internal
issuer so MediaWiki can reach phpBB over the Docker network. The discovery
document can advertise a separate browser-facing authorize URL via
`PHPBB_OIDC_AUTHORIZE_URL`; otherwise it is derived from phpBB's configured
public server URL. For production, set the issuer/provider URL to the public
HTTPS forum origin, for example `https://forum.vgindex.org/app.php/oidc`.

Only the seeded first-party MediaWiki client is supported in v1. Client secrets,
authorization codes, and opaque access tokens are stored hashed in phpBB-owned
tables. The RSA signing key lives at
`/var/www/html/store/vgindex_oidc_private_key.pem`, which is persisted by the
`phpbb_store` volume.

## Persisted Data

Only phpBB mutable data is persisted:

- `/var/www/html/files`
- `/var/www/html/store`
- `/var/www/html/images/avatars/upload`

Application code stays in the image.

## Redump Forum Import

The image includes `redump-forum-import` for the scraped Redump archive. Compose
mounts `./data/redump` read-only at `/import/redump`, so a preflight can be run
with:

```bash
docker compose exec --user www-data phpbb redump-forum-import \
  --forum-data /import/redump/forum \
  --users-dir /import/redump/users \
  --source-timezone UTC \
  --target-domain localhost \
  --dry-run
```

Remove `--dry-run` to import into a fresh/disposable phpBB board. The importer
refuses to run if real forum content already exists beyond phpBB installer
sample data.

Use `--target-domain vgindex.org` for deployment imports. Imported Redump-family
links outside the old forum are rewritten to HTTPS under that target domain,
including wiki subdomains. Old forum post/topic links that can be mapped to
imported phpBB IDs are stored as root-relative phpBB links, so they follow
whatever host you use to browse the board.

The importer looks for optional local test users at
`/import/redump/users/test_users.json`, which maps to
`data/redump/users/test_users.json` on the host. If the file does not exist,
test-user seeding is skipped.

## News Category

Create or import a forum called `News` for homepage news widgets that query
phpBB topics directly from PostgreSQL.
