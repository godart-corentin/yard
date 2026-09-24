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

const labels = {
  operational: 'Operational',
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
  badge.className = `badge ${resolvedStatus}`
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
  if (project.status === 'down') return 'down'
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
  name.textContent = project.name || 'Unnamed service'
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
  releaseTag.textContent = release.tag || shortRevision(release.revision)
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
      state.textContent = container.status === 'critical' ? 'Stopped' : container.state
      row.append(containerName, state, statusBadge(container.status === 'critical' ? 'down' : 'operational'))
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
  const operational = states.filter((state) => state === 'operational').length
  const down = states.filter((state) => state === 'down').length
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
}

const renderError = (error) => {
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
}

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

refreshEl.addEventListener('click', load)
load()
setInterval(load, 30_000)
