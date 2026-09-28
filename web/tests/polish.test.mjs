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
    this.className = ''
    this.value = ''
    this._text = ''
    this.handlers = {}
    this.classList = {
      toggle: (name, on) => {
        const classes = new Set(this.className.split(' ').filter(Boolean))
        if (on) classes.add(name)
        else classes.delete(name)
        this.className = [...classes].join(' ')
      },
      remove: name => this.classList.toggle(name, false)
    }
  }

  set textContent (value) { this._text = String(value); this.children = [] }
  get textContent () { return this._text + this.children.map(child => child.textContent).join('') }
  append (...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node) } }
  replaceChildren (...nodes) { this.children = []; this._text = ''; this.append(...nodes) }
  setAttribute (key, value) { this.attributes[key] = value }
  removeAttribute (key) { delete this.attributes[key] }
  addEventListener (event, handler) { this.handlers[event] = handler }
  dispatch (event) { this.handlers[event]?.({ preventDefault () {} }) }
  remove () { this.parent.children = this.parent.children.filter(node => node !== this) }
  querySelector (selector) { return find(this, selector)[0] }
}

function find (root, selector) {
  return root.children.flatMap(child => [
    ...(selector.startsWith('.') && child.className.split(' ').includes(selector.slice(1)) ? [child] : []),
    ...find(child, selector)
  ])
}

const payload = {
  checked_at: '2026-09-27T12:00:00Z',
  projects: [
    { name: 'app', status: 'operational', application_services: ['api'], offsite_configured: false },
    { name: 'monitor', status: 'operational', url_monitor: true, health_url: 'https://example.test' }
  ],
  host: { status: 'available', age_seconds: 1, snapshot: {
    cpu: { status: 'normal', value: 1 }, memory: { status: 'normal', value: { used_bytes: 1, total_bytes: 2 } },
    disks: [], containers: [{ project: 'app', service: 'api', status: 'normal', state: 'running' }]
  } }
}

function browser (fetchRead = () => new Promise(() => {})) {
  const nodes = Object.fromEntries([
    'projects', 'overall-badge', 'total-count', 'operational-count', 'down-count', 'checked-at',
    'refresh', 'host-metrics', 'host-badge', 'host-age', 'read-view', 'dashboard-view',
    'nav-services', 'nav-host', 'nav-images', 'host-section', 'services-section', 'summary'
  ].map(id => [id, new Element()]))
  const listeners = {}
  const scrolls = []
  const deferred = []
  const frames = []
  let scrollY = 0
  let interval
  const location = { hash: '#/services' }
  const context = {
    document: { querySelector: selector => nodes[selector.startsWith('#') ? selector.slice(1) : selector], createElement: tag => new Element(tag) },
    window: { addEventListener: (name, handler) => { listeners[name] = handler }, scrollTo: (...args) => { scrolls.push(args); scrollY = args[1] } },
    location, fetch: (url) => url.startsWith('/api/read') ? fetchRead(url) : Promise.resolve({ ok: true, json: async () => payload }),
    setInterval: callback => { interval = callback }, setTimeout: callback => { deferred.push(callback) },
    requestAnimationFrame: callback => { frames.push(callback) }, URL, URLSearchParams
  }
  vm.runInNewContext(`${source}\nglobalThis.testAPI = { render, renderRoute, load }`, context)
  return {
    nodes, scrolls, api: context.testAPI, refresh: () => interval(),
    navigate: hash => { location.hash = hash; listeners.hashchange() },
    traverse: hash => {
      location.hash = hash
      listeners.popstate?.()
      listeners.hashchange?.()
      scrollY = 350 // Browser restores the history entry after navigation listeners run.
      for (const callback of deferred.splice(0)) callback()
      for (const callback of frames.splice(0)) callback()
    },
    traverseWithLateRestoration: hash => {
      location.hash = hash
      listeners.popstate?.()
      listeners.hashchange?.()
      for (const callback of deferred.splice(0)) callback()
      scrollY = 350 // Chromium may restore the history entry after a zero-delay task.
      for (const callback of frames.splice(0)) callback()
    },
    setScroll: y => { scrollY = y }, getScroll: () => scrollY,
    read: () => nodes['read-view'], projects: () => nodes.projects
  }
}

const tick = async () => { await new Promise(resolve => setImmediate(resolve)) }

function response (status, data) {
  return { ok: status === 200, status, json: async () => data }
}

test('mixed cards retain semantic single detail links and independent external links', () => {
  const page = browser()
  page.api.render(payload)
  const cards = find(page.projects(), '.service-card')
  assert.equal(cards.length, 2)
  assert.equal(find(cards[0], '.service-name')[0].children[0].tag, 'a')
  assert.equal(find(cards[0], '.service-name')[0].children[0].href, '#/project/app/overview')
  assert.equal(find(cards[0], '.service-containers').length, 1)
  assert.equal(find(cards[1], '.service-containers').length, 0)
  assert.doesNotMatch(cards[1].textContent, /No containers|0 containers/)
  assert.match(cards[1].textContent, /Deployed releaseNot recorded/)
  assert.match(cards[1].textContent, /No backup recorded|Not configured/)
  assert.equal(find(cards[1], '.endpoint-link')[0].href, 'https://example.test/')
})

