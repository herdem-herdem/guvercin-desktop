// Parses a `mailto:` URI into a compose draft fragment.
//
// Handles the standard form:
//   mailto:alice@example.com,bob@example.com?subject=Hi&cc=...&bcc=...&body=...
// Recipients may appear in the path (comma-separated) and/or in `to` query
// params. All values are percent-decoded. Returns `null` when the input is not
// a usable mailto URI so callers can ignore unrelated deep links.

function decode(value) {
  if (typeof value !== 'string') return ''
  try {
    // mailto encodes spaces as %20; some clients use '+' too.
    return decodeURIComponent(value.replace(/\+/g, ' '))
  } catch {
    return value
  }
}

function joinRecipients(parts) {
  return parts
    .map((part) => decode(part).trim())
    .filter(Boolean)
    .join(', ')
}

export function parseMailtoUri(uri) {
  if (typeof uri !== 'string') return null
  const trimmed = uri.trim()
  if (!/^mailto:/i.test(trimmed)) return null

  const rest = trimmed.slice('mailto:'.length)
  const queryIndex = rest.indexOf('?')
  const pathPart = queryIndex === -1 ? rest : rest.slice(0, queryIndex)
  const queryPart = queryIndex === -1 ? '' : rest.slice(queryIndex + 1)

  const toParts = pathPart ? pathPart.split(',') : []
  const ccParts = []
  const bccParts = []
  let subject = ''
  let body = ''

  if (queryPart) {
    for (const pair of queryPart.split('&')) {
      if (!pair) continue
      const eq = pair.indexOf('=')
      const rawKey = eq === -1 ? pair : pair.slice(0, eq)
      const rawVal = eq === -1 ? '' : pair.slice(eq + 1)
      const key = decode(rawKey).trim().toLowerCase()
      switch (key) {
        case 'to':
          toParts.push(...rawVal.split(','))
          break
        case 'cc':
          ccParts.push(...rawVal.split(','))
          break
        case 'bcc':
          bccParts.push(...rawVal.split(','))
          break
        case 'subject':
          subject = decode(rawVal)
          break
        case 'body':
          body = decode(rawVal)
          break
        default:
          break
      }
    }
  }

  const draft = {}
  const to = joinRecipients(toParts)
  const cc = joinRecipients(ccParts)
  const bcc = joinRecipients(bccParts)
  if (to) draft.to = to
  if (cc) draft.cc = cc
  if (bcc) draft.bcc = bcc
  if (subject) draft.subject = subject
  if (body) draft.plainBody = body

  // A bare "mailto:" with nothing usable still opens an empty compose window.
  return draft
}
