# phpBB + PostgreSQL

This stack uses a repo-managed phpBB image and PostgreSQL. phpBB uses native
database authentication and exposes a first-party OpenID Connect provider for
MediaWiki and the Rust app.

## How it works

The container entrypoint automates first boot:

1. Waits for PostgreSQL.
2. Installs phpBB if `${PHPBB_TABLE_PREFIX}config` does not exist.
3. Writes `config.php` from environment variables.
4. Forces `auth_method = db`.
5. Syncs public URL and email/SMTP settings.
6. Disables any legacy `vgindex/oidc` phpBB extension rows if they exist.
7. Enables `vgindex/oidcprovider`, runs its migrations, and seeds the MediaWiki
   and Rust app OIDC clients.
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

The canonical local site is served through Caddy at
`http://forum.redump.test:$LOCAL_SITE_PORT/`.

## Configuration

Main variables:

- `PHPBB_DB_HOST` / `PHPBB_DB_PORT` / `PHPBB_DB_NAME`
- `PHPBB_DB_USER` / `PHPBB_DB_PASSWORD`
- `PHPBB_TABLE_PREFIX` (default: `phpbb_`)
- `PHPBB_ADMIN_USER` / `PHPBB_ADMIN_PASSWORD` / `PHPBB_ADMIN_EMAIL`
- `PHPBB_PUBLIC_URL` (canonical forum URL)
- `PHPBB_DIRECT_PORT` (loopback-only direct phpBB access, default `18080`)
- `PHPBB_COOKIE_DOMAIN`
- `OIDC_PROVIDER_URL` (normally `${PHPBB_PUBLIC_URL}/app.php/oidc`)
- `APP_PUBLIC_URL` / `MEDIAWIKI_PUBLIC_URL` for seeded redirect URIs
- `APP_OIDC_CLIENT_ID` / `APP_OIDC_CLIENT_SECRET`
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

Local development and production use one canonical provider URL,
`OIDC_PROVIDER_URL`, which normally points at the public forum URL plus
`/app.php/oidc`. For production, set `PHPBB_PUBLIC_URL` to the public HTTPS
forum origin, for example `https://forum.redump.info`.

Only seeded first-party clients are supported in v1. Client secrets,
authorization codes, and opaque access tokens are stored hashed in phpBB-owned
tables. The Rust app callback is seeded from `APP_PUBLIC_URL`, and the
MediaWiki callback is seeded from `MEDIAWIKI_PUBLIC_URL`. The RSA signing key lives at
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

Use `--target-domain redump.info` for deployment imports. Imported Redump-family
links outside the old forum are rewritten to HTTPS under that target domain,
including wiki subdomains. Old forum post/topic links that can be mapped to
imported phpBB IDs are stored as root-relative phpBB links, so they follow
whatever host you use to browse the board. Imported user emails under
`redump.org` or `*.redump.org` are also rewritten to the same target domain,
with any `--target-domain` port stripped for email addresses.

The importer looks for optional local test users at
`/import/redump/users/test_users.json`, which maps to
`data/redump/users/test_users.json` on the host. If the file does not exist,
test-user seeding is skipped.

## News Category

Create or import `Redump Forum / News` for homepage news. The importer marks
that forum as phpBB's built-in news feed source, so the app can read
`/feed.php?mode=news` over HTTP without direct phpBB database access.
