import assert from 'node:assert/strict'
import { chromium } from '/cache/tmp/pw-install/node_modules/playwright/index.mjs'

const [baseUrl, output] = process.argv.slice(2)
const browser = await chromium.launch({
  headless: true,
  executablePath: '/cache/playwright/chromium_headless_shell-1234/chrome-headless-shell-linux64/chrome-headless-shell',
  args: ['--no-sandbox']
})
try {
  for (const variant of ['a', 'b']) {
    for (const width of [1280, 400]) {
      const page = await browser.newPage({ viewport: { width, height: 900 }, deviceScaleFactor: 1 })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.goto(`${baseUrl}/?variant=${variant}`, { waitUntil: 'networkidle' })
      await page.locator('#projects[aria-busy="false"] .service-card').first().waitFor()
      const actual = await page.evaluate(() => {
        const cards = [...document.querySelectorAll('.service-card')]
        const host = document.querySelector('#host-metrics')
        const secondary = host.querySelector('.host-secondary')
        const rows = cards.map(card => ({
          name: card.querySelector('.service-name').textContent,
          status: card.querySelector('.service-header .badge').textContent,
          containers: [...card.querySelectorAll('.container-row')].map(row => ({
            name: row.querySelector('strong').textContent,
            state: row.querySelector('.container-state').textContent,
            status: row.querySelector('.badge').textContent
          }))
        }))
        const viewport = document.documentElement.clientWidth
        return {
          host: document.querySelector('#host-badge').textContent,
          hostMetrics: [...host.querySelectorAll('.host-metric strong')].map(x => x.textContent),
          summary: {
            total: document.querySelector('#total-count').textContent,
            operational: document.querySelector('#operational-count').textContent,
            attention: document.querySelector('#down-count').textContent
          },
          secondary: secondary?.textContent ?? null,
          secondaryBox: secondary?.getBoundingClientRect().toJSON() ?? null,
          services: document.querySelector('#overall-badge').textContent,
          rows,
          horizontalOverflow: document.documentElement.scrollWidth > viewport,
          clipped: [...document.querySelectorAll('.host-metric, .service-card')].some(element => {
            const box = element.getBoundingClientRect()
            return box.left < 0 || box.right > viewport + 1
          }),
          secondaryOverflow: secondary ? secondary.scrollWidth > secondary.clientWidth + 1 : false
        }
      })
      assert.deepEqual(errors, [])
      assert.deepEqual(actual.hostMetrics.map(x => x.split(' ')[0]), ['CPU', 'RAM', 'Disk'])
      assert.deepEqual(actual.summary, { total: '2', operational: '0', attention: '2' })
      assert.ok(['Normal', 'Unknown'].includes(actual.host), 'HOST never inherits stopped worker')
      assert.equal(actual.services, 'Down')
      assert.deepEqual(actual.rows, [
        { name: 'hello-api', status: 'Degraded', containers: [
          { name: 'api', state: 'running', status: 'Operational' },
          { name: 'worker', state: 'Stopped', status: 'Down' }
        ] },
        { name: 'wiki', status: 'Down', containers: [
          { name: 'wiki', state: 'running', status: 'Operational' }
        ] }
      ])
      assert.equal(actual.horizontalOverflow, false, `horizontal overflow at ${width}px (${variant})`)
      assert.equal(actual.clipped, false)
      assert.equal(actual.secondaryOverflow, false)
      if (variant === 'a') assert.equal(actual.secondary, null)
      else {
        assert.match(actual.secondary, /^Load .+ · Docker 2.4GB img \/ 12MB ctr \/ 4GB vol$/)
        assert.ok(actual.secondaryBox.width > 0)
      }
      const file = `${output}/yard-layout-${variant}-${width}.png`
      await page.screenshot({ path: file, fullPage: true, animations: 'disabled' })
      console.log(`${file} — ${JSON.stringify(actual)}`)
      await page.close()
    }
  }
} finally {
  await browser.close()
}