test('restore points use typed state, distinguish unreadable records and never read raw host output', () => {
  const requests = []
  const page = browser(url => { requests.push(url); return Promise.resolve(response(502, { error: 'Read operation failed' })) })
  const data = structuredClone(payload)
  data.projects[0].state_status = 'recorded'
  data.projects[0].release = { record_status: 'invalid' }
  data.projects[0].previous_release = { record_status: 'recorded', tag: 'previous', status: 'superseded', deployed_at_unix: 42 }
  data.projects[0].pending_release = { record_status: 'missing' }
  data.projects[0].restore_points = [{ position: 'previous', id: 'release:previous', status: 'superseded', deployed_at_unix: 42 }]
  data.projects[0].cli = { verdict: 'alert', branch: 'main', head: 'abcdef', drift: ['api'] }
  page.api.render(data)
  page.navigate('#/project/app/restore-points')
  assert.match(page.read().textContent, /current: unreadable release record \(not absent\)/)
  assert.match(page.read().textContent, /release:previous/)
  assert.match(page.read().textContent, /data: manual recovery only/)
  assert.equal(requests.length, 0)
  assert.match(find(page.projects(), '.cli-diagnostics')[0].textContent, /CLI overviewALERT/)
})

test('route links reset scroll; history traversal wins over native restoration; in-panel actions preserve scroll', async () => {
  const page = browser(async () => response(200, { output: 'real response' }))
  await tick()
  page.setScroll(350)
  page.navigate('#/project/app/logs')
  assert.equal(page.scrolls.length, 1)
  assert.deepEqual([...page.scrolls[0]], [0, 0])
  assert.equal(page.getScroll(), 0)
  assert.equal(page.nodes['nav-services'].attributes['aria-current'], 'page')
  assert.equal(find(page.read(), '.read-operations').length, 0)
  assert.equal(find(page.read(), '.read-scope').length, 1)
  page.setScroll(350)
  page.read().querySelector('.read-tail').value = '50'
  page.read().querySelector('.read-filters').dispatch('submit')
  await tick()
  assert.equal(page.scrolls.length, 1)
  assert.equal(page.getScroll(), 350)
  await page.refresh()
  assert.equal(page.scrolls.length, 1)
  assert.equal(page.getScroll(), 350)
  page.nodes.refresh.dispatch('click')
  await tick()
  assert.equal(page.getScroll(), 350)
  page.navigate('#/images')
  assert.equal(page.scrolls.length, 2)
  assert.equal(page.getScroll(), 0)
  assert.equal(page.nodes['nav-images'].attributes['aria-current'], 'page')
  assert.equal(page.nodes['nav-services'].attributes['aria-current'], undefined)
  page.setScroll(350)
  page.read().querySelector('.read-button').dispatch('click')
  await tick()
  assert.equal(page.getScroll(), 350)
  page.traverse('#/project/app/logs') // browser back, then native scroll restoration
  assert.equal(page.getScroll(), 0)
  assert.equal(page.read().querySelector('.read-filters')?.tag, 'form')
  page.setScroll(350)
  page.traverse('#/images') // browser forward
  assert.equal(page.getScroll(), 0)
  page.setScroll(350)
  const resets = page.scrolls.length
  page.navigate('#/images')
  page.traverse('#/images')
  assert.equal(page.scrolls.length, resets)
  assert.equal(page.getScroll(), 350)
})

test('Back and Forward between project overview and logs reset restored scroll', async () => {
  const page = browser(async () => response(200, { output: 'logs' }))
  await tick()
  page.navigate('#/project/app/overview')
  page.setScroll(350)
  page.navigate('#/project/app/logs')
  assert.equal(page.getScroll(), 0)
  page.setScroll(350)
  page.traverse('#/project/app/overview')
  assert.equal(page.getScroll(), 0)
  assert.equal(page.read().querySelector('.read-filters'), undefined)
  page.setScroll(350)
  page.traverse('#/project/app/logs')
  assert.equal(page.getScroll(), 0)
  assert.equal(page.read().querySelector('.read-filters')?.tag, 'form')
})

test('history traversal resets native scroll restored after the zero-delay task', async () => {
  const page = browser(async () => response(200, { output: 'logs' }))
  await tick()
  page.navigate('#/project/app/overview')
  page.navigate('#/project/app/logs')
  page.traverseWithLateRestoration('#/project/app/overview')
  assert.equal(page.getScroll(), 0)
  page.traverseWithLateRestoration('#/project/app/logs')
  assert.equal(page.getScroll(), 0)
  page.setScroll(350)
  const resets = page.scrolls.length
  page.traverseWithLateRestoration('#/project/app/logs')
  assert.equal(page.scrolls.length, resets)
})

