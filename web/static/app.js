const projectsEl = document.querySelector('#projects')
const overallBadgeEl = document.querySelector('#overall-badge')
const totalCountEl = document.querySelector('#total-count')
const operationalCountEl = document.querySelector('#operational-count')
const downCountEl = document.querySelector('#down-count')
const checkedAtEl = document.querySelector('#checked-at')
const refreshEl = document.querySelector('#refresh')

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

const renderProject = (project) => {
  const card = document.createElement('article')
  card.className = 'service-card'

  const header = document.createElement('header')
  header.className = 'service-header'
  const name = document.createElement('h2')
  name.className = 'service-name'
  name.textContent = project.name || 'Unnamed service'
  header.append(name, statusBadge(project.status))

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

  if (project.error) {
    const error = document.createElement('p')
    error.className = 'service-error'
    error.textContent = project.error
    card.append(error)
  }

  return card
}

const updateSummary = (payload) => {
  const projects = Array.isArray(payload.projects) ? payload.projects : []
  const operational = projects.filter((project) => project.status === 'operational').length
  const down = projects.filter((project) => project.status === 'down').length
  const status = labels[payload.status] ? payload.status : 'unknown'

  totalCountEl.textContent = String(projects.length)
  operationalCountEl.textContent = String(operational)
  downCountEl.textContent = String(down)
  downCountEl.classList.toggle('bad', down > 0)
  checkedAtEl.textContent = formatTime(payload.checked_at)
  const checkedDate = toDate(payload.checked_at)
  checkedAtEl.dateTime = checkedDate ? checkedDate.toISOString() : ''
  overallBadgeEl.className = `badge ${status}`
  overallBadgeEl.textContent = labels[status]
}

const render = (payload) => {
  const projects = Array.isArray(payload.projects) ? payload.projects : []
  updateSummary({ ...payload, projects })

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
    projectsEl.append(renderProject(project))
  }
}

const renderError = (error) => {
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
