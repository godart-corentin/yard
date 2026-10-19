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
REPO_WEB_SOURCE="${SOURCE_DIR}/web"
INSTALLED_WEB_SOURCE="/usr/local/share/yard/web-src"
YARD_ROOT="/opt/yard"
YARD_COMPOSE="${YARD_ROOT}/docker-compose.yml"
# Root-only env file (0600) holding the proxy token; Compose reads it as root at `up` time,
# so update.sh keeps working without ever printing or exporting the token.
YARD_WEB_ENV="${YARD_ROOT}/web.env"
CADDY_CONTAINER="${YARD_CADDY_CONTAINER:-caddy}"
AUTH_USER="${YARD_WEB_USER:-}"

command -v docker >/dev/null || die "docker not found"
command -v python3 >/dev/null || die "python3 not found"
command -v systemctl >/dev/null || die "systemd is required for the local read executor"
command -v yard >/dev/null || die "install the updated Yard CLI first"
yard read-server --help >/dev/null 2>&1 || die "installed Yard CLI lacks the read-only executor; run install.sh first"
yard ops-server --help >/dev/null 2>&1 || die "installed Yard CLI lacks the ops executor; run install.sh first"
docker compose version >/dev/null 2>&1 || die "docker compose plugin unavailable"

# Prefer the source checkout when install-web.sh is run from the Yard repo.
# Yard Web is built by its Dockerfile, so Cargo does not need to be installed
# on the host. The installed copy remains a fallback for packaged installs.
if [[ -f "${REPO_WEB_SOURCE}/Dockerfile" ]]; then
  WEB_SOURCE="$REPO_WEB_SOURCE"
elif [[ -f "${INSTALLED_WEB_SOURCE}/Dockerfile" ]]; then
  WEB_SOURCE="$INSTALLED_WEB_SOURCE"
else
  die "Yard Web source not found next to install-web.sh or in ${INSTALLED_WEB_SOURCE}"
fi
source "${WEB_SOURCE}/install-read.sh"

# The ops executor files live beside the CLI sources, not in the Web image context.
if [[ -f "${SOURCE_DIR}/ops/yard-web-ops.service" ]]; then
  OPS_SOURCE="${SOURCE_DIR}/ops"
elif [[ -f /usr/local/share/yard/ops-src/yard-web-ops.service ]]; then
  OPS_SOURCE="/usr/local/share/yard/ops-src"
else
  die "ops executor files not found next to install-web.sh or in /usr/local/share/yard/ops-src"
fi
source "${OPS_SOURCE}/install-ops.sh"

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

echo "Yard Web source: ${WEB_SOURCE}"
echo "Yard Web will use Caddy network: ${PROXY_NETWORK}"

STATE_GID="$(stat -c '%g' /var/lib/yard)"
READ_GROUP_GID="$(read_group_gid)" || die "cannot provision dedicated read executor group"
OPS_GROUP_GID="$(ops_group_gid)" || die "cannot provision dedicated ops executor group"
mkdir -p "$YARD_ROOT"

# Shared secret between Caddy and yard-web: Web only honors X-Yard-User when the proxy presents it.
# Generated once and reused by re-installs and updates. Never printed.
PROXY_TOKEN=""
if [[ -f "$YARD_WEB_ENV" ]]; then
  PROXY_TOKEN="$(sed -n 's/^YARD_WEB_PROXY_TOKEN=//p' "$YARD_WEB_ENV" | head -n1)"
fi
if [[ ! "$PROXY_TOKEN" =~ ^[0-9a-f]{64}$ ]]; then
  PROXY_TOKEN="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
  [[ "$PROXY_TOKEN" =~ ^[0-9a-f]{64}$ ]] || die "cannot generate the proxy token"
