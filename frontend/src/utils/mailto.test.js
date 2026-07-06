import test from 'node:test'
import assert from 'node:assert/strict'

const { parseMailtoUri } = await import('./mailto.js')

test('parseMailtoUri returns null for non-mailto input', () => {
  assert.equal(parseMailtoUri('https://example.com'), null)
  assert.equal(parseMailtoUri(''), null)
  assert.equal(parseMailtoUri(null), null)
})

test('parseMailtoUri parses a single recipient', () => {
  assert.deepEqual(parseMailtoUri('mailto:alice@example.com'), {
    to: 'alice@example.com',
  })
})

test('parseMailtoUri parses multiple path recipients and to param', () => {
  const draft = parseMailtoUri('mailto:a@x.com,b@x.com?to=c@x.com')
  assert.equal(draft.to, 'a@x.com, b@x.com, c@x.com')
})

test('parseMailtoUri decodes subject, body, cc and bcc', () => {
  const draft = parseMailtoUri(
    'mailto:a@x.com?subject=Hello%20World&body=Line%20one%0ALine%20two&cc=c@x.com&bcc=d@x.com',
  )
  assert.equal(draft.to, 'a@x.com')
  assert.equal(draft.subject, 'Hello World')
  assert.equal(draft.plainBody, 'Line one\nLine two')
  assert.equal(draft.cc, 'c@x.com')
  assert.equal(draft.bcc, 'd@x.com')
})

test('parseMailtoUri treats + as space in body', () => {
  const draft = parseMailtoUri('mailto:a@x.com?body=hi+there')
  assert.equal(draft.plainBody, 'hi there')
})

test('parseMailtoUri handles case-insensitive scheme and keys', () => {
  const draft = parseMailtoUri('MAILTO:a@x.com?SUBJECT=Hi')
  assert.equal(draft.to, 'a@x.com')
  assert.equal(draft.subject, 'Hi')
})

test('parseMailtoUri returns empty draft for bare mailto', () => {
  assert.deepEqual(parseMailtoUri('mailto:'), {})
})
