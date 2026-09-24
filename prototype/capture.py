"""Capture both dashboard-layout options against Yard's real CLI and Web server.

Run with TMPDIR=/cache/tmp CARGO_TARGET_DIR=/cache/yard-dashboard-target
and built binaries in that target. Fixture files are created only under /cache/tmp.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from tempfile import TemporaryDirectory
from time import sleep
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = Path('/cache/tmp/yard-dashboard-captures')
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', '/cache/yard-dashboard-target')) / 'debug'


class Health(BaseHTTPRequestHandler):
    def do_GET(self):
        status = 503 if self.path.startswith('/wiki') else 200
        self.send_response(status)
        self.end_headers()
        self.wfile.write(b'health check')

    def log_message(self, *_args):
        pass


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def run():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='yard-layout-', dir='/cache/tmp') as temp:
        root = Path(temp)
        projects = root / 'projects'
        state = root / 'state'
        bin_dir = root / 'bin'
        for directory in (projects, state, bin_dir, root / 'hello-api', root / 'wiki'):
            directory.mkdir()
        health = ThreadingHTTPServer(('127.0.0.1', 0), Health)
        health_thread = Thread(target=health.serve_forever, daemon=True)
        health_thread.start()
        for name, service in [('hello-api', 'api'), ('wiki', 'wiki')]:
            (projects / f'{name}.toml').write_text(
                f'repo = "{ROOT}"\nbranch = "dashboard-layout"\n'
                f'[compose]\ndirectory = "{root / name}"\nfile = "compose.yml"\n'
                'env_file = "app.env"\n'
                f'service = "{service}"\n[image]\nname = "{name}"\n'
                f'tag_env = "APP_TAG"\n[deployment]\nhealth_url = "http://127.0.0.1:{health.server_port}/{name}"\n'
            )
        docker = bin_dir / 'docker'
        docker.write_text('''#!/bin/sh
case "$1 $2" in
  'system df') printf '%s\\n' '{"Type":"Images","Size":"2.4GB"}' '{"Type":"Containers","Size":"12MB"}' '{"Type":"Local Volumes","Size":"4GB"}' ;;
  'compose --env-file') case "$PWD" in
    */hello-api) printf '%s\\n' '{"Service":"api","State":"running"}' '{"Service":"worker","State":"exited"}' ;;
    */wiki) printf '%s\\n' '{"Service":"wiki","State":"running"}' ;;
    *) exit 2 ;;
  esac ;;
  *) exit 2 ;;
esac
''')
        docker.chmod(0o755)
        env = dict(os.environ, PATH=f'{bin_dir}:{os.environ["PATH"]}', TMPDIR='/cache/tmp')
        command = [str(TARGET / 'yard'), '--projects-dir', str(projects), '--state-dir', str(state), 'host']
        collected = subprocess.run(command, env=env, check=True, text=True, capture_output=True)
        snapshot = json.loads((state / 'host.json').read_text())
        assert [(c['project'], c['service'], c['state']) for c in snapshot['containers']] == [
            ('hello-api', 'api', 'running'), ('hello-api', 'worker', 'exited'), ('wiki', 'wiki', 'running')
        ]
        print('CLI:', ' '.join(command), '\n', collected.stdout, flush=True)
        print('Snapshot disk statuses:', [d['status'] for d in snapshot['disks']], flush=True)
        web_port = port()
        web_env = dict(env, YARD_PROJECTS_DIR=str(projects), YARD_STATE_DIR=str(state),
                       YARD_WEB_STATIC=str(ROOT / 'web/static'), YARD_WEB_HOST='127.0.0.1',
                       YARD_WEB_PORT=str(web_port), YARD_WEB_CACHE_SECONDS='0',
                       YARD_WEB_HOST_MAX_AGE_SECONDS='3600')
        server = subprocess.Popen([str(TARGET / 'yard-web')], env=web_env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            url = f'http://127.0.0.1:{web_port}'
            for _ in range(40):
                try:
                    with urlopen(url + '/healthz', timeout=1):
                        break
                except OSError:
                    sleep(.1)
            else:
                raise RuntimeError('Yard Web did not start')
            with urlopen(url + '/api/status') as response:
                payload = json.load(response)
            assert payload['host']['status'] == 'available'
            assert [(p['name'], p['status']) for p in payload['projects']] == [
                ('hello-api', 'operational'), ('wiki', 'down')]
            print('API:', [(p['name'], p['status']) for p in payload['projects']], flush=True)
            subprocess.run(['node', str(ROOT / 'prototype/capture.mjs'), url, str(OUTPUT)],
                           env=web_env, check=True)
        finally:
            server.terminate()
            server.wait(timeout=5)
            health.shutdown()
            health_thread.join(timeout=5)


if __name__ == '__main__':
    run()
