import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

const read = name => readFileSync(new URL(`../../${name}`, import.meta.url), 'utf8')
const installer = read('install-web.sh')

function caddyBlock (token) {
  const dir = mkdtempSync(join(tmpdir(), 'yard-wiring-'))
  try {
    const script = installer.match(/python3 <<'PY'\n([\s\S]*?)\nPY\n/)[1]
    const caddyfile = join(dir, 'Caddyfile')
    writeFileSync(caddyfile, 'other.example.com {\n    respond ok\n}\n')
    const env = {
      ...process.env, CADDYFILE: caddyfile, DOMAIN: 'status.example.com', AUTH_USER: 'corentin',
      CADDY_HASH: '$2a$14$hash', AUTH_DIRECTIVE: 'basic_auth', PROXY_TOKEN: token
    }
    for (let i = 0; i < 2; i++) { // second run proves the block is replaced, not duplicated
      const result = spawnSync('python3', ['-c', script], { env, encoding: 'utf8' })
      assert.equal(result.status, 0, result.stderr)
    }
    return readFileSync(caddyfile, 'utf8')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

test('Caddy block overwrites identity headers with the Basic Auth user and the proxy token', () => {
  const token = 'a'.repeat(64)
  const text = caddyBlock(token)
  assert.equal(text.match(/# BEGIN YARD/g).length, 1)
  assert.match(text, /other\.example\.com/)
  // Both the SSE and the regular handler overwrite the identity headers.
  const proxies = text.match(/reverse_proxy yard-web:8088 \{[^}]*\{http\.auth\.user\.id\}[^}]*\}/g)
  assert.equal(proxies.length, 2)
  for (const proxy of proxies) {
    assert.match(proxy, /header_up X-Yard-User \{http\.auth\.user\.id\}\n\s+header_up X-Yard-Proxy-Token a{64}/)
  }
  // header_up must replace (no "+" or "-" prefix), otherwise a client value could pass through.
  assert.doesNotMatch(text, /header_up [+-]/)
})

test('proxy token is generated once, root-only, reused, and wired into Compose', () => {
  assert.match(installer, /YARD_WEB_ENV="\$\{YARD_ROOT\}\/web\.env"/)
  assert.match(installer, /secrets\.token_hex\(32\)/)
  assert.match(installer, /sed -n 's\/\^YARD_WEB_PROXY_TOKEN=\/\/p' "\$YARD_WEB_ENV"/)
  assert.match(installer, /umask 077[\s\S]*?YARD_WEB_PROXY_TOKEN=%s/)
  assert.match(installer, /chmod 0600 "\$\{YARD_WEB_ENV\}\.new"/)
  assert.match(installer, /env_file:\n\s+- \$\{YARD_WEB_ENV\}/)
  assert.doesNotMatch(installer, /echo[^\n]*PROXY_TOKEN/)
  assert.doesNotMatch(installer, /YARD_WEB_PROXY_TOKEN: /) // not inlined in the world-readable compose file
})

test('ops socket env and directory mount agree with the unit', () => {
  const unit = read('ops/yard-web-ops.service')
  assert.match(installer, /YARD_WEB_OPS_SOCKET: "\/run\/yard-web-ops\/ops\.sock"/)
  assert.match(installer, /source: \/run\/yard-web-ops\n\s+target: \/run\/yard-web-ops/)
  assert.match(installer, /group_add:[\s\S]*?"\$\{OPS_GROUP_GID\}"/)
  assert.match(unit, /ops-server --socket \/run\/yard-web-ops\/ops\.sock/)
  assert.match(unit, /^Group=yard-web-ops$/m)
  assert.match(unit, /^RuntimeDirectoryPreserve=yes$/m)
  assert.match(read('web/yard-web-read.service'), /^RuntimeDirectoryPreserve=yes$/m)
})

test('backups of the Caddyfile are root-only', () => {
  assert.match(installer, /chmod 0700 "\$BACKUP_DIR"/)
  assert.match(installer, /chmod 0600 "\$BACKUP_DIR\/Caddyfile"/)
  assert.match(read('uninstall-web.sh'), /chmod 0600 "\$BACKUP_DIR\/Caddyfile"/)
})

test('update and uninstall handle the token file and the ops unit', () => {
  const update = read('update.sh')
  assert.match(update, /web\.env/)
  assert.match(update, /yard-web-ops\.service/)
  const uninstall = read('uninstall-web.sh')
  assert.match(uninstall, /rm -f "\$\{YARD_ROOT\}\/web\.env"/)
  assert.ok(uninstall.indexOf('compose -f "$YARD_COMPOSE" down') < uninstall.indexOf('web.env"'))
  assert.match(uninstall, /yard-web-ops\.service/)
})

test('job history is group-readable by the container group only, and capped', () => {
  const ops = read('ops/install-ops.sh')
  const unit = read('ops/yard-web-ops.service')
  // The installer prepares the directory for the same group it adds to the container.
  assert.match(ops, /install -d -o "\$user" -g yard-web-ops -m 0750 "\$jobs"/)
  assert.match(ops, /-exec chmod 0640/)
  assert.match(installer, /prepare_ops_jobs_dir \/var\/lib\/yard/)
  assert.ok(installer.indexOf('prepare_ops_jobs_dir') < installer.indexOf('start_ops_service\n'))
  assert.match(installer, /group_add:[\s\S]*?"\$\{OPS_GROUP_GID\}"/)
  assert.match(ops, /getent group yard-web-ops/)
  assert.match(unit, /^Group=yard-web-ops$/m)
  assert.match(unit, /^Environment=YARD_JOBS_MAX_COUNT=\d+$/m)
  assert.match(unit, /^Environment=YARD_JOBS_MAX_BYTES=\d+$/m)
})

test('ops executor runs as the deploy user that owns the state, never root', () => {
  const ops = read('ops/install-ops.sh')
  const unit = read('ops/yard-web-ops.service')
  assert.match(unit, /^User=@DEPLOY_USER@$/m)
  assert.match(unit, /^SupplementaryGroups=@DEPLOY_GROUP@$/m)
  assert.match(unit, /^CapabilityBoundingSet=$/m)
  assert.doesNotMatch(unit, /^User=root$/m)
  assert.match(ops, /stat -c '%U' "\$state"/)
  assert.match(ops, /"\$user" == root/)
  assert.match(installer, /install_ops_unit "\$\{OPS_SOURCE\}\/yard-web-ops\.service"/)
  assert.match(installer, /prepare_ops_jobs_dir \/var\/lib\/yard "\$OPS_USER"/)
  assert.match(installer, /grep -qx docker/)
  assert.doesNotMatch(installer, /install -o root -g root -m 0644 "\$\{OPS_SOURCE\}/)
})

test('Caddy block never compresses or buffers the SSE routes', () => {
  const text = caddyBlock('b'.repeat(64))
  const stream = text.match(/@stream path ([^\n]+)\n\s+handle @stream \{\n([\s\S]*?)\n    \}\n/)
  assert.ok(stream, text)
  // The matcher covers exactly the two event-stream routes of the API.
  assert.equal(stream[1], '/api/logs/stream /api/jobs/*/output')
  assert.match(stream[2], /flush_interval -1/)
  assert.doesNotMatch(stream[2], /encode/)
  // Everything else is compressed, in a handler that comes after the SSE one.
  const rest = text.match(/\n    handle \{\n\s+encode zstd gzip\n\s+reverse_proxy yard-web:8088 \{/)
  assert.ok(rest, text)
  assert.ok(text.indexOf('handle @stream') < text.indexOf(rest[0]))
  assert.equal(text.match(/encode /g).length, 1)
  assert.equal(text.match(/flush_interval/g).length, 1)
  // Same routes the Web server exposes as event streams.
  const http = read('web/server/http.rs')
  assert.match(http, /"\/api\/logs\/stream"/)
  assert.match(http, /"\/api\/jobs\/\{id\}\/output"/)
})
