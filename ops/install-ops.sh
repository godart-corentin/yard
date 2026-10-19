#!/usr/bin/env bash
# Sourced by install-web.sh; mirrors web/install-read.sh for the ops executor.

ops_group_gid() {
  local entry name password gid members
  if ! entry="$(getent group yard-web-ops)"; then
    groupadd --system yard-web-ops >&2 || return 1
    entry="$(getent group yard-web-ops)" || {
      echo 'yard web install: created ops group is missing from NSS' >&2
      return 1
    }
  fi
  IFS=: read -r name password gid members <<< "$entry"
  if [[ "$name" != yard-web-ops || ! "$gid" =~ ^[0-9]+$ || "$gid" =~ ^0+$ ]]; then
    echo 'yard web install: invalid NSS entry for yard-web-ops group' >&2
    return 1
  fi
  printf '%s\n' "$gid"
}

# The executor runs as the deploy user, the owner of the state directory: the user whose `yard`
# commands own the repositories, Compose files and backups. Root is refused, since Git rejects
# repositories owned by another user and root-written files would break that user's next CLI run.
ops_deploy_user() {
  local state="${1:-/var/lib/yard}" user group
  user="$(stat -c '%U' "$state")" || return 1
  if [[ "$user" == root || "$user" == UNKNOWN || ! "$user" =~ ^[a-z_][a-z0-9_.-]*$ ]]; then
    echo "yard web install: ${state} must be owned by the deploy user, not ${user}" >&2
    return 1
  fi
  group="$(id -gn "$user")" || return 1
  if [[ ! "$group" =~ ^[a-z_][a-z0-9_.-]*$ ]]; then
    echo "yard web install: invalid primary group for ${user}" >&2
    return 1
  fi
  printf '%s %s\n' "$user" "$group"
}

install_ops_unit() {
  local source="$1" target="$2" user="$3" group="$4"
  sed -e "s/@DEPLOY_USER@/${user}/g" -e "s/@DEPLOY_GROUP@/${group}/g" "$source" > "${target}.new" \
    && chown root:root "${target}.new" && chmod 0644 "${target}.new" \
    && mv -f "${target}.new" "$target"
}

# Job history readable by the yard-web-ops group (the Web container's supplementary group, through
# its read-only /var/lib/yard mount) and by nobody else. The executor enforces the same layout when
# it starts; doing it here also covers files of an older install before the new unit runs. Files an
# older root executor left (job history, state written by a Web job) go back to the deploy user.
prepare_ops_jobs_dir() {
  local state="${1:-/var/lib/yard}" user="$2" jobs
  jobs="${state}/jobs"
  install -d -o "$user" -g yard-web-ops -m 0750 "$jobs" || return 1
  chown "$user" "$jobs" || return 1
  find "$jobs" -maxdepth 1 -type f \( -name '*.json' -o -name '*.log' -o -name '*.pid' \) \
    -exec chown "${user}:yard-web-ops" {} + || return 1
  find "$jobs" -maxdepth 1 -type f \( -name '*.json' -o -name '*.log' \) \
    -exec chmod 0640 {} + || return 1
  find "$state" -maxdepth 1 -type f -name '*.json' -user root -exec chown "${user}:" {} + || return 1
}

ops_service_status() {
  systemctl show yard-web-ops.service \
    --property=ActiveState --property=Result --property=ExecMainStatus >&2 || true
}

start_ops_service() {
  systemctl daemon-reload || return 1
  if ! systemctl enable --now yard-web-ops.service \
    || ! systemctl restart yard-web-ops.service; then
    ops_service_status
    echo 'yard web install: ops executor service failed to start' >&2
    return 1
  fi
}

wait_ops_socket() {
  local socket="$1" attempts="${2:-20}" interval="${3:-0.5}" attempt
  for ((attempt = 0; attempt < attempts; attempt++)); do
    if systemctl is-failed --quiet yard-web-ops.service; then
      ops_service_status
      echo 'yard web install: ops executor service failed' >&2
      return 1
    fi
    if [[ -S "$socket" ]]; then
      return 0
    fi
    sleep "$interval"
  done
  ops_service_status
  echo 'yard web install: timed out waiting for ops executor socket' >&2
  return 1
}
