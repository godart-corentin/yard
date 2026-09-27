import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync, chmodSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'node:net'
import { test } from 'node:test'

const installer = new URL('../../install-web.sh', import.meta.url)
const unit = new URL('../yard-web-read.service', import.meta.url)
const helper = new URL('../install-read.sh', import.meta.url).pathname

function fixture (t) {
  const dir = mkdtempSync(join(tmpdir(), 'yard-read-install-'))
  t.after(() => rmSync(dir, { recursive: true, force: true }))
  return dir
}

function bash (script, env) {
  return spawnSync('bash', ['-c', `source "${helper}"; ${script}`], {
    env: { ...process.env, ...env }, encoding: 'utf8'
  })
}

test('installer provisions a dedicated NSS group once and passes its host GID to Web', t => {
  const dir = fixture(t)
  const group = join(dir, 'group')
  const calls = join(dir, 'calls')
  const script = `
    getent() { [[ "$1" == group && "$2" == yard-web-read && -f "$GROUP_FILE" ]] && printf 'yard-web-read:x:32001:\\n'; }
    groupadd() { printf '%s\\n' "$*" >> "$CALLS_FILE"; touch "$GROUP_FILE"; }
    if getent group 65532; then exit 42; fi
    first=$(read_group_gid); second=$(read_group_gid)
    printf '%s %s\\n' "$first" "$second"
  `
  const result = bash(script, { GROUP_FILE: group, CALLS_FILE: calls })
  assert.equal(result.status, 0, result.stderr)
  assert.equal(result.stdout.trim(), '32001 32001')
  assert.equal(readFileSync(calls, 'utf8').trim(), '--system yard-web-read')
  assert.match(readFileSync(installer, 'utf8'), /group_add:[\s\S]*?"\$\{READ_GROUP_GID\}"/)
  assert.match(readFileSync(unit, 'utf8'), /^Group=yard-web-read$/m)
  assert.match(readFileSync(unit, 'utf8'), /^RuntimeDirectoryMode=0750$/m)
  assert.match(readFileSync(new URL('../../src/read_server.rs', import.meta.url), 'utf8'), /from_mode\(0o660\)/)
  assert.doesNotMatch(readFileSync(installer, 'utf8'), /- "65532"/)
})

test('invalid or unprovisionable NSS group never yields a GID', () => {
  const missing = bash('getent() { return 2; }; groupadd() { return 1; }; read_group_gid')
  assert.notEqual(missing.status, 0)
  assert.equal(missing.stdout, '')
  const invalid = bash('getent() { printf "yard-web-read:x:root:\\n"; }; read_group_gid')
  assert.notEqual(invalid.status, 0)
  assert.equal(invalid.stdout, '')
})

test('an existing root group alias cannot grant Web the root GID', () => {
  const result = bash('getent() { printf "yard-web-read:x:0:\\n"; }; read_group_gid')
  assert.notEqual(result.status, 0)
  assert.equal(result.stdout, '')
})

test('a synchronously failing service start reports its result', t => {
  const dir = fixture(t)
  const systemctl = join(dir, 'systemctl')
  writeFileSync(systemctl, '#!/bin/sh\nif [ "$1" = show ]; then echo "Result=exit-code ExecMainStatus=216/GROUP"; exit 0; fi\nif [ "$1" = restart ]; then exit 1; fi\nexit 0\n')
  chmodSync(systemctl, 0o755)
  const result = bash('start_read_service', { PATH: `${dir}:${process.env.PATH}` })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /read executor service failed/)
  assert.match(result.stderr, /216\/GROUP/)
  assert.match(readFileSync(installer, 'utf8'), /start_read_service/)
})

test('socket readiness accepts a delayed Unix socket, not an ordinary file', async t => {
  const dir = fixture(t)
  const socket = join(dir, 'read.sock')
  const systemctl = join(dir, 'systemctl')
  writeFileSync(systemctl, '#!/bin/sh\nexit 1\n') // is-failed: no
  chmodSync(systemctl, 0o755)
  const env = { ...process.env, PATH: `${dir}:${process.env.PATH}` }
  const child = spawn('bash', ['-c', `source "${helper}"; wait_read_socket "$1" 12 0.05`, '_', socket], { env })
  const finished = new Promise(resolve => child.on('close', resolve))
  let output = ''
  child.stderr.on('data', chunk => { output += chunk })
  await new Promise(resolve => setTimeout(resolve, 120))
  const server = createServer()
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(socket, resolve) })
  t.after(() => server.close())
  const exit = await finished
  assert.equal(exit, 0, output)

  const ordinary = join(dir, 'ordinary')
  writeFileSync(ordinary, '')
  const rejected = bash(`wait_read_socket "${ordinary}" 2 0.01`, { PATH: env.PATH })
  assert.notEqual(rejected.status, 0)
  assert.match(rejected.stderr, /timed out waiting for read executor socket/)
})

test('failed service exits promptly with a useful diagnosis', t => {
  const dir = fixture(t)
  const systemctl = join(dir, 'systemctl')
  writeFileSync(systemctl, '#!/bin/sh\nif [ "$1" = is-failed ]; then exit 0; fi\necho "Result=exit-code ExecMainStatus=216/GROUP"\n')
  chmodSync(systemctl, 0o755)
  const start = Date.now()
  const result = bash(`wait_read_socket "${join(dir, 'missing.sock')}" 20 0.1`, { PATH: `${dir}:${process.env.PATH}` })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /read executor service failed/)
  assert.match(result.stderr, /216\/GROUP/)
  assert.ok(Date.now() - start < 1000)
})

test('missing socket times out with the service status', t => {
  const dir = fixture(t)
  const systemctl = join(dir, 'systemctl')
  writeFileSync(systemctl, '#!/bin/sh\nif [ "$1" = is-failed ]; then exit 1; fi\necho "ActiveState=active"\n')
  chmodSync(systemctl, 0o755)
  const result = bash(`wait_read_socket "${join(dir, 'missing.sock')}" 2 0.01`, { PATH: `${dir}:${process.env.PATH}` })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /timed out waiting for read executor socket/)
  assert.match(result.stderr, /ActiveState=active/)
})
