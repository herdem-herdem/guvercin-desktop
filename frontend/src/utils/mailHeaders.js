export function extractRfc822Headers(rawText) {
  const text = `${rawText || ''}`.replace(/\r\n/g, '\n').replace(/\r/g, '\n')
  if (!text.trim()) return ''

  const separatorIndex = text.indexOf('\n\n')
  const headers = separatorIndex >= 0 ? text.slice(0, separatorIndex) : text
  return headers.trimEnd()
}

export function decodeMailRawBytes(bytes) {
  if (!bytes) return ''
  try {
    return new TextDecoder('utf-8', { fatal: false }).decode(bytes)
  } catch {
    return ''
  }
}

export function headersFromRawBytes(bytes) {
  return extractRfc822Headers(decodeMailRawBytes(bytes))
}

export function fullMessageFromRawBytes(bytes) {
  return decodeMailRawBytes(bytes).trimEnd()
}

export function buildHeaderFallback(mail, content, formatDate = (value) => value || '') {
  const from = content?.from_name && content?.from_address
    ? `${content.from_name} <${content.from_address}>`
    : (mail?.name && mail?.address ? `${mail.name} <${mail.address}>` : (mail?.address || mail?.name || 'Unknown'))

  const lines = [
    `Subject: ${content?.subject || mail?.subject || '(No Subject)'}`,
    `From: ${from}`,
  ]

  if (mail?.recipient_to) lines.push(`To: ${mail.recipient_to}`)
  if (content?.cc) lines.push(`Cc: ${content.cc}`)
  if (content?.bcc) lines.push(`Bcc: ${content.bcc}`)
  if (content?.date || mail?.date) lines.push(`Date: ${formatDate(content?.date || mail?.date)}`)
  if (mail?.message_id) lines.push(`Message-ID: ${mail.message_id}`)
  if (mail?.in_reply_to) lines.push(`In-Reply-To: ${mail.in_reply_to}`)
  if (mail?.references) lines.push(`References: ${mail.references}`)

  return lines.join('\n')
}

export function mailHeadersKey(mail, mailbox) {
  if (!mail?.id) return ''
  return `${mailbox || mail?.mailbox || 'INBOX'}:${mail.id}`
}

function sanitizeFilePart(value) {
  const cleaned = `${value || ''}`
    .replace(/[\\/:*?"<>|]+/g, '_')
    .replace(/\s+/g, ' ')
    .trim()
  return (cleaned || 'message').slice(0, 80).replace(/[._\s]+$/g, '') || 'message'
}

export function buildHeadersFileName(mail, content) {
  const subject = content?.subject || mail?.subject || 'message'
  return `${sanitizeFilePart(subject)}-source.txt`
}
