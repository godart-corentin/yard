#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to build Yard from source." >&2
  exit 1
fi

cargo build --release

if [[ ${EUID:-$(id -u)} -eq 0 ]]; then
  SUDO=()
else
  if ! command -v sudo >/dev/null 2>&1; then
    echo "sudo is required to install Yard outside a root shell." >&2
    exit 1
  fi
  SUDO=(sudo)
fi

"${SUDO[@]}" install -d -m 755 /etc/yard/projects
"${SUDO[@]}" install -d -m 755 /var/lib/yard
"${SUDO[@]}" install -m 755 target/release/yard /usr/local/bin/yard

echo "Yard installed to /usr/local/bin/yard"
