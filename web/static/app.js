const projectsEl = document.querySelector('#projects')
const overallBadgeEl = document.querySelector('#overall-badge')
const totalCountEl = document.querySelector('#total-count')
const operationalCountEl = document.querySelector('#operational-count')
const downCountEl = document.querySelector('#down-count')
const checkedAtEl = document.querySelector('#checked-at')
const refreshEl = document.querySelector('#refresh')
const hostMetricsEl = document.querySelector('#host-metrics')
const hostBadgeEl = document.querySelector('#host-badge')
const hostAgeEl = document.querySelector('#host-age')
const readViewEl = document.querySelector('#read-view')
const dashboardViewEl = document.querySelector('#dashboard-view')
let lastPayload = null
let readGeneration = 0
let renderedRoute = null
let navigatedHash = typeof location === 'undefined' ? '' : location.hash

const labels = {
  operational: 'Operational',
  healthy: 'Healthy',
  unhealthy: 'Unhealthy',
  degraded: 'Degraded',
  down: 'Down',
  unknown: 'Unknown'
}

const toDate = (value, unix = false) => {
  if (!value) return null
  const date = new Date(unix ? Number(value) * 1000 : value)
  return Number.isNaN(date.getTime()) ? null : date
}

const formatDateTime = (value, unix = false) => {
  const date = toDate(value, unix)
  if (!date) return '—'
  return date.toLocaleString([], {
    dateStyle: 'medium',
    timeStyle: 'short'
  })
}

