#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import threading
import time
import tomllib
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

HOST = os.environ.get("YARD_WEB_HOST", "0.0.0.0")
PORT = int(os.environ.get("YARD_WEB_PORT", "8088"))
PROJECTS_DIR = Path(os.environ.get("YARD_PROJECTS_DIR", "/etc/yard/projects"))
STATE_DIR = Path(os.environ.get("YARD_STATE_DIR", "/var/lib/yard"))
STATIC_DIR = Path(os.environ.get("YARD_WEB_STATIC", "/opt/yard/static"))
CHECK_TIMEOUT = float(os.environ.get("YARD_WEB_CHECK_TIMEOUT_SECONDS", "4"))
CACHE_SECONDS = float(os.environ.get("YARD_WEB_CACHE_SECONDS", "15"))

_cache_lock = threading.Lock()
_cache_value: dict | None = None
_cache_until = 0.0


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def load_state(project: str) -> dict | None:
    path = STATE_DIR / f"{project}.json"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    current = data.get("current")
    return current if isinstance(current, dict) else None


def load_projects() -> list[dict]:
    projects: list[dict] = []
    if not PROJECTS_DIR.is_dir():
        return projects

    for path in sorted(PROJECTS_DIR.glob("*.toml")):
        project = path.stem
        try:
            config = tomllib.loads(path.read_text(encoding="utf-8"))
            deployment = config.get("deployment") or {}
            health_url = deployment.get("health_url")
            if health_url is not None and not isinstance(health_url, str):
                raise ValueError("deployment.health_url must be a string")
            projects.append(
                {
                    "name": project,
                    "health_url": health_url.strip() if health_url else None,
                    "release": load_state(project),
                }
            )
        except (OSError, tomllib.TOMLDecodeError, ValueError) as error:
            projects.append(
                {
                    "name": project,
                    "health_url": None,
                    "release": load_state(project),
                    "config_error": str(error),
                }
            )
    return projects


def check_project(project: dict) -> dict:
    result = dict(project)
    checked_at = utc_now()
    result["checked_at"] = checked_at
    result["latency_ms"] = None
    result["http_status"] = None

    if project.get("config_error"):
        result["status"] = "unknown"
        result["error"] = project["config_error"]
        return result

    health_url = project.get("health_url")
    if not health_url:
        result["status"] = "unknown"
        result["error"] = "No deployment.health_url configured"
        return result

    parsed = urlparse(health_url)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        result["status"] = "unknown"
        result["error"] = "Health URL must use http or https"
        return result

    request = urllib.request.Request(
        health_url,
        method="GET",
        headers={"User-Agent": "yard-status/1"},
    )
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=CHECK_TIMEOUT) as response:
            status = response.getcode()
            response.read(1024)
        result["latency_ms"] = round((time.monotonic() - started) * 1000)
        result["http_status"] = status
        result["status"] = "operational" if 200 <= status < 400 else "down"
        result["error"] = None
    except urllib.error.HTTPError as error:
        result["latency_ms"] = round((time.monotonic() - started) * 1000)
        result["http_status"] = error.code
        result["status"] = "down"
        result["error"] = f"HTTP {error.code}"
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        result["latency_ms"] = round((time.monotonic() - started) * 1000)
        result["status"] = "down"
        result["error"] = str(getattr(error, "reason", error))
    return result


def overall_status(projects: list[dict]) -> str:
    if not projects:
        return "unknown"
    statuses = [project.get("status") for project in projects]
    operational = statuses.count("operational")
    down = statuses.count("down")
    if operational == len(statuses):
        return "operational"
    if operational == 0 and down > 0:
        return "down"
    if down > 0:
        return "degraded"
    return "unknown"


def build_status() -> dict:
    global _cache_until, _cache_value
    now = time.monotonic()
    with _cache_lock:
        if _cache_value is not None and now < _cache_until:
            return _cache_value

    projects = load_projects()
    if projects:
        with ThreadPoolExecutor(max_workers=min(8, len(projects))) as executor:
            checked = list(executor.map(check_project, projects))
    else:
        checked = []

    payload = {
        "status": overall_status(checked),
        "checked_at": utc_now(),
        "projects": checked,
    }
    with _cache_lock:
        _cache_value = payload
        _cache_until = time.monotonic() + CACHE_SECONDS
    return payload


def safe_static_path(url_path: str) -> Path | None:
    relative = "index.html" if url_path == "/" else url_path.lstrip("/")
    candidate = (STATIC_DIR / relative).resolve()
    try:
        candidate.relative_to(STATIC_DIR.resolve())
    except ValueError:
        return None
    return candidate if candidate.is_file() else None


class Handler(BaseHTTPRequestHandler):
    server_version = "YardWeb/1"

    def do_GET(self) -> None:
        if self.path == "/healthz":
            self.send_json({"status": "ok"})
            return
        if self.path == "/api/status":
            self.send_json(build_status())
            return

        path = safe_static_path(self.path.split("?", 1)[0])
        if path is None:
            self.send_error(404)
            return

        content_type = {
            ".html": "text/html; charset=utf-8",
            ".css": "text/css; charset=utf-8",
            ".js": "text/javascript; charset=utf-8",
            ".svg": "image/svg+xml",
        }.get(path.suffix, "application/octet-stream")
        body = path.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        self.wfile.write(body)

    def send_json(self, payload: dict) -> None:
        body = (json.dumps(payload, separators=(",", ":")) + "\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt: str, *args) -> None:
        print(f"{self.address_string()} - {fmt % args}", flush=True)


if __name__ == "__main__":
    server = ThreadingHTTPServer((HOST, PORT), Handler)
    print(f"Yard Web listening on {HOST}:{PORT}", flush=True)
    server.serve_forever()
