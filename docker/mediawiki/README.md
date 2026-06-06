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

The image patches OpenIDConnect's PostgreSQL primary-key migration during build
so fresh databases create the `openid_connect` table with the expected schema.
Startup fails if `update.php` fails, which prevents a broken schema from being
silently accepted.

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

## Redump Wiki Import

Scraped Redump wiki XML can be imported with:

```bash
bash scripts/redump_wiki_scraper/import.sh --target-domain vgindex.org
```

The importer rewrites Redump-family links before passing each XML file to
MediaWiki. Large pages need more than PHP's default 128M CLI memory limit, so
the script uses `1024M` by default. Override it with `--php-memory-limit 2048M`
or `WIKI_IMPORT_MEMORY_LIMIT=2048M`.

For one-off imports where pages were copied into the MediaWiki upload volume,
pass both the host path and the matching in-container path:

```bash
bash scripts/redump_wiki_scraper/import.sh --target-domain vgindex.org --pages-dir /root/temp/vgindex/data/redump/wiki/pages --container-pages-dir /var/www/html/images/redump-import/wiki/pages
```

## Health checks

- Container is running: `docker compose ps mediawiki`
- DB reachable: `docker compose exec mediawiki php -m | grep pgsql`
- Route works through Caddy: open `$MEDIAWIKI_PUBLIC_URL`
- Discovery works from MediaWiki:
  `docker compose exec mediawiki php -r 'echo file_get_contents(getenv("OIDC_PROVIDER_URL") . "/.well-known/openid-configuration");'`
