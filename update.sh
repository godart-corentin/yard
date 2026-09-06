#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "${EUID}" -eq 0 ]]; then
  "$ROOT_DIR/install.sh"
else
  sudo "$ROOT_DIR/install.sh"
fi

if docker ps --format '{{.Names}}' 2>/dev/null | grep -Fxq 'yard-web'; then
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
