#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if command -v cargo >/dev/null 2>&1; then
  cargo build --release --locked --manifest-path "$ROOT_DIR/Cargo.toml"
elif command -v docker >/dev/null 2>&1; then
  echo "cargo not found; building Yard with Docker"
  docker run --rm \
    -v "$ROOT_DIR:/src" \
    -w /src \
    rust:bookworm \
    cargo build --release --locked
else
  echo "cargo or Docker is required to build Yard from source." >&2
  exit 1
fi

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
"${SUDO[@]}" install -m 755 "$ROOT_DIR/target/release/yard" /usr/local/bin/yard

"${SUDO[@]}" install -d -o root -g root -m 755 /usr/local/share/yard
"${SUDO[@]}" rm -rf /usr/local/share/yard/web-src
"${SUDO[@]}" cp -R "$ROOT_DIR/web" /usr/local/share/yard/web-src
"${SUDO[@]}" rm -rf /usr/local/share/yard/web-src/target
"${SUDO[@]}" chown -R root:root /usr/local/share/yard/web-src
"${SUDO[@]}" find /usr/local/share/yard/web-src -type d -exec chmod 0755 {} +
"${SUDO[@]}" find /usr/local/share/yard/web-src -type f -exec chmod 0644 {} +

echo "Yard installed to /usr/local/bin/yard"
echo "Yard Web source installed to /usr/local/share/yard/web-src"
