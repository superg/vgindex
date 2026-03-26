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

2. Log in to the container registry on the server so `docker compose pull` can fetch images:

```bash
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
3. Sync runtime config files to server.
4. Run `deploy.sh` (pull images, compose up, health checks).
5. Run smoke tests against the live site.

### Manual trigger

Go to Actions → CD → Run workflow.

### Manual SSH deploy

```bash
ssh deploy@<server-ip>
IMAGE_TAG=sha-abc1234 bash /opt/app/deploy.sh
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
IMAGE_TAG=$(cat /opt/app/.last_release) bash /opt/app/deploy.sh
```

Or rollback to any known good tag:

```bash
IMAGE_TAG=sha-<known-good-commit> bash /opt/app/deploy.sh
```