const formatTime = (value) => {
  const date = toDate(value)
  if (!date) return '—'
  return date.toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

const shortRevision = (value) => value ? String(value).slice(0, 12) : '—'

const statusBadge = (status) => {
  const resolvedStatus = labels[status] ? status : 'unknown'
  const badge = document.createElement('div')
  badge.className = `badge ${{ healthy: 'operational', unhealthy: 'down' }[resolvedStatus] || resolvedStatus}`
  badge.textContent = labels[resolvedStatus]
  return badge
}

const hostStatus = (status) => ({ normal: 'operational', warning: 'degraded', critical: 'down' }[status] || 'unknown')
const hostLabel = (status) => ({ normal: 'Normal', warning: 'Warning', critical: 'Critical' }[status] || 'Unknown')
const hostBadge = (status) => {
  const badge = statusBadge(hostStatus(status))
  badge.textContent = hostLabel(status)
  return badge
}
const formatBytes = (bytes) => `${(bytes / (1024 ** 3)).toFixed(1)} GiB`
const level = (status) => ({ critical: 3, warning: 2, unknown: 1, normal: 0 }[status] ?? 1)
const serviceState = (project, containers) => {
  if (project.status === 'down' || project.status === 'unhealthy') return project.status
  if (containers.some((container) => container.status === 'critical')) return 'degraded'
  return labels[project.status] ? project.status : 'unknown'
}

const renderHost = (host) => {
  hostMetricsEl.replaceChildren()
  const age = host?.age_seconds
  hostAgeEl.textContent = age == null ? 'Measurement unavailable' : `Measured ${age} seconds ago${host.status === 'available' ? '' : ' — stale'}`
  const snapshot = host?.status === 'available' ? host.snapshot : null
  hostBadgeEl.className = 'badge unknown'
  hostBadgeEl.textContent = 'Unknown'
  if (!snapshot) {
    const empty = document.createElement('div')
    empty.className = 'empty'
    empty.textContent = host?.message || 'Host snapshot unavailable'
    hostMetricsEl.append(empty)
    return
  }

  const addMetric = (label, metric, value) => {
    const card = document.createElement('div')
    card.className = 'host-metric'
    const header = document.createElement('div')
    header.className = 'host-metric-header'
    const name = document.createElement('strong')
    name.textContent = label
    header.append(name, hostBadge(metric?.status))
    const detail = document.createElement('p')
    detail.textContent = value || metric?.message || 'Unavailable'
    card.append(header, detail)
    hostMetricsEl.append(card)
  }
  // Container and Docker diagnostics remain in the API, not in the host badge.
  const states = [snapshot.cpu.status, snapshot.memory.status,
    ...snapshot.disks.map((disk) => disk.status)]
  const overall = states.reduce((highest, status) => level(status) > level(highest) ? status : highest, 'normal')
  hostBadgeEl.className = `badge ${hostStatus(overall)}`
  hostBadgeEl.textContent = hostLabel(overall)

  addMetric('CPU', snapshot.cpu, snapshot.cpu.value == null ? null : `${snapshot.cpu.value.toFixed(1)}%`)
  const memory = snapshot.memory.value
  addMetric('RAM', snapshot.memory, memory ? `${formatBytes(memory.used_bytes)} / ${formatBytes(memory.total_bytes)}` : null)
  for (const disk of snapshot.disks) {
    const value = disk.value
    addMetric(`Disk ${value?.mount || ''}`, disk, value ? `${formatBytes(value.used_bytes)} / ${formatBytes(value.total_bytes)}` : null)
  }
}

const externalLink = (label, value, className = '') => {
  let url
  try {
    url = new URL(value)
    if (!['http:', 'https:'].includes(url.protocol)) throw new Error('Unsupported URL')
  } catch {
    const text = document.createElement('span')
    text.className = `${className} invalid-url`.trim()
    text.textContent = label
    text.title = value
    return text
  }

  const link = document.createElement('a')
  link.className = className
  link.href = url.href
  link.target = '_blank'
  link.rel = 'noreferrer'
  link.textContent = label
  link.title = value
  return link
}

const addFact = (list, label, value, className = '') => {
  const item = document.createElement('div')
  item.className = 'fact'
  const term = document.createElement('dt')
  term.textContent = label
  const description = document.createElement('dd')
  if (className) description.className = className
  description.textContent = value
  item.append(term, description)
  list.append(item)
}

const backupDetail = (attempt) => {
  if (!attempt) return 'No backup recorded'
  if (attempt.result === 'invalid') return 'Invalid backup record'
  const date = toDate(attempt.started_at_unix, true)
  const age = date ? `${Math.max(0, Math.floor((Date.now() - date.getTime()) / 1000))}s ago` : 'time unavailable'
  const result = attempt.result === 'success' ? 'Success' : attempt.result === 'failure' ? 'Failure' : 'Unknown result'
  const destination = attempt.destination || 'unknown destination'
  const duration = attempt.duration_ms == null ? '' : ` · ${attempt.duration_ms} ms`
  return `${result} · ${age} · ${destination}${duration}`
}

const renderProject = (project, containers) => {
  const card = document.createElement('article')
  card.className = 'service-card'

  const header = document.createElement('header')
  header.className = 'service-header'
  const name = document.createElement('h2')
  name.className = 'service-name'
  const projectLink = document.createElement('a')
  projectLink.href = `#/project/${encodeURIComponent(project.name)}/overview`
  projectLink.textContent = project.name || 'Unnamed service'
  name.append(projectLink)
  header.append(name, statusBadge(serviceState(project, containers)))

  const endpoints = document.createElement('div')
  endpoints.className = 'endpoints'

  const publicUrl = project.public_url || project.url
  if (publicUrl) {
    const publicRow = document.createElement('div')
    publicRow.className = 'endpoint'
    const publicLabel = document.createElement('span')
    publicLabel.textContent = 'Service'
    publicRow.append(publicLabel, externalLink(publicUrl, publicUrl, 'endpoint-link'))
    endpoints.append(publicRow)
  }

  const healthRow = document.createElement('div')
  healthRow.className = 'endpoint'
  const healthLabel = document.createElement('span')
  healthLabel.textContent = 'Health'
  const healthValue = project.health_url
    ? externalLink(project.health_url, project.health_url, 'endpoint-link')
    : document.createElement('span')
  if (!project.health_url) {
    healthValue.className = 'endpoint-empty'
    healthValue.textContent = 'Not configured'
  }
  healthRow.append(healthLabel, healthValue)
  endpoints.append(healthRow)

  const facts = document.createElement('dl')
  facts.className = 'service-facts'
  addFact(
    facts,
    'Latency',
    project.latency_ms == null ? '—' : `${project.latency_ms} ms`,
    'tabular'
  )
  addFact(
    facts,
    'HTTP',
    project.http_status == null ? '—' : String(project.http_status),
    'tabular'
  )
  addFact(facts, 'Checked', formatTime(project.checked_at), 'tabular')

  const release = project.release || {}
  const releaseBlock = document.createElement('div')
  releaseBlock.className = 'release'
  const releaseCopy = document.createElement('div')
  releaseCopy.className = 'release-copy'
  const releaseLabel = document.createElement('span')
  releaseLabel.className = 'release-label'
  releaseLabel.textContent = 'Deployed release'
  const releaseValue = document.createElement('div')
  releaseValue.className = 'release-value'
  const releaseTag = document.createElement('code')
  releaseTag.textContent = release.tag || (release.revision ? shortRevision(release.revision) : 'Not recorded')
  releaseValue.append(releaseTag)
  if (release.tag && release.revision && !String(release.revision).startsWith(String(release.tag))) {
    const releaseSha = document.createElement('span')
    releaseSha.textContent = shortRevision(release.revision)
    releaseValue.append(releaseSha)
  }
  releaseCopy.append(releaseLabel, releaseValue)

  const deployedAt = document.createElement('time')
  deployedAt.className = 'deployed-at'
  deployedAt.textContent = formatDateTime(release.deployed_at_unix, true)
  if (release.deployed_at_unix) {
    const date = toDate(release.deployed_at_unix, true)
    if (date) deployedAt.dateTime = date.toISOString()
  }
  releaseBlock.append(releaseCopy, deployedAt)

  card.append(header, endpoints, facts, releaseBlock)

  const probes = Array.isArray(project.services)
    ? project.services.filter(probe => probe.message !== 'No service probe configured')
    : []
  if (probes.length) {
    const group = document.createElement('div')
    group.className = 'service-containers'
    const label = document.createElement('h3')
    label.textContent = 'Service health'
    group.append(label)
    for (const probe of probes) {
      const row = document.createElement('div')
      row.className = 'container-row service-probe'
      const name = document.createElement('strong')
      name.textContent = probe.service
      const value = document.createElement('span')
      value.className = 'container-state'
      value.textContent = probe.kind === 'http' && probe.latency_ms != null ? `${probe.latency_ms} ms`
        : probe.kind === 'heartbeat' && probe.age_seconds != null ? `${probe.age_seconds} s since heartbeat`
          : probe.message || 'No measurement'
      row.append(name, statusBadge(probe.status), value)
      group.append(row)
    }
    card.append(group)
  }

  const backup = document.createElement('dl')
  backup.className = 'service-facts backup-facts'
  addFact(backup, 'Local backup', backupDetail(project.last_backup), ['failure', 'invalid'].includes(project.last_backup?.result) ? 'bad' : '')
  const offsite = project.last_offsite?.result === 'invalid' ? backupDetail(project.last_offsite)
    : project.offsite_configured === false ? 'Not configured'
    : project.offsite_configured === true
      ? project.last_offsite ? backupDetail(project.last_offsite)
        : 'No copy recorded for the last local backup'
      : 'Configuration unavailable'
  addFact(backup, 'Off-site copy', offsite, ['failure', 'invalid'].includes(project.last_offsite?.result) ? 'bad' : '')
  card.append(backup)

  if (containers.length) {
    const group = document.createElement('div')
    group.className = 'service-containers'
    const label = document.createElement('h3')
    label.textContent = 'Containers'
    group.append(label)
    for (const container of containers) {
      const row = document.createElement('div')
      row.className = 'container-row'
      const containerName = document.createElement('strong')
      containerName.textContent = container.service
      const state = document.createElement('span')
      state.className = 'container-state'
      const completed = container.state === 'exited' && container.status === 'normal'
      state.textContent = completed ? 'Completed' : container.status === 'critical' ? 'Stopped' : container.state
      const badge = statusBadge(container.status === 'critical' ? 'down' : 'operational')
      if (completed) badge.textContent = 'Completed'
      row.append(containerName, state, badge)
      group.append(row)
    }
    card.append(group)
  }
  if (Array.isArray(release.services)) {
    const images = document.createElement('div')
    images.className = 'service-facts'
    for (const service of release.services) {
      addFact(images, service.name, service.image || '—')
    }
    card.append(images)
  }
  if (project.pending_release) {
    const pending = document.createElement('p')
    pending.className = 'service-error'
    pending.textContent = `Release ${project.pending_release.tag || '—'} ${project.pending_release.status || 'activating'} — Docker state may differ; run yard status and yard rollback.`
    card.append(pending)
  }

  if (project.error) {
    const error = document.createElement('p')
    error.className = 'service-error'
    error.textContent = project.error
    card.append(error)
  }

  return card
}

const updateSummary = (payload, containers) => {
  const projects = Array.isArray(payload.projects) ? payload.projects : []
  const states = projects.map((project) => serviceState(project,
    containers.filter((container) => container.project === project.name)))
  const operational = states.filter((state) => state === 'operational' || state === 'healthy').length
  const down = states.filter((state) => state === 'down' || state === 'unhealthy').length
  const attention = states.length - operational
  const status = !projects.length ? 'unknown' : operational === projects.length ? 'operational'
    : operational === 0 && down > 0 ? 'down' : down > 0 || states.includes('degraded') ? 'degraded' : 'unknown'

  totalCountEl.textContent = String(projects.length)
  operationalCountEl.textContent = String(operational)
  downCountEl.textContent = String(attention)
  downCountEl.classList.toggle('bad', attention > 0)
  checkedAtEl.textContent = formatTime(payload.checked_at)
  const checkedDate = toDate(payload.checked_at)
  checkedAtEl.dateTime = checkedDate ? checkedDate.toISOString() : ''
  overallBadgeEl.className = `badge ${status}`
  overallBadgeEl.textContent = labels[status]
}

const render = (payload) => {
  lastPayload = payload
  renderHost(payload.host)
  const projects = Array.isArray(payload.projects) ? payload.projects : []
  const snapshot = payload.host?.status === 'available' ? payload.host.snapshot : null
  const containers = snapshot?.containers || []
  updateSummary({ ...payload, projects }, containers)

  projectsEl.replaceChildren()
  projectsEl.setAttribute('aria-busy', 'false')
  if (!projects.length) {
    const empty = document.createElement('div')
    empty.className = 'empty'
    empty.setAttribute('role', 'status')
    empty.textContent = 'No Yard services found.'
    projectsEl.append(empty)
    renderRoute(payload)
    return
  }

  for (const project of projects) {
    projectsEl.append(renderProject(project, containers.filter((container) => container.project === project.name)))
  }
  if (snapshot?.containers_message) {
    const notice = document.createElement('div')
    notice.className = 'service-notice'
    notice.textContent = snapshot.containers_message
    projectsEl.append(notice)
  }
  renderRoute(payload)
}

const renderError = (error) => {
  lastPayload = null
  renderHost(null)
  totalCountEl.textContent = '—'
  operationalCountEl.textContent = '—'
  downCountEl.textContent = '—'
  downCountEl.classList.remove('bad')
  checkedAtEl.textContent = 'Unavailable'
  checkedAtEl.dateTime = ''
  overallBadgeEl.className = 'badge down'
  overallBadgeEl.textContent = 'Unavailable'

  projectsEl.replaceChildren()
  projectsEl.setAttribute('aria-busy', 'false')
  const empty = document.createElement('div')
  empty.className = 'empty error-state'
  empty.setAttribute('role', 'alert')
  empty.textContent = error instanceof Error ? error.message : String(error)
  projectsEl.append(empty)
  if (readViewEl) {
    readViewEl.replaceChildren()
    readViewEl.hidden = false
    readViewEl.textContent = 'Status unavailable. Read views cannot be loaded.'
  }
}

const element = (tag, className, text) => {
  const node = document.createElement(tag)
  node.className = className
  if (text != null) node.textContent = text
  return node
}

const readLink = (label, href, active = false) => {
  const link = element('a', active ? 'active' : '', label)
  link.href = href
  if (active) link.setAttribute('aria-current', 'page')
  return link
}

const readPanel = (title, subtitle) => {
  const panel = element('section', 'read-panel')
  panel.append(element('h2', '', title), element('p', 'read-caption', subtitle))
  return panel
}

const readUnavailable = (title, action, status, serverMessage) => {
  const state = element('div', 'read-unavailable')
  state.setAttribute('role', 'alert')
  const heading = element('div', 'read-unavailable-heading')
  heading.append(element('span', 'badge unknown', 'Unavailable'), element('strong', '', `${title} unavailable`))
  state.append(heading)
  const causes = {
    400: 'HTTP 400 — this read was refused by the allowlist or its bounded parameters. Check the selected service and refresh the inventory.',
    502: 'HTTP 502 — the host read failed; the exact cause is unknown to this page. A failed command, timeout, output above 128 KiB or invalid executor reply are possible.',
    503: 'HTTP 503 — the read executor on the host did not answer. No data was read; this is not an empty result.'
  }
  state.append(element('p', '', causes[status] || (status ? `HTTP ${status} — the read could not be completed.` : 'The read could not be completed (transport or invalid response).')))
  state.append(element('p', '', `Use ${action} to repeat this request.${status === 502 ? ' If it keeps failing, inspect the host read executor (yard-web-read).' : status === 503 ? ' If it keeps failing, check the host read executor service.' : ''}`))
  const expected = { 400: 'Read operation refused', 502: 'Read operation failed or output limit exceeded', 503: 'Read executor unavailable' }
  if (typeof serverMessage === 'string' && serverMessage.trim() && serverMessage === expected[status]) {
    state.append(element('p', 'read-detail', `Server message · ${serverMessage}`))
  }
  return state
}

const clearRead = panel => {
  panel.querySelector?.('.read-output')?.remove()
  panel.querySelector?.('.read-unavailable')?.remove()
}

const readOutput = (panel, params, title, action) => {
  const output = element('pre', 'read-output', 'Loading real host data…')
  output.setAttribute('role', 'status')
  panel.append(output)
  const generation = ++readGeneration
  fetch(`/api/read?${new URLSearchParams(params)}`, { cache: 'no-store' }).then(async response => {
    let data
    try { data = await response.json() } catch { return { status: null } }
    if (!response.ok) return { status: response.status, message: data?.error }
    return typeof data?.output === 'string' ? { output: data.output } : { status: null }
  }).catch(() => ({ status: null })).then(result => {
    if (generation !== readGeneration) return
    if ('output' in result) {
      output.textContent = result.output || 'No output returned.'
    } else {
      output.remove()
      panel.append(readUnavailable(title, action, result.status, result.message))
    }
  })
}

const renderReadView = (payload, name, tab) => {
  if (!readViewEl) return
  ++readGeneration
  readViewEl.replaceChildren()
  const project = (payload.projects || []).find(item => item.name === name)
  if (!project) {
    readViewEl.append(element('p', 'read-error', 'Project not found in the current Yard inventory.'))
    return
  }
  readViewEl.append(readLink('← All services', '#/services'))
  const heading = element('div', 'read-heading')
  heading.append(element('h1', '', name), statusBadge(project.status))
  readViewEl.append(heading, element('p', 'read-meta', `Health: ${project.health_url || 'Not configured'} · Release: ${project.release?.tag || 'not recorded'}`))
  const monitor = project.url_monitor === true
  if (monitor) {
    const panel = readPanel('URL monitor', 'No deployment actions: this project only checks an HTTP URL.')
    panel.append(element('p', '', `Status: ${labels[project.status] || 'Unknown'} · HTTP ${project.http_status || 'unavailable'} · ${project.latency_ms == null ? 'latency unavailable' : `${project.latency_ms} ms`}`))
    if (project.error) panel.append(element('p', 'read-error', project.error))
    readViewEl.append(panel)
    return
  }
  const tabs = element('nav', 'read-tabs')
  tabs.setAttribute('aria-label', 'Project views')
  for (const [key, label] of [['overview', 'Overview'], ['logs', 'Logs'], ['restore-points', 'Restore points'], ['restore-log', 'Restore log']]) {
    tabs.append(readLink(label, `#/project/${encodeURIComponent(name)}/${key}`, tab === key))
  }
  readViewEl.append(tabs)
  readViewEl.append(element('p', 'read-scope', 'Read-only view — deploy, backup, restore, prune and host collection are unavailable here.'))
  const grid = element('div', 'read-layout')
  const content = element('div', 'read-main')
  if (tab === 'overview') {
    const panel = readPanel('Status', 'Current HTTP health and recorded Yard state · /api/status')
    const containers = payload.host?.status === 'available' ? payload.host.snapshot?.containers || [] : []
    panel.append(renderProject(project, containers.filter(item => item.project === name)))
    content.append(panel)
  } else if (tab === 'logs') {
    const panel = readPanel('Application logs', `yard logs ${name} --no-follow · one-shot reading, no live stream`)
    const form = element('form', 'read-filters')
    const serviceLabel = element('label', '', 'Service')
    const service = element('select', 'read-service')
    const all = element('option', '', 'All application services')
    all.value = ''
    service.append(all)
    for (const value of project.application_services || []) {
      const option = element('option', '', value)
      option.value = value
      service.append(option)
    }
    serviceLabel.append(service)
    const tailLabel = element('label', '', 'Tail')
    const tail = element('select', 'read-tail')
    for (const value of [50, 100, 200, 500, 1000]) {
      const option = element('option', '', `${value} lines`)
      option.value = String(value)
      tail.append(option)
    }
    tail.value = '200'
    tailLabel.append(tail)
    const sinceLabel = element('label', '', 'Since (duration or timestamp)')
    const since = element('input', 'read-since')
    since.maxLength = 40
    since.placeholder = '2h or 2026-09-24T08:00:00Z'
    sinceLabel.append(since)
    const button = element('button', 'read-button', 'Load logs')
    button.type = 'submit'
    form.append(serviceLabel, tailLabel, sinceLabel, button)
    panel.append(form, element('p', 'read-note', 'Only configured application services; at most 1000 lines and 128 KiB. Follow is not available.'))
    const request = () => {
      const params = { op: 'logs', project: name, tail: tail.value }
      if (service.value) params.service = service.value
      if (since.value?.trim()) params.since = since.value.trim()
      clearRead(panel)
      readOutput(panel, params, 'Application logs', 'Load logs')
    }
    form.addEventListener('submit', event => { event.preventDefault(); request() })
    content.append(panel)
    request()
  } else if (tab === 'restore-points' || tab === 'restore-log') {
    const title = tab === 'restore-points' ? 'Recorded releases and backup attempts' : 'Restore attempt journal'
    const panel = readPanel(title, `yard ${tab} ${name} · read-only output from the host`)
    if (tab === 'restore-points') panel.append(element('p', 'read-note', 'Only recorded application releases. Backup destinations are descriptive metadata, not restorable data targets.'))
    readOutput(panel, { op: tab, project: name }, title, 'Refresh')
    content.append(panel)
  } else {
    content.append(element('p', 'read-error', 'Unknown project view.'))
  }
  grid.append(content)
  readViewEl.append(grid)
}

const renderImagesView = () => {
  if (!readViewEl) return
  ++readGeneration
  readViewEl.replaceChildren()
  const panel = readPanel('Image revisions', 'yard images · global read-only inventory of protected, in-use and reclaimable revisions')
  panel.append(element('p', 'read-note', 'Inventory fails closed when release state or Docker inspection is unavailable. Prune is CLI-only and removes all eligible candidates, never a selection.'))
  const button = element('button', 'read-button', 'Re-run inventory')
  button.addEventListener('click', () => {
    clearRead(panel)
    readOutput(panel, { op: 'images' }, 'Image revisions', 'Re-run inventory')
  })
  panel.append(button)
  readViewEl.append(panel)
  readOutput(panel, { op: 'images' }, 'Image revisions', 'Re-run inventory')
}

const renderRoute = payload => {
  if (!readViewEl || !dashboardViewEl || typeof location === 'undefined') return
  const hash = location.hash || '#/services'
  const match = /^#\/project\/([A-Za-z0-9_-]{1,64})\/(overview|logs|restore-points|restore-log)$/.exec(hash)
  const images = hash === '#/images'
  const detail = images || Boolean(match)
  readViewEl.hidden = !detail
  dashboardViewEl.hidden = detail
  const host = document.querySelector('#host-section')
  if (host) host.hidden = false
  const services = document.querySelector('#services-section')
  if (services) services.hidden = hash === '#/host'
  const summary = document.querySelector('.summary')
  if (summary) summary.hidden = hash === '#/host'
  for (const [id, selected] of [['nav-services', !images && hash !== '#/host'], ['nav-host', hash === '#/host'], ['nav-images', images]]) {
    const link = document.querySelector(`#${id}`)
    if (link) {
      link.className = selected ? 'active' : ''
      if (selected) link.setAttribute('aria-current', 'page')
      else link.removeAttribute('aria-current')
    }
  }
  if (renderedRoute === hash && detail && !hash.endsWith('/overview') &&
    (images || (payload.projects || []).some(item => item.name === match[1]))) return
  renderedRoute = hash
  if (images) renderImagesView()
  else if (match) renderReadView(payload, match[1], match[2])
  else { ++readGeneration; readViewEl.replaceChildren() }
}

if (typeof window !== 'undefined') window.addEventListener('hashchange', () => {
  if (location.hash === navigatedHash) return
  navigatedHash = location.hash
  window.scrollTo(0, 0)
  if (lastPayload) renderRoute(lastPayload)
})

if (typeof window !== 'undefined') window.addEventListener('popstate', () => {
  const hash = location.hash
  if (hash === navigatedHash) return
  // History traversal can restore the entry's scroll after popstate; leave native
  // restoration enabled, then override it only for a changed application route.
  setTimeout(() => {
    if (location.hash === hash) window.scrollTo(0, 0)
  }, 0)
})

const load = async () => {
  refreshEl.disabled = true
  refreshEl.setAttribute('aria-label', 'Refreshing service status')
  projectsEl.setAttribute('aria-busy', 'true')
  try {
    const response = await fetch('/api/status', { cache: 'no-store' })
    if (!response.ok) throw new Error(`Status request failed with HTTP ${response.status}`)
    render(await response.json())
  } catch (error) {
    renderError(error)
  } finally {
    refreshEl.disabled = false
    refreshEl.removeAttribute('aria-label')
  }
}

refreshEl.addEventListener('click', () => { renderedRoute = null; load() })
load()
setInterval(load, 30_000)
