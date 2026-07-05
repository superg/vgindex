# MediaWiki + PostgreSQL

This stack uses a custom MediaWiki image that explicitly enables PostgreSQL
PHP extensions (`pgsql`, `pdo_pgsql`) for reliable DB connectivity.

MediaWiki runs on its own public URL (`MEDIAWIKI_PUBLIC_URL`), not under a path.

## How it works

The container has a custom entrypoint that automates first-time setup without
silently changing existing databases:

1. Waits for PostgreSQL to be ready.
2. Checks if the `page` table exists in the `mediawiki` database.
3. If not, runs `maintenance/install.php` to create the schema and admin account.
4. Runs the schema updater once to create extension tables such as `openid_connect`.
5. Starts Apache.

On subsequent starts, the web container never runs schema migrations. Major
upgrades must use the explicit one-shot migration service:

```bash
docker compose --profile migration run --rm -T mediawiki-migrate
```

This separation prevents a routine restart from mutating the production schema.
OIDC client registration is owned by the phpBB container; MediaWiki does not
write client rows into the application database.

The MediaWiki base image and both authentication extensions are pinned to exact
1.46-compatible revisions. The `REL1_46` OpenIDConnect PostgreSQL migration is
used without the workaround required by the old 1.42 extension.

## Usage

```bash
docker compose up -d postgres phpbb mediawiki caddy
```

No manual install steps needed. With default config, the wiki is at
`http://wiki.redump.test:$LOCAL_SITE_PORT/`.

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
- `MEDIAWIKI_READ_ONLY_REASON` (default: blank) - block HTTP writes while still
  allowing CLI maintenance commands; optional and not used by the production
  migration workflow

## Volumes

Only `mediawiki_uploads` is persisted (mounted at `/var/www/html/images`).
The rest of the container filesystem comes fresh from the image on each restart,
ensuring Dockerfile changes (e.g. new extensions) always take effect.

## Health checks

- Container is running: `docker compose ps mediawiki`
- Container health is `healthy`: `docker inspect $(docker compose ps -q mediawiki)`
- DB reachable: `docker compose exec mediawiki php -m | grep pgsql`
- Route works through Caddy: open `$MEDIAWIKI_PUBLIC_URL`
- Discovery works from MediaWiki:
  `docker compose exec mediawiki php -r 'echo file_get_contents(getenv("OIDC_PROVIDER_URL") . "/.well-known/openid-configuration");'`

## Major upgrades

Production major upgrades use `scripts/deployment/mediawiki-migration.sh` via
the manually dispatched **MediaWiki schema migration** workflow:

1. `prepare` drains jobs, stops the wiki, creates a MediaWiki-only snapshot,
   runs the updater, and starts the target version writable.
2. Validate pages, files, Special:Version, SSO, and group synchronization.
3. Run `finalize` to commit the release markers, or `rollback` to restore the
   database, uploads, runtime configuration, and previous image. Rollback also
   discards any wiki changes made after `prepare` created the snapshot.

The upgrade archive and checksum are retained under
`data/mediawiki-upgrade-backups`; they are not affected by daily-backup pruning.
