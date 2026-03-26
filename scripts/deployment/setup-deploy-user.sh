#!/usr/bin/env bash
set -euo pipefail

# Run this script as root on the production server to create a dedicated
# deploy user for GitHub Actions CD.  It generates an SSH keypair; the
# private key must be added to the GitHub repo as the DEPLOY_SSH_KEY secret.
#
# Usage:  bash scripts/deployment/setup-deploy-user.sh

DEPLOY_USER="deploy"
KEY_COMMENT="github-actions-deploy"

if [[ $(id -u) -ne 0 ]]; then
    echo "ERROR: run this script as root" >&2
    exit 1
fi

if id "$DEPLOY_USER" &>/dev/null; then
    echo "User '$DEPLOY_USER' already exists — skipping creation."
else
    useradd --create-home --shell /usr/sbin/nologin "$DEPLOY_USER"
    echo "Created user '$DEPLOY_USER' (shell disabled)."
fi

usermod -aG docker "$DEPLOY_USER"
echo "Added '$DEPLOY_USER' to docker group."

SSH_DIR="/home/$DEPLOY_USER/.ssh"
mkdir -p "$SSH_DIR"
chmod 700 "$SSH_DIR"

KEY_PATH="$SSH_DIR/deploy_ed25519"
if [[ ! -f "$KEY_PATH" ]]; then
    ssh-keygen -t ed25519 -C "$KEY_COMMENT" -f "$KEY_PATH" -N ""
    cat "$KEY_PATH.pub" >> "$SSH_DIR/authorized_keys"
    chmod 600 "$SSH_DIR/authorized_keys"
    echo ""
    echo "========================================="
    echo "DEPLOY PRIVATE KEY (add to GitHub secret DEPLOY_SSH_KEY):"
    echo "========================================="
    cat "$KEY_PATH"
    echo "========================================="
    echo ""
    echo "After copying the key, delete the private key from the server:"
    echo "  rm $KEY_PATH"
else
    echo "SSH key already exists at $KEY_PATH — skipping."
fi

chown -R "$DEPLOY_USER:$DEPLOY_USER" "$SSH_DIR"

APP_DIR="/opt/app"
mkdir -p "$APP_DIR"
chown "$DEPLOY_USER:$DEPLOY_USER" "$APP_DIR"
echo "Deploy directory: $APP_DIR (owned by $DEPLOY_USER)"

echo ""
echo "Done. Required GitHub Actions secrets:"
echo "  DEPLOY_SSH_KEY       — private key printed above"
echo "  DEPLOY_HOST          — $(hostname -I | awk '{print $1}')"
echo "  DEPLOY_USER          — $DEPLOY_USER"
echo "  DEPLOY_SSH_PORT      — 22 (or your custom SSH port)"
echo "  DEPLOY_DOMAIN        — production domain name"
echo ""
SERVER_IP=$(hostname -I | awk '{print $1}')
echo "To get the DEPLOY_KNOWN_HOSTS value, run:"
echo "  ssh-keyscan -t ed25519 $SERVER_IP"
echo ""
echo "Copy the line that looks like:"
echo "  $SERVER_IP ssh-ed25519 AAAA..."
echo "and add it as the DEPLOY_KNOWN_HOSTS GitHub secret."
