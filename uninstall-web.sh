#!/usr/bin/env bash
set -Eeuo pipefail

die() {
  echo "yard web uninstall: $*" >&2
  exit 1
}

[[ "${EUID}" -eq 0 ]] || die "run with sudo"

YARD_ROOT="/opt/yard"
YARD_COMPOSE="${YARD_ROOT}/docker-compose.yml"
CADDY_CONTAINER="${YARD_CADDY_CONTAINER:-caddy}"

command -v docker >/dev/null || die "docker not found"
command -v python3 >/dev/null || die "python3 not found"
docker inspect "$CADDY_CONTAINER" >/dev/null 2>&1 || die "running Caddy container '$CADDY_CONTAINER' not found"

CADDYFILE="$(
  docker inspect "$CADDY_CONTAINER" \
    --format '{{range .Mounts}}{{if eq .Destination "/etc/caddy/Caddyfile"}}{{.Source}}{{end}}{{end}}'
)"
[[ -f "$CADDYFILE" ]] || die "cannot locate host Caddyfile mount"

BACKUP_DIR="${YARD_ROOT}/backups/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"
cp "$CADDYFILE" "$BACKUP_DIR/Caddyfile"

TMP_CADDYFILE="$(mktemp)"
trap 'rm -f "$TMP_CADDYFILE"' EXIT

CADDYFILE="$CADDYFILE" OUTPUT="$TMP_CADDYFILE" python3 <<'PY'
import os
import re
from pathlib import Path

source = Path(os.environ["CADDYFILE"])
output = Path(os.environ["OUTPUT"])
text = source.read_text(encoding="utf-8")
text = re.sub(r"\n?# BEGIN YARD\n.*?# END YARD\n?", "\n", text, flags=re.DOTALL)
output.write_text(text.rstrip() + "\n", encoding="utf-8")
PY

# Keep the inode of the single-file bind mount so the running Caddy container
# sees the updated configuration without being recreated.
cat "$TMP_CADDYFILE" >"$CADDYFILE"

if ! docker exec "$CADDY_CONTAINER" \
  caddy validate --config /etc/caddy/Caddyfile >/dev/null; then
  cat "$BACKUP_DIR/Caddyfile" >"$CADDYFILE"
  die "live Caddyfile validation failed; restored previous configuration"
fi

docker exec "$CADDY_CONTAINER" caddy reload --config /etc/caddy/Caddyfile >/dev/null

if [[ -f "$YARD_COMPOSE" ]]; then
  docker compose -f "$YARD_COMPOSE" down
  cp "$YARD_COMPOSE" "$BACKUP_DIR/docker-compose.yml"
  rm -f "$YARD_COMPOSE"
fi

echo "Yard Web removed. Yard CLI configuration and state were left untouched."
