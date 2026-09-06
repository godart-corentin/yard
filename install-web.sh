#!/usr/bin/env bash
set -Eeuo pipefail

die() {
  echo "yard web install: $*" >&2
  exit 1
}

[[ "${EUID}" -eq 0 ]] || die "run with sudo"
[[ "$#" -eq 1 ]] || die "usage: sudo ./install-web.sh <domain>"

DOMAIN="$1"
[[ "$DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] || die "invalid domain"

SOURCE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB_SOURCE="${SOURCE_DIR}/web"
YARD_ROOT="/opt/yard"
YARD_COMPOSE="${YARD_ROOT}/docker-compose.yml"
CADDY_CONTAINER="${YARD_CADDY_CONTAINER:-caddy}"
AUTH_USER="${YARD_WEB_USER:-yard}"

command -v docker >/dev/null || die "docker not found"
command -v python3 >/dev/null || die "python3 not found"
docker compose version >/dev/null 2>&1 || die "docker compose plugin unavailable"
[[ -f "${WEB_SOURCE}/Dockerfile" ]] || die "web source not found at ${WEB_SOURCE}"
[[ -d /etc/yard/projects ]] || die "/etc/yard/projects does not exist; install Yard first"
[[ -d /var/lib/yard ]] || die "/var/lib/yard does not exist; install Yard first"
docker inspect "$CADDY_CONTAINER" >/dev/null 2>&1 || die "running Caddy container '$CADDY_CONTAINER' not found"

CADDYFILE="$(
  docker inspect "$CADDY_CONTAINER" \
    --format '{{range .Mounts}}{{if eq .Destination "/etc/caddy/Caddyfile"}}{{.Source}}{{end}}{{end}}'
)"
[[ -f "$CADDYFILE" ]] || die "cannot locate host Caddyfile mount"

CADDY_IMAGE="$(docker inspect "$CADDY_CONTAINER" --format '{{.Config.Image}}')"
CADDY_PROJECT="$(
  docker inspect "$CADDY_CONTAINER" \
    --format '{{ index .Config.Labels "com.docker.compose.project" }}'
)"

mapfile -t CADDY_NETWORKS < <(
  docker inspect "$CADDY_CONTAINER" \
    --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{println}}{{end}}' \
    | sed '/^[[:space:]]*$/d'
)
[[ "${#CADDY_NETWORKS[@]}" -gt 0 ]] || die "Caddy has no Docker networks"

PROXY_NETWORK="${YARD_PROXY_NETWORK:-}"
if [[ -n "$PROXY_NETWORK" ]]; then
  printf '%s\n' "${CADDY_NETWORKS[@]}" | grep -Fxq "$PROXY_NETWORK" \
    || die "Caddy is not attached to YARD_PROXY_NETWORK=$PROXY_NETWORK"
else
  for network in "${CADDY_NETWORKS[@]}"; do
    compose_network="$(
      docker network inspect "$network" \
        --format '{{ index .Labels "com.docker.compose.network" }}' 2>/dev/null || true
    )"
    compose_project="$(
      docker network inspect "$network" \
        --format '{{ index .Labels "com.docker.compose.project" }}' 2>/dev/null || true
    )"
    if [[ "$compose_network" == "default" ]] \
      && { [[ -z "$CADDY_PROJECT" ]] || [[ "$compose_project" == "$CADDY_PROJECT" ]]; }; then
      PROXY_NETWORK="$network"
      break
    fi
  done
fi

if [[ -z "$PROXY_NETWORK" ]]; then
  for network in "${CADDY_NETWORKS[@]}"; do
    internal="$(docker network inspect "$network" --format '{{.Internal}}' 2>/dev/null || true)"
    if [[ "$internal" == "false" ]]; then
      PROXY_NETWORK="$network"
      break
    fi
  done
fi

[[ -n "$PROXY_NETWORK" ]] || die "cannot determine a reusable Caddy Docker network; set YARD_PROXY_NETWORK explicitly"

echo "Yard Web will use Caddy network: ${PROXY_NETWORK}"

STATE_GID="$(stat -c '%g' /var/lib/yard)"
mkdir -p "$YARD_ROOT"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
TMP_COMPOSE="${TMP_DIR}/docker-compose.yml"
TMP_CADDYFILE="${TMP_DIR}/Caddyfile"

