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

test('successful one-shot migration is completed without degrading its project', () => {
  const elements = dashboard()
  const completed = structuredClone(payload)
  completed.host.snapshot.containers = [
    { project: 'hello-api', service: 'api', state: 'running', status: 'normal' },
    { project: 'hello-api', service: 'migrate', state: 'exited', status: 'normal' }
  ]
  const context = {
    document: { querySelector: selector => elements[selector.slice(1)], createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}, URL
  }
  vm.runInNewContext(`${source}\nglobalThis.renderForTest = render`, context)
  context.renderForTest(completed)
  const card = byClass(elements.projects, 'service-card')[0]
  assert.equal(byClass(card, 'service-header')[0].children[1].textContent, 'Operational')
  const migration = byClass(card, 'container-row')[1]
  assert.deepEqual(migration.children.map(child => child.textContent), ['migrate', 'Completed', 'Completed'])
  assert.match(migration.children[2].className, /operational/)
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
  assert.match(facts.textContent, /Local backupSuccess.*destination hidden/)
  assert.match(facts.textContent, /Off-site copyFailure.*destination hidden/)
  assert.doesNotMatch(facts.textContent, /\/archives|remote:test/)
  assert.equal(byClass(facts, 'bad').length, 1)
})

test('unreadable records are visibly invalid, never missing or successful', () => {
  const context = {
    document: { querySelector: () => new Element(), createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}
  }
  vm.runInNewContext(`${source}\nglobalThis.renderForTest = renderProject`, context)
  const card = context.renderForTest({
    name: 'demo', status: 'operational',
    last_backup: { result: 'invalid' }, last_offsite: { result: 'invalid' },
    offsite_configured: true
  }, [])
  const facts = byClass(card, 'backup-facts')[0]
  assert.match(facts.textContent, /Local backupInvalid backup record/)
  assert.match(facts.textContent, /Off-site copyInvalid backup record/)
  assert.doesNotMatch(facts.textContent, /No backup recorded|No copy recorded/)
  assert.equal(byClass(facts, 'bad').length, 2)
  const unconfigured = context.renderForTest({
    name: 'demo', status: 'operational', last_offsite: { result: 'invalid' },
    offsite_configured: false
  }, [])
  assert.match(byClass(unconfigured, 'backup-facts')[0].textContent, /Off-site copyInvalid backup record/)
})

test('host snapshot cannot override live project status; host keeps its own critical metrics', () => {
  const elements = dashboard()
  const cards = byClass(elements.projects, 'service-card')
  assert.deepEqual(cards.map(card => byClass(card, 'service-header')[0].children[1].textContent),
    ['Operational', 'Down'])
  assert.equal(elements['host-badge'].textContent, 'Critical')
  assert.equal(elements['operational-count'].textContent, '1')
  assert.equal(elements['down-count'].textContent, '1')
  assert.equal(elements['overall-badge'].textContent, 'Degraded')
})

test('badge follows API status, not CLI alert or host snapshot', () => {
  const elements = dashboard()
  const context = {
    document: { querySelector: selector => elements[selector.slice(1)], createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}, URL
  }
  vm.runInNewContext(`${source}\nglobalThis.renderForTest = render`, context)
  const sample = structuredClone(payload)
  sample.projects = [{ name: 'reviewdesk', status: 'degraded', url_monitor: false,
    cli: { verdict: 'alert' } }]
  sample.host.snapshot.containers = [
    { project: 'reviewdesk', service: 'api', state: 'running', status: 'normal' },
    { project: 'reviewdesk', service: 'migrate', state: 'exited', status: 'normal' }
  ]
  for (const [status, verdict, label] of [
    ['degraded', 'alert', 'Degraded'],
    ['down', 'alert', 'Down'],
    ['unknown', null, 'Unknown']
  ]) {
    sample.projects[0].status = status
    sample.projects[0].cli = verdict ? { verdict, runtime: null, drift: [] } : null
    context.renderForTest(sample)
    const badge = byClass(elements.projects, 'service-header')[0].children[1]
    assert.equal(badge.textContent, label)
    assert.equal(elements['overall-badge'].textContent, label)
    assert.match(badge.className, new RegExp(status))
    assert.match(elements['overall-badge'].className, new RegExp(status))
  }
})

