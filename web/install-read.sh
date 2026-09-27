#!/usr/bin/env bash
# Sourced by install-web.sh; keep provisioning and readiness checks testable without host mutations.

read_group_gid() {
  local entry name password gid members
  if ! entry="$(getent group yard-web-read)"; then
    groupadd --system yard-web-read >&2 || return 1
    entry="$(getent group yard-web-read)" || {
      echo 'yard web install: created read group is missing from NSS' >&2
      return 1
    }
  fi
  IFS=: read -r name password gid members <<< "$entry"
  if [[ "$name" != yard-web-read || ! "$gid" =~ ^[0-9]+$ || "$gid" =~ ^0+$ ]]; then
    echo 'yard web install: invalid NSS entry for yard-web-read group' >&2
    return 1
  fi
  printf '%s\n' "$gid"
}

read_service_status() {
  systemctl show yard-web-read.service \
    --property=ActiveState --property=Result --property=ExecMainStatus >&2 || true
}

start_read_service() {
  systemctl daemon-reload || return 1
  if ! systemctl enable --now yard-web-read.service \
    || ! systemctl restart yard-web-read.service; then
    read_service_status
    echo 'yard web install: read executor service failed to start' >&2
    return 1
  fi
}

wait_read_socket() {
  local socket="$1" attempts="${2:-20}" interval="${3:-0.5}" attempt
  for ((attempt = 0; attempt < attempts; attempt++)); do
    if systemctl is-failed --quiet yard-web-read.service; then
      read_service_status
      echo 'yard web install: read executor service failed' >&2
      return 1
    fi
    if [[ -S "$socket" ]]; then
      return 0
    fi
    sleep "$interval"
  done
  read_service_status
  echo 'yard web install: timed out waiting for read executor socket' >&2
  return 1
}
