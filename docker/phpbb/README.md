# phpBB + PostgreSQL

This stack uses a repo-managed phpBB image and PostgreSQL. phpBB uses native
database authentication and exposes a first-party OpenID Connect provider for
MediaWiki and the Rust app.

## How it works

The container entrypoint always performs runtime setup:

1. Waits for PostgreSQL.
2. Installs phpBB if `${PHPBB_TABLE_PREFIX}config` does not exist.
3. Writes `config.php` from environment variables so phpBB can reach the
   database after container recreation.
4. Removes the bundled `/install` directory.
5. Generates one RSA signing key in the persisted phpBB `store` volume if it is
   missing.
6. Starts Apache.

On a fresh phpBB database install, the entrypoint also bootstraps phpBB from
environment variables: public URL/cookie settings, registration/password reset,
email/SMTP, feed settings, legacy OIDC cleanup, `vgindex/oidcprovider`, and the
MediaWiki and Rust app OIDC clients.

Subsequent restarts skip that bootstrap in the default `auto` mode, so phpBB ACP
and database settings remain the source of truth after deployment.

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
- `PHPBB_BOOTSTRAP_MODE` (`auto`, `force`, or `never`; default: `auto`)
- `PHPBB_REQUIRE_ACTIVATION` (bootstrap default: `3`, registration disabled)
- `PHPBB_ALLOW_PASSWORD_RESET` (bootstrap default: `true`)
- `PHPBB_FEED_ENABLE` (bootstrap default: `true`)
- `PHPBB_FEED_LIMIT_TOPIC` (bootstrap default: `5`)
- `PHPBB_REMOTE_IP_INTERNAL_PROXIES` (comma-separated internal proxy IPs/CIDRs
  trusted for `X-Forwarded-For`; default covers Docker RFC1918 networks)
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

In the default `auto` bootstrap mode, these phpBB settings are applied only when
the phpBB database is first created. To intentionally reapply environment
defaults to an existing database, start phpBB once with
`PHPBB_BOOTSTRAP_MODE=force`. Use `never` for manual recovery paths where even a
fresh install should not receive bootstrap settings.

Public self-registration is disabled by the default bootstrap values, but active
imported users can recover access through phpBB password reset once email is
configured. After bootstrap, registration and email settings can be changed in
phpBB ACP and will survive container restarts.

## Real Client IPs Behind Caddy

Caddy forwards the client IP in `X-Forwarded-For`. The phpBB image enables
Apache `mod_remoteip` so Apache rewrites `REMOTE_ADDR` before phpBB reads it.
That makes phpBB logs, bans, and sessions use the external visitor IP instead of
the internal Docker proxy address, such as `172.18.0.6`.

By default, Apache trusts forwarded IPs only from private Docker/LAN ranges:

```text
10.0.0.0/8,172.16.0.0/12,192.168.0.0/16
```

For production, you can tighten this to the exact Compose subnet or proxy
container address:

```env
PHPBB_REMOTE_IP_INTERNAL_PROXIES=172.18.0.0/16
```

Recreate the phpBB container after changing it:

```bash
docker compose up -d --build phpbb
```

If another proxy sits in front of Caddy, configure that proxy to pass
`X-Forwarded-For` to Caddy as well.

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
