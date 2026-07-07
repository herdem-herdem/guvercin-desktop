import { htmlToPlainText } from './composeHtml.js'

/**
 * Per-account "Compose / Send" preferences.
 *
 * These are stored in localStorage keyed by the active account id (mirroring
 * the app's existing localStorage fallback pattern for account-scoped
 * settings), so they work without touching the Rust backend schema.
 */

export const COMPOSE_SETTINGS_DEFAULTS = {
  // 'plain' | 'html' — the initial format for a brand-new message.
  defaultFormat: 'plain',
  // Font applied to new HTML messages. '' means "use the editor default".
  fontFamily: '',
  fontSize: '',
  // HTML signature (raw HTML). Empty string means "no signature".
  signature: '',
  // Undo-send window in seconds. 0 sends immediately (no undo notice).
  undoSendSeconds: 10,
  // Auto-save drafts every N seconds while composing. 0 disables auto-save.
  autosaveSeconds: 0,
  // Reply body layout: type above the quote ('top') or below it ('bottom').
  replyQuotePosition: 'top',
  // Always add the account's own address to Cc.
  autoCcSelf: false,
}

const MIN_UNDO_SECONDS = 0
const MAX_UNDO_SECONDS = 60
const MIN_AUTOSAVE_SECONDS = 0
const MAX_AUTOSAVE_SECONDS = 600

function storageKey(accountId) {
  return `compose_settings_${accountId ?? 'default'}`
}

function clampNumber(value, fallback, min, max) {
  const n = Number(value)
  if (!Number.isFinite(n)) return fallback
  return Math.min(max, Math.max(min, Math.round(n)))
}

function coerceSettings(raw) {
  const base = { ...COMPOSE_SETTINGS_DEFAULTS, ...(raw && typeof raw === 'object' ? raw : {}) }
  return {
    defaultFormat: base.defaultFormat === 'html' ? 'html' : 'plain',
    fontFamily: typeof base.fontFamily === 'string' ? base.fontFamily.trim() : '',
    fontSize: typeof base.fontSize === 'string' ? base.fontSize.trim() : '',
    signature: typeof base.signature === 'string' ? base.signature : '',
    undoSendSeconds: clampNumber(base.undoSendSeconds, COMPOSE_SETTINGS_DEFAULTS.undoSendSeconds, MIN_UNDO_SECONDS, MAX_UNDO_SECONDS),
    autosaveSeconds: clampNumber(base.autosaveSeconds, COMPOSE_SETTINGS_DEFAULTS.autosaveSeconds, MIN_AUTOSAVE_SECONDS, MAX_AUTOSAVE_SECONDS),
    replyQuotePosition: base.replyQuotePosition === 'bottom' ? 'bottom' : 'top',
    autoCcSelf: Boolean(base.autoCcSelf),
  }
}

export function getComposeSettings(accountId) {
  try {
    const raw = localStorage.getItem(storageKey(accountId))
    if (!raw) return { ...COMPOSE_SETTINGS_DEFAULTS }
    return coerceSettings(JSON.parse(raw))
  } catch {
    return { ...COMPOSE_SETTINGS_DEFAULTS }
  }
}

export function saveComposeSettings(accountId, settings) {
  const coerced = coerceSettings(settings)
  try {
    localStorage.setItem(storageKey(accountId), JSON.stringify(coerced))
  } catch {
    /* storage full / unavailable — ignore */
  }
  return coerced
}

/* ─── Body / draft composition helpers ─────────────────────────────── */

function signaturePlain(signature) {
  const html = (signature || '').trim()
  if (!html) return ''
  const text = htmlToPlainText(html).trim()
  return text ? `-- \n${text}` : ''
}

function signatureHtmlBlock(signature) {
  const html = (signature || '').trim()
  if (!html) return ''
  return `<br><br>-- <br>${html}`
}

function fontWrapperStyle(prefs) {
  const parts = []
  if (prefs.fontFamily) parts.push(`font-family:${prefs.fontFamily}, sans-serif`)
  if (prefs.fontSize) parts.push(`font-size:${prefs.fontSize}`)
  return parts.join(';')
}

/**
 * Body fields for a brand-new (empty) message, honouring the default format,
 * default font (HTML only) and signature.
 */
export function buildNewMailFields(prefs) {
  if (prefs.defaultFormat === 'html') {
    const style = fontWrapperStyle(prefs)
    const inner = `<p></p>${signatureHtmlBlock(prefs.signature)}`
    const htmlBody = style ? `<div style="${style}">${inner}</div>` : inner
    return { format: 'html', htmlBody, plainBody: '' }
  }
  const sig = signaturePlain(prefs.signature)
  return { format: 'plain', htmlBody: '', plainBody: sig ? `\n\n${sig}` : '' }
}

/**
 * Plain-text reply/forward body combining the signature, the quoted block and
 * the configured quote position.
 */
export function buildReplyPlainBody(quoteBlock, prefs) {
  const quote = `${quoteBlock || ''}`
  const sig = signaturePlain(prefs.signature)

  if (prefs.replyQuotePosition === 'bottom') {
    // Quote first, blank line to type below it, then signature.
    const tail = sig ? `\n\n\n${sig}` : '\n\n'
    return `${quote}${tail}`
  }

  // Top-posting (default): blank lines to type, optional signature, then quote.
  const head = sig ? `\n\n${sig}\n\n` : '\n\n'
  return `${head}${quote}`
}

/** Extra Cc recipients implied by the "Cc myself" preference. */
export function autoCcRecipients(prefs, selfEmail) {
  const self = `${selfEmail || ''}`.trim()
  return prefs.autoCcSelf && self ? [self] : []
}