test('host contains CPU, load, RAM, disk and Docker with measurement age', () => {
  const elements = dashboard('?variant=b')
  assert.deepEqual(byClass(elements['host-metrics'], 'host-metric').map(card => card.children[0].children[0].textContent),
    ['CPU', 'Load (1 / 5 / 15 min)', 'RAM', 'Disk /', 'Docker disk'])
  assert.equal(elements['host-age'].textContent, 'Measured 7 seconds ago')
  assert.equal(byClass(elements['host-metrics'], 'host-secondary').length, 0)
  assert.equal(elements['host-badge'].textContent, 'Critical')
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
  assert.equal(elements['host-badge'].textContent, 'Critical')
})

test('unconfigured service probe does not create a Service health section', () => {
  const elements = dashboard()
  const withoutProbe = structuredClone(payload)
  withoutProbe.projects[0].services = [{
    service: 'api', status: 'unknown', kind: null,
    message: 'No service probe configured'
  }]
  withoutProbe.host.snapshot.containers = [
    { project: 'hello-api', service: 'api', state: 'running', status: 'normal' }
  ]
  const context = {
    document: { querySelector: selector => elements[selector.slice(1)], createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}, URL
  }
  vm.runInNewContext(`${source}\nglobalThis.renderForTest = render`, context)
  context.renderForTest(withoutProbe)
  const card = byClass(elements.projects, 'service-card')[0]
  assert.equal(byClass(card, 'service-probe').length, 0)
  assert.equal(byClass(card, 'container-row').length, 1)
  assert.equal(byClass(card, 'service-header')[0].children[1].textContent, 'Operational')
})

test('project read views expose bounded one-shot log filters and no mutation controls', () => {
  const elements = dashboard()
  elements['read-view'] = new Element()
  elements['dashboard-view'] = new Element()
  const context = { document: { querySelector: selector => elements[selector.slice(1)], createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}, URL, URLSearchParams,
    location: { hash: '#/project/hello-api/logs' } }
  vm.runInNewContext(`${source}\nglobalThis.renderReadForTest = renderReadView`, context)
  context.renderReadForTest({ ...payload, projects: [
    { ...payload.projects[0], application_services: ['api', 'worker'], url_monitor: false },
    payload.projects[1]
  ] }, 'hello-api', 'logs')
  const view = elements['read-view']
  assert.match(view.textContent, /All services.*hello-api.*Overview.*Logs.*Restore points.*Restore log/)
  assert.match(view.textContent, /Service.*Tail.*Since.*Load logs.*Follow is not available/i)
  assert.equal(byClass(view, 'read-service').length, 1)
  assert.equal(byClass(view, 'read-tail').length, 1)
  assert.equal(byClass(view, 'read-since').length, 1)
  assert.equal(byClass(view, 'read-button').length, 1)
  assert.doesNotMatch(view.textContent, /Prune selected|Restore…|Following/)
})

test('URL monitor shows an honest read-only overview and no CLI read actions', () => {
  const elements = dashboard()
  elements['read-view'] = new Element()
  elements['dashboard-view'] = new Element()
  const context = { document: { querySelector: selector => elements[selector.slice(1)], createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}, URL, URLSearchParams,
    location: { hash: '#/project/monitor/overview' } }
  vm.runInNewContext(`${source}\nglobalThis.renderReadForTest = renderReadView`, context)
  context.renderReadForTest({ ...payload, projects: [{ name: 'monitor', health_url: 'http://example.test', status: 'operational', url_monitor: true, application_services: [] }] }, 'monitor', 'overview')
  assert.match(elements['read-view'].textContent, /URL monitor.*No deployment actions/)
  assert.doesNotMatch(elements['read-view'].textContent, /Load logs|Restore points.*Load|Deploy/)
})

test('refresh removes a read view when its project leaves the inventory', () => {
  const elements = dashboard()
  elements['read-view'] = new Element()
  elements['dashboard-view'] = new Element()
  const context = { document: { querySelector: selector => elements[selector.slice(1)], createElement: tag => new Element(tag) },
    fetch: () => new Promise(() => {}), setInterval: () => {}, URL, URLSearchParams,
    location: { hash: '#/project/hello-api/logs' } }
  vm.runInNewContext(`${source}\nglobalThis.routeForTest = renderRoute`, context)
  context.routeForTest({ ...payload, projects: [{ ...payload.projects[0], application_services: ['api'] }] })
  context.routeForTest({ ...payload, projects: [] })
  assert.match(elements['read-view'].textContent, /Project not found/)
})
