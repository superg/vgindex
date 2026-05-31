# MediaWiki + PostgreSQL

This stack uses a custom MediaWiki image that explicitly enables PostgreSQL
PHP extensions (`pgsql`, `pdo_pgsql`) for reliable DB connectivity.

MediaWiki runs on its own public URL (`MEDIAWIKI_PUBLIC_URL`), not under a path.

## How it works

The container has a custom entrypoint that fully automates first-time setup:

1. Waits for PostgreSQL to be ready.
2. Checks if the `page` table exists in the `mediawiki` database.
3. If not, runs `maintenance/install.php` to create the schema and admin account.
4. Runs `maintenance/update.php` to ensure extension tables (e.g. `openid_connect`) exist.
5. Starts Apache.

On subsequent starts, only `update.php` runs (fast no-op if nothing changed).
OIDC client registration is owned by the phpBB container; MediaWiki does not
write client rows into the application database.

## Usage

```bash
docker compose up -d postgres phpbb mediawiki caddy
```

No manual install steps needed. With default config, the wiki is at
`http://wiki.vgindex.test:$LOCAL_SITE_PORT/`.

## Configuration

Main settings:

- `MEDIAWIKI_ADMIN_USER` (default: `admin`) - local wiki admin username
- `MEDIAWIKI_ADMIN_PASSWORD` (default: `changeme-mediawiki`) - local wiki admin password
- `MEDIAWIKI_PUBLIC_URL` - canonical wiki URL
- `OIDC_PROVIDER_URL` - phpBB OIDC provider URL
- `MEDIAWIKI_OIDC_CLIENT_ID` / `MEDIAWIKI_OIDC_CLIENT_SECRET`
- `MEDIAWIKI_OIDC_VERIFY_TLS` (default: `true`) - set `false` only for local
  HTTPS issuer testing with self-signed certificates
- `MEDIAWIKI_LOCAL_LOGIN` (default: `false`) - keep ordinary local login hidden

## Volumes

Only `mediawiki_uploads` is persisted (mounted at `/var/www/html/images`).
The rest of the container filesystem comes fresh from the image on each restart,
ensuring Dockerfile changes (e.g. new extensions) always take effect.

## Health checks

- Container is running: `docker compose ps mediawiki`
- DB reachable: `docker compose exec mediawiki php -m | grep pgsql`
- Route works through Caddy: open `$MEDIAWIKI_PUBLIC_URL`
- Discovery works from MediaWiki:
  `docker compose exec mediawiki php -r 'echo file_get_contents(getenv("OIDC_PROVIDER_URL") . "/.well-known/openid-configuration");'`
