# Deployment Guide

## Architecture

```
GitHub push → CI (build check) → CD (build/push to registry) → SSH deploy → server
```

Three custom images are built and published to a container registry on every push to `main`:
- `app`
- `mediawiki`
- `phpbb`

Each image is tagged with `sha-<commit>` (immutable) and `main` (moving).

## Prerequisites

### Server setup

1. Run the deploy user setup script as root on the server and follow its
   printed instructions:

```bash
bash scripts/deployment/setup-deploy-user.sh
```

2. Log in to the container registry **as the deploy user** so `docker compose pull` can fetch images:

```bash
su - deploy
echo "<TOKEN>" | docker login <registry> -u <user> --password-stdin
```

3. Create `/opt/app/.env` with production values:

Create `/opt/app/.env` based on `.env.example` from the repository, then:

```bash
chmod 600 /opt/app/.env
chown deploy:deploy /opt/app/.env
```

Must include image references pointing to the registry:

```
APP_IMAGE=<registry>/<org>/app
MEDIAWIKI_IMAGE=<registry>/<org>/mediawiki
PHPBB_IMAGE=<registry>/<org>/phpbb
IMAGE_TAG=main
```
### GitHub repository secrets and variables

**Secrets** (Settings → Secrets and variables → Actions → Secrets):

| Name                | Value                                    |
|---------------------|------------------------------------------|
| `DEPLOY_SSH_KEY`    | Private ed25519 key from setup script    |
| `DEPLOY_HOST`       | Server IP address                        |
| `DEPLOY_USER`       | `deploy`                                 |
| `DEPLOY_SSH_PORT`   | `22` (or custom)                         |
| `DEPLOY_KNOWN_HOSTS`| Output of `ssh-keyscan` from server      |
| `DEPLOY_DOMAIN`     | Production domain name                   |
## Deploying

### Automatic (recommended)

Push to `main`. The CD workflow will:

1. Build Docker images.
2. Push to registry with `sha-<commit>` and `main` tags.
3. Stage runtime configuration under `/opt/app/.incoming/<image-tag>`.
4. Run `scripts/deployment/deploy.sh` (schema guard, promotion, pull, health checks).
5. Run smoke tests against the live site.

The staged release is promoted only after its Compose model is valid and its
MediaWiki schema generation matches `/opt/app/.mediawiki_schema_version`.
This prevents a new bind-mounted `LocalSettings.php` from becoming live before
the deployment guard runs.

### Manual trigger

Go to Actions → CD → Run workflow.

### Manual SSH deploy

```bash
ssh deploy@<server-ip>
IMAGE_TAG=sha-abc1234 bash /opt/app/scripts/deployment/deploy.sh
```

## Rollback

The deploy script saves the previous tag in `/opt/app/.last_release`.

### Automatic rollback

If health checks fail during deploy, the script automatically rolls back to
the previous tag.

### Manual rollback

```bash
ssh deploy@<server-ip>

# Check what was previously deployed
cat /opt/app/.last_release

# Rollback to that tag
IMAGE_TAG=$(cat /opt/app/.last_release) bash /opt/app/scripts/deployment/deploy.sh
```

Or rollback to any known good tag:

```bash
IMAGE_TAG=sha-<known-good-commit> bash /opt/app/scripts/deployment/deploy.sh
```

Normal deployments restore the previous runtime configuration as well as the
previous image tag. A MediaWiki schema migration has its own rollback described
below; never start the old MediaWiki image against a migrated database.

## MediaWiki major-version migration

Changing `docker/mediawiki/schema-version` causes ordinary deployment to exit
before making any production changes. The image is still built and published.
Use the **MediaWiki schema migration** workflow for the guarded cutover:

1. Run `prepare` on the same `main` revision whose automatic deployment was
   blocked. It verifies free space and both images, drains jobs, stops only the
   wiki, writes and verifies a MediaWiki-only archive, runs the explicit updater,
   and starts the new wiki writable.
2. Check the public wiki, representative pages and files, Special:Version, and
   a real SSO login with the expected MediaWiki groups. The app and forum stay
   online; Caddy returns an HTTP 503 with `Retry-After` while the wiki is stopped.
3. Run `finalize` to commit the release/schema markers, or run `rollback` to
   restore only the MediaWiki database and upload volume.

The wiki accepts writes as soon as `prepare` succeeds. A rollback restores the
pre-migration snapshot, so it discards every wiki edit, upload, SSO mapping, and
other wiki change made after that snapshot. Keep validation short and finalize
promptly when it passes.

The operation lock also blocks daily backup and ordinary deployment. Interrupted
work leaves `/opt/app/.mediawiki_migration_state`; rerun `rollback`, or rerun
`finalize` if finalization had already begun. Rollback is intentionally disabled
once finalization begins; subsequent problems require a forward fix or a new
maintenance decision because restoring could discard production edits.

The final archive, checksum, and migration log are stored in
`/opt/app/data/mediawiki-upgrade-backups` and are never removed by the seven-file
daily retention. Keep them for at least seven clean production days.

### Rehearsal checklist

Before `prepare`, restore a recent production backup into a separate Compose
project and volumes. Start it on the current immutable image, record page,
revision, user, upload, and `openid_connect` counts, then run the target
`mediawiki-migrate` service. Test reads, history, search, SSO, group mapping,
editing, PDF/SVG uploads, existing media, and dark mode. Finally exercise the
MediaWiki-only restore and prove the previous image starts before repeating the
migration from a fresh restore.

## Daily backups

The production host runs a one-shot PostgreSQL 16 backup container every day at
06:00 UTC. It writes timestamped archives to the `backup_data` Docker volume and
retains the newest seven completed files. The application reads that volume at
`data/backups` for the admin-only Maintenance listing and downloads.

After the first deployment containing the backup files, install the host timer
once as root:

```bash
sudo bash /opt/app/scripts/deployment/install-backup-timer.sh
```

Run a backup immediately and inspect its logs with:

```bash
sudo systemctl start vgindex-backup.service
journalctl -u vgindex-backup.service -n 100 --no-pager
systemctl list-timers vgindex-backup.timer --no-pager
```

The timer and the deployment script share `/opt/app/.operation.lock`, so a
deployment and a backup wait for one another instead of running concurrently.

For local testing, generate a backup from the running Compose stack:

```bash
docker compose --profile backup run --rm backup
```

Download the archive through the administrator UI or copy it out of the backup
volume, then import it into an initialized local stack:

```bash
./scripts/backups/restore-local.sh /path/to/vgindex-backup-YYYYMMDDTHHMMSSZ.tar.gz
```

The restore command replaces the three local databases and the persisted phpBB
and MediaWiki content volumes. It preserves the local phpBB signing-key volume,
refreshes phpBB's OIDC configuration from the local `.env`, and restarts the
web services. It prints the failing phase and returns the original nonzero exit
code if a command fails; it does not attempt recovery.

MediaWiki migration snapshots use `BACKUP_SCOPE=mediawiki` and require
`RESTORE_SCOPE=mediawiki`; the restore script validates the archive manifest and
cannot replace the app or phpBB databases in that mode.
