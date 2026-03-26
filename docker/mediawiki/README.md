# MediaWiki + PostgreSQL

This stack uses a custom MediaWiki image that explicitly enables PostgreSQL
PHP extensions (`pgsql`, `pdo_pgsql`) for reliable DB connectivity.

MediaWiki runs on its own subdomain (`wiki.$DOMAIN`), not under a path.

## How it works

The container has a custom entrypoint that fully automates first-time setup:

1. Waits for PostgreSQL to be ready.
2. Checks if the `page` table exists in the `mediawiki` database.
3. If not, runs `maintenance/install.php` to create the schema and admin account.
4. Runs `maintenance/update.php` to ensure extension tables (e.g. `openid_connect`) exist.
5. Starts Apache.

On subsequent starts, only `update.php` runs (fast no-op if nothing changed).

## Usage

```bash
docker compose up -d postgres app mediawiki caddy
```

No manual install steps needed. With default config, the wiki is at
`https://wiki.localhost:8443/`.

## Configuration

All settings derived from the top-level `DOMAIN` and `HTTPS_PORT` env vars.
Additional overrides in `docker-compose.yml`:

- `MEDIAWIKI_ADMIN_USER` (default: `admin`) - local wiki admin username
- `MEDIAWIKI_ADMIN_PASSWORD` (default: `ChangeMe!Admin2026`) - local wiki admin password
- `OIDC_PROVIDER_URL` (default: `http://app:3000`) - internal OIDC provider URL

## Volumes

Only `mediawiki_uploads` is persisted (mounted at `/var/www/html/images`).
The rest of the container filesystem comes fresh from the image on each restart,
ensuring Dockerfile changes (e.g. new extensions) always take effect.

## Health checks

- Container is running: `docker compose ps mediawiki`
- DB reachable: `docker compose exec mediawiki php -m | grep pgsql`
- Route works through Caddy: open `https://wiki.$DOMAIN:$HTTPS_PORT/`
