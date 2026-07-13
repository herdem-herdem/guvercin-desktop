/**
 * UI and display preferences.
 *
 * Stored in localStorage keyed by the active account id, so they work without
 * touching the Rust backend schema.
 */

export const UI_PREFERENCES_DEFAULTS = {
  // Remote images handling: 'auto' loads all, 'block' blocks all, 'prompt' asks per email
  remoteImageMode: 'auto',
  // Delay before marking emails as read when opened (in seconds). 0 = immediate
  markAsReadDelaySeconds: 0,
  // Thread/conversation view: 'on' or 'off'
  threadViewEnabled: false,
  // Message list display density: 'compact' or 'normal'
  messageListDensity: 'normal',
  // Preview panel position: 'right' or 'bottom'
  previewPanelPosition: 'right',
}

function storageKey(accountId) {
  return `ui_preferences_${accountId ?? 'default'}`
}

function clampNumber(value, fallback, min, max) {
  const n = Number(value)
  if (!Number.isFinite(n)) return fallback
  return Math.min(max, Math.max(min, Math.round(n)))
}

function coerceSettings(raw) {
  const base = { ...UI_PREFERENCES_DEFAULTS, ...(raw && typeof raw === 'object' ? raw : {}) }
  return {
    remoteImageMode: ['auto', 'block', 'prompt'].includes(base.remoteImageMode) ? base.remoteImageMode : 'auto',
    markAsReadDelaySeconds: clampNumber(base.markAsReadDelaySeconds, UI_PREFERENCES_DEFAULTS.markAsReadDelaySeconds, 0, 30),
    threadViewEnabled: Boolean(base.threadViewEnabled),
    messageListDensity: ['compact', 'normal'].includes(base.messageListDensity) ? base.messageListDensity : 'normal',
    previewPanelPosition: ['right', 'bottom'].includes(base.previewPanelPosition) ? base.previewPanelPosition : 'right',
  }
}

export function getUIPreferences(accountId) {
  try {
    const raw = localStorage.getItem(storageKey(accountId))
    if (!raw) return { ...UI_PREFERENCES_DEFAULTS }
    return coerceSettings(JSON.parse(raw))
  } catch {
    return { ...UI_PREFERENCES_DEFAULTS }
  }
}

export function saveUIPreferences(accountId, settings) {
  const coerced = coerceSettings(settings)
  try {
    localStorage.setItem(storageKey(accountId), JSON.stringify(coerced))
  } catch {
    /* storage full / unavailable — ignore */
  }
  return coerced
}