fi
(
  umask 077
  printf 'YARD_WEB_PROXY_TOKEN=%s\n' "$PROXY_TOKEN" >"${YARD_WEB_ENV}.new"
)
chmod 0600 "${YARD_WEB_ENV}.new"
chown root:root "${YARD_WEB_ENV}.new"
mv -f "${YARD_WEB_ENV}.new" "$YARD_WEB_ENV"

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
      - "${READ_GROUP_GID}"
      - "${OPS_GROUP_GID}"
    env_file:
      - ${YARD_WEB_ENV}
    environment:
      YARD_WEB_HOST: "0.0.0.0"
      YARD_WEB_PORT: "8088"
      YARD_PROJECTS_DIR: "/etc/yard/projects"
      YARD_STATE_DIR: "/var/lib/yard"
      YARD_WEB_STATIC: "/opt/yard/static"
      YARD_WEB_CHECK_TIMEOUT_SECONDS: "5"
      YARD_WEB_CACHE_SECONDS: "15"
      YARD_WEB_READ_SOCKET: "/run/yard-web-read/read.sock"
      YARD_WEB_OPS_SOCKET: "/run/yard-web-ops/ops.sock"
    volumes:
      - type: bind
        source: /etc/yard/projects
        target: /etc/yard/projects
        read_only: true
      - type: bind
        source: /var/lib/yard
        target: /var/lib/yard
        read_only: true
      - type: bind
        source: /run/yard-web-read
        target: /run/yard-web-read
        read_only: true
      # Connecting to the socket needs no write access to the mount; the yard-web-ops group does it.
      - type: bind
        source: /run/yard-web-ops
        target: /run/yard-web-ops
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
        - yard-web
        - --healthcheck
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

if [[ -z "$AUTH_USER" ]]; then
  read -rp "Username: " AUTH_USER
fi
[[ "$AUTH_USER" =~ ^[A-Za-z0-9._-]+$ ]] \
  || die "username must contain only letters, numbers, dot, underscore or dash"

echo "Choose the password for https://${DOMAIN}"
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
  PROXY_TOKEN="$PROXY_TOKEN" \
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
    # header_up without +/- replaces any client-supplied value of the same header, so
    # a browser cannot forge the identity or the proxy token.
    reverse_proxy yard-web:8088 {{
        header_up X-Yard-User {{http.auth.user.id}}
        header_up X-Yard-Proxy-Token {os.environ['PROXY_TOKEN']}
    }}
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
# Backups of the Caddyfile hold the proxy token and the Basic Auth hash: root only.
mkdir -p "${YARD_ROOT}/backups"
chmod 0700 "${YARD_ROOT}/backups"
mkdir -p "$BACKUP_DIR"
chmod 0700 "$BACKUP_DIR"
cp "$CADDYFILE" "$BACKUP_DIR/Caddyfile"
chmod 0600 "$BACKUP_DIR/Caddyfile"
if [[ -f "$YARD_COMPOSE" ]]; then
  cp "$YARD_COMPOSE" "$BACKUP_DIR/docker-compose.yml"
fi

# The proxy token is written in clear in the Caddyfile (and so in Caddy's loaded config).
# Anyone who can read it, or reach the Caddy admin API, can forge a Yard Web identity.
if [[ "$(stat -c '%a' "$CADDYFILE")" =~ [0-7][0-7][1-7]$ ]]; then
  echo "warning: ${CADDYFILE} is readable by other users and now contains the Yard Web proxy token; run: chmod o-rw ${CADDYFILE}" >&2
fi

install -o root -g root -m 0644 "$TMP_COMPOSE" "$YARD_COMPOSE"

# Only the allowlisted host reader has Docker access; Web gets a local socket.
install -o root -g root -m 0644 "${WEB_SOURCE}/yard-web-read.service" /etc/systemd/system/yard-web-read.service
start_read_service
wait_read_socket /run/yard-web-read/read.sock

# The ops executor runs Docker, Git and backup commands; Web only gets its group-protected socket.
install -o root -g root -m 0644 "${OPS_SOURCE}/yard-web-ops.service" /etc/systemd/system/yard-web-ops.service
prepare_ops_jobs_dir /var/lib/yard || die "cannot prepare /var/lib/yard/jobs"
start_ops_service
wait_ops_socket /run/yard-web-ops/ops.sock

# The Caddyfile is bind-mounted as a single file. Replacing it with rename/install
# changes the inode and leaves the running container attached to the old file.
# Update the existing inode in place so Caddy sees the new contents immediately.
cat "$TMP_CADDYFILE" >"$CADDYFILE"

if ! docker exec "$CADDY_CONTAINER" \
  caddy validate --config /etc/caddy/Caddyfile >/dev/null; then
  cat "$BACKUP_DIR/Caddyfile" >"$CADDYFILE"
  die "live Caddyfile validation failed; restored previous configuration"
fi

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
    echo "Web actions (deploy, backup, restore, prune) run on the host through yard-web-ops.service."
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
