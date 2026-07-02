import test from 'node:test'
import assert from 'node:assert/strict'
import { JSDOM } from 'jsdom'
import { sanitizeMailHtml } from './externalLinks.js'

function withDom(cb) {
  const win = new JSDOM('<!doctype html><html><body></body></html>').window
  const oldDOMParser = global.DOMParser
  try {
    global.DOMParser = win.DOMParser
    cb()
  } finally {
    if (oldDOMParser === undefined) delete global.DOMParser
    else global.DOMParser = oldDOMParser
  }
}

test('sanitizeMailHtml moves first srcset candidate to src when src is missing', () => {
  withDom(() => {
    const html = '<img srcset="https://example.com/a.jpg 1x, https://example.com/b.jpg 2x" loading="lazy">'
    const out = sanitizeMailHtml(html)
    assert.match(out, /src="https:\/\/example\.com\/a\.jpg"/)
    assert.doesNotMatch(out, /srcset=/)
    assert.doesNotMatch(out, /loading=/)
  })
})

test('sanitizeMailHtml ignores javascript: srcset candidates and removes srcset', () => {
  withDom(() => {
    const html = '<img srcset="javascript:alert(1), https://example.com/c.jpg 2x">'
    const out = sanitizeMailHtml(html)
    assert.doesNotMatch(out, /src="javascript:/)
    // srcset should be removed even if no src was produced
    assert.doesNotMatch(out, /srcset=/)
  })
})