for (const view of ['images', 'logs']) {
  const title = view === 'images' ? 'Image revisions' : 'Application logs'
  const route = view === 'images' ? '#/images' : '#/project/app/logs'
  for (const [status, message, expected] of [
    [400, 'Read operation refused', /HTTP 400.*allowlist/s],
    [502, 'Read operation failed or output limit exceeded', /HTTP 502.*cause.*unknown/s],
    [503, 'Read executor unavailable', /HTTP 503.*executor/s]
  ]) {
    test(`${view}: HTTP ${status} has honest unavailable state, safe server message, and existing action`, async () => {
      const page = browser(async () => response(status, { error: message }))
      page.api.render(payload)
      page.navigate(route)
      await tick()
      const state = page.read().querySelector('.read-unavailable')
      assert.equal(state.attributes.role, 'alert')
      assert.match(state.textContent, new RegExp(`${title} unavailable`))
      assert.match(state.textContent, expected)
      assert.match(state.textContent, new RegExp(`Server message · ${message}`))
      assert.equal(find(page.read(), '.read-output').length, 0)
      assert.equal(find(page.read(), '.read-button').length, 1)
      if (status === 502) assert.doesNotMatch(state.textContent, /socket.*absent|socket.*unreachable/i)
    })
  }
  test(`${view}: successful empty and nonempty reads are distinct from failures`, async () => {
    let output = 'actual data'
    const page = browser(async () => response(200, { output }))
    page.api.render(payload)
    page.navigate(route)
    await tick()
    assert.equal(page.read().querySelector('.read-output').textContent, 'actual data')
    output = ''
    page.read().querySelector('.read-button').dispatch(view === 'images' ? 'click' : 'submit')
    if (view === 'logs') page.read().querySelector('.read-filters').dispatch('submit')
    await tick()
    assert.match(page.read().querySelector('.read-output').textContent, /No output returned/)
    assert.equal(find(page.read(), '.read-unavailable').length, 0)
  })
  test(`${view}: generic HTTP 502 never discloses arbitrary server text`, async () => {
    const page = browser(async () => response(502, { error: 'secret from host' }))
    page.api.render(payload)
    page.navigate(route)
    await tick()
    assert.doesNotMatch(page.read().textContent, /secret from host/)
    assert.match(page.read().textContent, /HTTP 502/)
    assert.equal(find(page.read(), '.read-detail').length, 0)
  })
  for (const [failure, fetchFailure] of [
    ['aborted transport', () => Promise.reject(new Error('private connection detail'))],
    ['non-JSON HTTP 200', async () => ({ ok: true, status: 200, json: async () => { throw new Error('private JSON detail') } })]
  ]) {
    test(`${view}: ${failure} is unavailable without a server detail and recovers on retry`, async () => {
      let broken = true
      const page = browser(async () => broken ? fetchFailure() : response(200, { output: 'real recovered output' }))
      page.api.render(payload)
      page.navigate(route)
      await tick()
      const state = page.read().querySelector('.read-unavailable')
      assert.equal(state.attributes.role, 'alert')
      assert.match(state.textContent, /transport or invalid response/)
      assert.doesNotMatch(state.textContent, /undefined|private|No output returned/)
      assert.equal(find(state, '.read-detail').length, 0)
      assert.equal(find(page.read(), '.read-output').length, 0)
      assert.equal(find(page.read(), '.read-button').length, 1)
      broken = false
      if (view === 'images') page.read().querySelector('.read-button').dispatch('click')
      else page.read().querySelector('.read-filters').dispatch('submit')
      await tick()
      assert.equal(find(page.read(), '.read-unavailable').length, 0)
      assert.equal(page.read().querySelector('.read-output').textContent, 'real recovered output')
    })
  }
  test(`${view}: limit failure never appears as an empty success and retry replaces error`, async () => {
    let oversized = true
    const page = browser(async () => response(oversized ? 502 : 200,
      oversized ? { error: 'Read operation failed or output limit exceeded' } : { output: 'real recovered output' }))
    page.api.render(payload)
    page.navigate(route)
    await tick()
    assert.equal(find(page.read(), '.read-unavailable').length, 1)
    assert.equal(find(page.read(), '.read-output').length, 0)
    oversized = false
    if (view === 'images') page.read().querySelector('.read-button').dispatch('click')
    else page.read().querySelector('.read-filters').dispatch('submit')
    await tick()
    assert.equal(find(page.read(), '.read-unavailable').length, 0)
    assert.equal(page.read().querySelector('.read-output').textContent, 'real recovered output')
  })
}
