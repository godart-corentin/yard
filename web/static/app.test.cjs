const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const vm = require('node:vm')

function element (tag) {
  return {
    tag, children: [], textContent: '', className: '',
    append (...children) { this.children.push(...children) },
    setAttribute () {},
    addEventListener () {},
    classList: { toggle () {}, remove () {} }
  }
}

function texts (node) {
  return [node.textContent, ...node.children.flatMap(texts)].filter(Boolean).join(' ')
}

test('card shows each release image and warns about interrupted activation', () => {
  const document = {
    querySelector: () => element('div'),
    createElement: element
  }
  const source = fs.readFileSync(__dirname + '/app.js', 'utf8')
  const card = vm.runInNewContext(source + '\nrenderProject({ name: "demo", status: "operational", release: { tag: "old", services: [{ name: "api", image: "api:old" }, { name: "worker", image: "worker:old" }] }, pending_release: { tag: "new", status: "activating" } })', {
    document, fetch: () => new Promise(() => {}), setInterval: () => 0
  })
  const text = texts(card)
  assert.match(text, /api:old/)
  assert.match(text, /worker:old/)
  assert.match(text, /activating/)
  assert.match(text, /new/)
})
