#!/usr/bin/env bash
set -euo pipefail

if [[ $(id -u) -ne 0 ]]; then
    echo "ERROR: run this script as root" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UNIT_DIR="$SCRIPT_DIR/systemd"

install -m 0644 "$UNIT_DIR/vgindex-backup.service" /etc/systemd/system/vgindex-backup.service
install -m 0644 "$UNIT_DIR/vgindex-backup.timer" /etc/systemd/system/vgindex-backup.timer

systemctl daemon-reload
systemctl enable --now vgindex-backup.timer

echo "Installed VGIndex backup timer."
systemctl list-timers vgindex-backup.timer --no-pager
