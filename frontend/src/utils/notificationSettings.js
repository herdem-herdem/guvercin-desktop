/**
 * Per-account notification preferences.
 *
 * Stored in localStorage keyed by the active account id (mirroring the
 * per-account pattern used by composeSettings.js), so they work without
 * touching the Rust backend schema.
 */

export const NOTIFICATION_SETTINGS_DEFAULTS = {
  // Master switch for native desktop notifications.
  enabled: true,
  // Notification sound: 'default' plays the OS sound, 'none' is silent.
  soundMode: 'default',
  // Show the sender and subject in the notification, or a generic message.
  showPreview: true,
  // Do Not Disturb / quiet hours. When active, notifications are suppressed
  // (the in-app notification center and unread badge still update).
  dndEnabled: false,
  quietStart: '22:00',
  quietEnd: '07:00',
  // Which senders trigger a notification: 'all' or only 'vip' addresses.
  senderMode: 'all',
  // Lowercased VIP sender addresses (used when senderMode === 'vip').
  vipSenders: [],
  // OS dock/launcher badge: 'unread' count, 'total' count, or 'off'.
  badgeMode: 'unread',
}

function storageKey(accountId) {
  return `notification_settings_${accountId ?? 'default'}`
}

const TIME_RE = /^([01]?\d|2[0-3]):([0-5]\d)$/

function coerceTime(value, fallback) {
  const v = `${value || ''}`.trim()
  if (!TIME_RE.test(v)) return fallback
  const [h, m] = v.split(':')
  return `${h.padStart(2, '0')}:${m}`
}

function coerceSenderList(value) {
  const list = Array.isArray(value) ? value : []
  const seen = new Set()
  const out = []
  list.forEach((entry) => {
    const addr = `${entry || ''}`.trim().toLowerCase()
    if (!addr || seen.has(addr)) return
    seen.add(addr)
    out.push(addr)
  })
  return out
}

function coerceSettings(raw) {
  const base = { ...NOTIFICATION_SETTINGS_DEFAULTS, ...(raw && typeof raw === 'object' ? raw : {}) }
  return {
    enabled: Boolean(base.enabled),
    soundMode: base.soundMode === 'none' ? 'none' : 'default',
    showPreview: Boolean(base.showPreview),
    dndEnabled: Boolean(base.dndEnabled),
    quietStart: coerceTime(base.quietStart, NOTIFICATION_SETTINGS_DEFAULTS.quietStart),
    quietEnd: coerceTime(base.quietEnd, NOTIFICATION_SETTINGS_DEFAULTS.quietEnd),
    senderMode: base.senderMode === 'vip' ? 'vip' : 'all',
    vipSenders: coerceSenderList(base.vipSenders),
    badgeMode: ['unread', 'total', 'off'].includes(base.badgeMode) ? base.badgeMode : 'unread',
  }
}

export function getNotificationSettings(accountId) {
  try {
    const raw = localStorage.getItem(storageKey(accountId))
    if (!raw) return { ...NOTIFICATION_SETTINGS_DEFAULTS }
    return coerceSettings(JSON.parse(raw))
  } catch {
    return { ...NOTIFICATION_SETTINGS_DEFAULTS }
  }
}

export function saveNotificationSettings(accountId, settings) {
  const coerced = coerceSettings(settings)
  try {
    localStorage.setItem(storageKey(accountId), JSON.stringify(coerced))
  } catch {
    /* storage full / unavailable — ignore */
  }
  return coerced
}

/* ─── Runtime helpers ──────────────────────────────────────────────── */

function toMinutes(hhmm) {
  const [h, m] = `${hhmm}`.split(':').map(Number)
  return (h * 60) + m
}

/** True when `date` falls inside the configured quiet-hours window. */
export function isWithinQuietHours(prefs, date = new Date()) {
  if (!prefs?.dndEnabled) return false
  const start = toMinutes(prefs.quietStart)
  const end = toMinutes(prefs.quietEnd)
  if (start === end) return false
  const now = (date.getHours() * 60) + date.getMinutes()
  if (start < end) return now >= start && now < end
  // Window wraps past midnight (e.g. 22:00 → 07:00).
  return now >= start || now < end
}

function mailSenderAddress(mail) {
  return `${mail?.address || mail?.from || ''}`
    .trim()
    .toLowerCase()
    .replace(/^.*<([^<>]+)>.*$/, '$1')
    .trim()
}

/** Whether a native notification should fire for this mail given the prefs. */
export function shouldNotifyForMail(prefs, mail, date = new Date()) {
  if (!prefs?.enabled) return false
  if (isWithinQuietHours(prefs, date)) return false
  if (prefs.senderMode === 'vip') {
    const addr = mailSenderAddress(mail)
    if (!addr || !prefs.vipSenders.includes(addr)) return false
  }
  return true
}
