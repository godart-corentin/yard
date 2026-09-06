const projectsEl = document.querySelector('#projects')
const summaryEl = document.querySelector('#summary')
const summaryBadgeEl = document.querySelector('#summary-badge')
const checkedAtEl = document.querySelector('#checked-at')
const refreshEl = document.querySelector('#refresh')

const labels = {
  operational: 'Operational',
  degraded: 'Degraded',
  down: 'Down',
  unknown: 'Unknown'
}

const overallTitles = {
  operational: 'All systems operational',
  degraded: 'Some services are degraded',
  down: 'Services are unavailable',
  unknown: 'Status is incomplete'
}

const formatTime = (value) => {
  if (!value) return '—'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleString()
}

const formatUnix = (value) => {
  if (!value) return '—'
  const date = new Date(Number(value) * 1000)
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleString()
}

const shortRevision = (value) => value ? String(value).slice(0, 12) : '—'

const statusBadge = (status) => {
  const badge = document.createElement('div')
  badge.className = `badge ${status || 'unknown'}`
  badge.textContent = labels[status] || labels.unknown
  return badge
}

const renderProject = (project) => {
  const row = document.createElement('article')
  row.className = 'project-row'

  const identity = document.createElement('div')
  const name = document.createElement('div')
  name.className = 'project-name'
  name.textContent = project.name
  const url = document.createElement('div')
  url.className = 'project-url'
  url.textContent = project.health_url || 'No health check configured'
  identity.append(name, url)

  const meta = document.createElement('div')
  meta.className = 'project-meta'
  const release = project.release || {}
  const releaseLine = document.createElement('div')
  releaseLine.append('Release ')
  const sha = document.createElement('span')
  sha.className = 'sha'
  sha.textContent = release.tag || shortRevision(release.revision)
  releaseLine.append(sha)
  const deployed = document.createElement('div')
  deployed.textContent = `Deployed ${formatUnix(release.deployed_at_unix)}`
  meta.append(releaseLine, deployed)

  const status = document.createElement('div')
  status.className = 'project-status'
  status.append(statusBadge(project.status))
  const latency = document.createElement('div')
  latency.className = 'latency'
  latency.textContent = project.latency_ms == null ? '—' : `${project.latency_ms} ms${project.http_status ? ` · HTTP ${project.http_status}` : ''}`
  status.append(latency)
  if (project.error) {
    const error = document.createElement('div')
    error.className = 'error'
    error.textContent = project.error
    status.append(error)
  }

  row.append(identity, meta, status)
  return row
}

const render = (payload) => {
  const status = payload.status || 'unknown'
  summaryEl.className = `summary panel ${status}`
  summaryEl.querySelector('h1').textContent = overallTitles[status] || overallTitles.unknown
  summaryBadgeEl.className = `badge ${status}`
  summaryBadgeEl.textContent = labels[status] || labels.unknown
  checkedAtEl.textContent = `Checked ${formatTime(payload.checked_at)}`

  projectsEl.replaceChildren()
  if (!payload.projects?.length) {
    const empty = document.createElement('div')
    empty.className = 'empty'
    empty.textContent = 'No Yard projects found.'
    projectsEl.append(empty)
    return
  }

  for (const project of payload.projects) {
    projectsEl.append(renderProject(project))
  }
}

const load = async () => {
  refreshEl.disabled = true
  try {
    const response = await fetch('/api/status', { cache: 'no-store' })
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    render(await response.json())
  } catch (error) {
    summaryEl.className = 'summary panel down'
    summaryEl.querySelector('h1').textContent = 'Yard status is unavailable'
    summaryBadgeEl.className = 'badge down'
    summaryBadgeEl.textContent = 'Down'
    checkedAtEl.textContent = ''
    projectsEl.replaceChildren()
    const empty = document.createElement('div')
    empty.className = 'empty'
    empty.textContent = error instanceof Error ? error.message : String(error)
    projectsEl.append(empty)
  } finally {
    refreshEl.disabled = false
  }
}

refreshEl.addEventListener('click', load)
load()
setInterval(load, 30_000)
