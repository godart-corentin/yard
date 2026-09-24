import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import vm from 'node:vm'

const source = readFileSync(new URL('../static/app.js', import.meta.url), 'utf8')

class Element {
  constructor (tag = 'div') {
    this.tag = tag
    this.children = []
    this.attributes = {}
    this._text = ''
    this.className = ''
    this.classList = {
      toggle: (name, enabled) => {
        const classes = new Set(this.className.split(' ').filter(Boolean))
        if (enabled) classes.add(name)
        else classes.delete(name)
        this.className = [...classes].join(' ')
      },
      remove: (name) => this.classList.toggle(name, false)
    }
  }

  set textContent (text) {
    this._text = String(text)
    this.children = []
  }

  get textContent () {
    return this._text + this.children.map(child => child.textContent).join('')
  }

  append (...children) { this.children.push(...children) }
  replaceChildren (...children) {
    this._text = ''
    this.children = children
  }

  setAttribute (name, value) { this.attributes[name] = value }
  removeAttribute (name) { delete this.attributes[name] }
  addEventListener () {}
}

const byClass = (element, name) => element.children.flatMap(child => [
  ...(child.className.split(' ').includes(name) ? [child] : []),
  ...byClass(child, name)
])

const metric = (status, value) => ({ status, value, message: null })
const payload = {
  checked_at: '2026-09-24T12:00:00Z',
  projects: [
    { name: 'hello-api', status: 'operational', offsite_configured: false },
    { name: 'wiki', status: 'down', error: 'HTTP 503', offsite_configured: false }
  ],
  host: {
    status: 'available',
    age_seconds: 7,
    snapshot: {
      cpu: metric('normal', 21.4),
      memory: metric('normal', { used_bytes: 2e9, total_bytes: 8e9 }),
      disks: [metric('normal', { mount: '/', used_bytes: 2e10, total_bytes: 1e11 })],
      load: metric('critical', [3, 2, 1]),
      docker: metric('critical', { images: '2.4GB', containers: '12MB', volumes: '4GB' }),
      containers_status: 'critical',
      containers: [
        { project: 'hello-api', service: 'api', state: 'running', status: 'normal' },
        { project: 'wiki', service: 'wiki', state: 'running', status: 'normal' },
        { project: 'hello-api', service: 'worker', state: 'exited', status: 'critical' }
      ]
    }
  }
}

function dashboard (search = '') {
  const elements = Object.fromEntries([
    'projects', 'overall-badge', 'total-count', 'operational-count', 'down-count',
    'checked-at', 'refresh', 'host-metrics', 'host-badge', 'host-age'
  ].map(id => [id, new Element()]))
  const context = {
    document: {
      querySelector: selector => elements[selector.slice(1)],
      createElement: tag => new Element(tag)
    },
    location: { search },
    fetch: () => new Promise(() => {}),
    setInterval: () => {},
    URL,
    URLSearchParams
  }
  vm.runInNewContext(`${source}\nglobalThis.renderForTest = render`, context, { filename: 'app.js' })
  context.renderForTest(payload)
  return elements
}

test('each container appears only within its own service, with its state', () => {
  const elements = dashboard()
  const cards = byClass(elements.projects, 'service-card')
  assert.equal(cards.length, 2)
  assert.deepEqual(cards.map(card => byClass(card, 'container-row').map(row => row.children[0].textContent)),
    [['api', 'worker'], ['wiki']])
  assert.deepEqual(cards.map(card => byClass(card, 'container-row').map(row => row.children[1].textContent)),
    [['running', 'Stopped'], ['running']])
  assert.deepEqual(cards.map(card => byClass(card, 'container-row').map(row => row.children[2].textContent)),
    [['Operational', 'Down'], ['Operational']])
  assert.equal(byClass(elements['host-metrics'], 'container-row').length, 0)
})

test('backup results remain separate and missing information is explicit', () => {
  const cards = byClass(dashboard().projects, 'service-card')
  const first = byClass(cards[0], 'backup-facts')[0]
  assert.match(first.textContent, /Local backupNo backup recorded/)
  assert.match(first.textContent, /Off-site copyNot configured/)
  const attempt = { started_at_unix: 1790251200, duration_ms: 42, result: 'success', destination: '/archives' }
  const failed = { ...attempt, result: 'failure', destination: 'remote:test' }
  const context = {
    document: { querySelector: () => new Element(), createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}
  }
  vm.runInNewContext(`${source}\nglobalThis.renderForTest = renderProject`, context)
  const block = context.renderForTest({ name: 'demo', status: 'operational', last_backup: attempt, last_offsite: failed, offsite_configured: true }, [])
  const facts = byClass(block, 'backup-facts')[0]
  assert.match(facts.textContent, /Local backupSuccess.*\/archives/)
  assert.match(facts.textContent, /Off-site copyFailure.*remote:test/)
  assert.equal(byClass(facts, 'bad').length, 1)
})

test('stopped worker degrades only its service and summary, not the host', () => {
  const elements = dashboard()
  const cards = byClass(elements.projects, 'service-card')
  assert.deepEqual(cards.map(card => byClass(card, 'service-header')[0].children[1].textContent),
    ['Degraded', 'Down'])
  assert.equal(elements['host-badge'].textContent, 'Normal')
  assert.equal(elements['operational-count'].textContent, '0')
  assert.equal(elements['down-count'].textContent, '2')
  assert.equal(elements['overall-badge'].textContent, 'Down')
})

test('host contains CPU, RAM, disk and measurement age, never Load or Docker cards even with a query parameter', () => {
  const elements = dashboard('?variant=b')
  assert.deepEqual(byClass(elements['host-metrics'], 'host-metric').map(card => card.children[0].children[0].textContent),
    ['CPU', 'RAM', 'Disk /'])
  assert.equal(elements['host-age'].textContent, 'Measured 7 seconds ago')
  assert.equal(byClass(elements['host-metrics'], 'host-secondary').length, 0)
  assert.equal(elements['host-badge'].textContent, 'Normal')
})

test('probes stay with their project card and display real measurements', () => {
  const elements = dashboard()
  const withServices = structuredClone(payload)
  withServices.projects[0].status = 'degraded'
  withServices.projects[0].services = [
    { service: 'api', status: 'healthy', kind: 'http', latency_ms: 42 },
    { service: 'worker', status: 'unhealthy', kind: 'heartbeat', age_seconds: 94 }
  ]
  const context = { document: { querySelector: selector => elements[selector.slice(1)], createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}, URL }
  vm.runInNewContext(`${source}\nglobalThis.renderForTest = render`, context)
  context.renderForTest(withServices)
  const cards = byClass(elements.projects, 'service-card')
  const probes = byClass(cards[0], 'service-probe')
  assert.equal(probes.length, 2)
  assert.match(probes[0].textContent, /api.*Healthy.*42 ms/)
  assert.match(probes[1].textContent, /worker.*Unhealthy.*94 s/)
  assert.equal(byClass(cards[1], 'service-probe').length, 0)
  assert.equal(elements['host-badge'].textContent, 'Normal')
})
