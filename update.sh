#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "${EUID}" -eq 0 ]]; then
  "$ROOT_DIR/install.sh"
else
  sudo "$ROOT_DIR/install.sh"
fi

if docker ps --format '{{.Names}}' 2>/dev/null | grep -Fxq 'yard-web'; then
  if [[ ! -f /etc/systemd/system/yard-web-read.service ]]; then
    echo "Yard Web read-only executor is not installed. Run install-web.sh to migrate the deployment."
    exit 0
  fi
  if [[ "${EUID}" -eq 0 ]]; then
    systemctl restart yard-web-read.service
  else
    sudo systemctl restart yard-web-read.service
  fi
  # Restarting the ops executor ends running jobs (marked failed on start): do it only when idle.
  if [[ -f /etc/systemd/system/yard-web-ops.service ]]; then
    if compgen -G '/var/lib/yard/jobs/*.pid' >/dev/null; then
      echo "Yard Web jobs are running; restart yard-web-ops.service later to use the new CLI."
    elif [[ "${EUID}" -eq 0 ]]; then
      systemctl restart yard-web-ops.service
    else
      sudo systemctl restart yard-web-ops.service
    fi
  else
    echo "Yard Web ops executor is not installed. Run install-web.sh to enable Web actions."
  fi
  if [[ -f /opt/yard/docker-compose.yml ]] \
    && grep -q '^[[:space:]]*build:' /opt/yard/docker-compose.yml
  then
    docker compose \
      -f /opt/yard/docker-compose.yml \
      up -d --build yard-web
    echo "Rebuilt yard-web with the updated Rust server and frontend."
  else
    echo "Yard Web update staged. Run install-web.sh once to migrate the deployment."
  fi
fi