cat >"$TMP_COMPOSE" <<EOF
services:
  yard-web:
    build:
      context: ${WEB_SOURCE}
      dockerfile: Dockerfile
    image: yard-web:local
    container_name: yard-web
    restart: unless-stopped
    init: true
    user: "65532:65532"
    group_add:
      - "${STATE_GID}"
    environment:
      YARD_WEB_HOST: "0.0.0.0"
      YARD_WEB_PORT: "8088"
      YARD_PROJECTS_DIR: "/etc/yard/projects"
      YARD_STATE_DIR: "/var/lib/yard"
      YARD_WEB_STATIC: "/opt/yard/static"
      YARD_WEB_CHECK_TIMEOUT_SECONDS: "4"
      YARD_WEB_CACHE_SECONDS: "15"
    volumes:
      - type: bind
        source: /etc/yard/projects
        target: /etc/yard/projects
        read_only: true
      - type: bind
        source: /var/lib/yard
        target: /var/lib/yard
        read_only: true
    read_only: true
    tmpfs:
      - /tmp:size=16m,mode=1777
    cap_drop:
      - ALL
    security_opt:
      - no-new-privileges:true
    pids_limit: 128
    mem_limit: 128m
    cpus: 0.50
    healthcheck:
      test:
        - CMD
        - python3
        - -c
        - "import urllib.request; urllib.request.urlopen('http://127.0.0.1:8088/healthz', timeout=2).read()"
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 5s
    networks:
      - caddy_proxy

networks:
  caddy_proxy:
    external: true
    name: ${PROXY_NETWORK}
EOF

docker compose -f "$TMP_COMPOSE" config >/dev/null

echo "Choose the password for https://${DOMAIN}"
echo "Username: ${AUTH_USER}"
read -rsp "Password: " PASSWORD
echo
read -rsp "Confirm password: " PASSWORD2
echo
[[ -n "$PASSWORD" ]] || die "password cannot be empty"
[[ "$PASSWORD" == "$PASSWORD2" ]] || die "passwords do not match"

CADDY_HASH="$(
  docker exec \
    -e YARD_PASSWORD="$PASSWORD" \
    "$CADDY_CONTAINER" \
    sh -c 'caddy hash-password --plaintext "$YARD_PASSWORD"'
)"
unset PASSWORD PASSWORD2
[[ -n "$CADDY_HASH" ]] || die "Caddy generated an empty password hash"

cp "$CADDYFILE" "$TMP_CADDYFILE"

write_caddy_block() {
  local directive="$1"
  DOMAIN="$DOMAIN" \
  AUTH_USER="$AUTH_USER" \
  CADDY_HASH="$CADDY_HASH" \
  AUTH_DIRECTIVE="$directive" \
  CADDYFILE="$TMP_CADDYFILE" \
  python3 <<'PY'
import os
import re
from pathlib import Path

path = Path(os.environ["CADDYFILE"])
text = path.read_text(encoding="utf-8")
text = re.sub(r"\n?# BEGIN YARD\n.*?# END YARD\n?", "\n", text, flags=re.DOTALL)
block = f"""# BEGIN YARD
{os.environ['DOMAIN']} {{
    {os.environ['AUTH_DIRECTIVE']} {{
        {os.environ['AUTH_USER']} {os.environ['CADDY_HASH']}
    }}

    encode zstd gzip
    reverse_proxy yard-web:8088
}}
# END YARD
"""
path.write_text(text.rstrip() + "\n\n" + block, encoding="utf-8")
PY
}

validate_caddy() {
  docker run --rm \
    -v "${TMP_CADDYFILE}:/etc/caddy/Caddyfile:ro" \
    "$CADDY_IMAGE" \
    caddy validate --config /etc/caddy/Caddyfile >/dev/null 2>&1
}

write_caddy_block "basic_auth"
if ! validate_caddy; then
  cp "$CADDYFILE" "$TMP_CADDYFILE"
  write_caddy_block "basicauth"
  validate_caddy || {
    docker run --rm \
      -v "${TMP_CADDYFILE}:/etc/caddy/Caddyfile:ro" \
      "$CADDY_IMAGE" \
      caddy validate --config /etc/caddy/Caddyfile
    die "generated Caddyfile is invalid"
  }
fi

BACKUP_DIR="${YARD_ROOT}/backups/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"
cp "$CADDYFILE" "$BACKUP_DIR/Caddyfile"
if [[ -f "$YARD_COMPOSE" ]]; then
  cp "$YARD_COMPOSE" "$BACKUP_DIR/docker-compose.yml"
fi

install -o root -g root -m 0644 "$TMP_COMPOSE" "$YARD_COMPOSE"
install -o root -g root -m 0644 "$TMP_CADDYFILE" "$CADDYFILE"

docker compose -f "$YARD_COMPOSE" up -d --build

docker exec "$CADDY_CONTAINER" \
  caddy reload --config /etc/caddy/Caddyfile >/dev/null

for _ in $(seq 1 30); do
  HEALTH="$(
    docker inspect yard-web \
      --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}starting{{end}}' 2>/dev/null || true
  )"
  if [[ "$HEALTH" == "healthy" ]]; then
    echo "Yard Web installed: https://${DOMAIN}"
    echo "Username: ${AUTH_USER}"
    exit 0
  fi
  [[ "$HEALTH" != "unhealthy" ]] || {
    docker logs --tail 100 yard-web >&2 || true
    die "yard-web became unhealthy"
  }
  sleep 1
done

docker logs --tail 100 yard-web >&2 || true
die "timed out waiting for yard-web"
