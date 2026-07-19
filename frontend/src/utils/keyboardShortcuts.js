/**
 * Keyboard shortcuts registry, storage, and matching engine.
 *
 * Shortcuts are app-wide (not per-account), so they live under a single global
 * localStorage key. Each shortcut has a stable `id`, a default key combo, and a
 * default enabled flag. Users can rebind the combo or toggle it on/off; those
 * overrides are stored and merged over the defaults on read.
 *
 * Combos are stored in a canonical, platform-neutral string form. The token
 * `Ctrl` means the platform's primary command modifier — Cmd (⌘) on macOS,
 * Ctrl elsewhere — so a single default like `Ctrl+N` does the right thing on
 * every OS. Modifiers are always serialized in the fixed order Ctrl, Alt,
 * Shift, followed by the main key, e.g. `Ctrl+Shift+R`, `Delete`, `E`.
 */

export const isMac = typeof navigator !== 'undefined'
  && /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent || '')

const STORAGE_KEY = 'keyboard_shortcuts_global'

/** Ordered category metadata (id → human label). */
export const SHORTCUT_CATEGORIES = [
  { id: 'mail', label: 'Mail Actions' },
  { id: 'navigation', label: 'Navigation & Tabs' },
  { id: 'view', label: 'View & Layout' },
  { id: 'compose', label: 'Compose' },
  { id: 'application', label: 'Application' },
]

/**
 * The full shortcut catalogue. `defaultKeys: null` means "no default binding"
 * (user can assign one). `enabled: false` means the shortcut ships defined but
 * inactive — the user can turn it on. The commonly-known actions are enabled
 * with conventional bindings by default.
 */
export const SHORTCUT_DEFINITIONS = [
  /* ── Mail actions ─────────────────────────────────────────────── */
  { id: 'compose_new', category: 'mail', label: 'New message', defaultKeys: 'Ctrl+N', enabled: true },
  { id: 'reply', category: 'mail', label: 'Reply', defaultKeys: 'Ctrl+R', enabled: true },
  { id: 'reply_all', category: 'mail', label: 'Reply all', defaultKeys: 'Ctrl+Shift+R', enabled: true },
  { id: 'forward', category: 'mail', label: 'Forward', defaultKeys: 'Ctrl+Shift+F', enabled: true },
  { id: 'delete', category: 'mail', label: 'Delete', defaultKeys: 'Delete', enabled: true },
  { id: 'move_to_trash', category: 'mail', label: 'Move to Trash', defaultKeys: 'Ctrl+Backspace', enabled: false },
  { id: 'archive', category: 'mail', label: 'Archive', defaultKeys: 'E', enabled: true },
  { id: 'mark_spam', category: 'mail', label: 'Mark as spam', defaultKeys: 'Shift+S', enabled: true },
  { id: 'toggle_read', category: 'mail', label: 'Toggle read / unread', defaultKeys: 'Ctrl+U', enabled: true },
  { id: 'move_to_folder', category: 'mail', label: 'Move to folder…', defaultKeys: 'V', enabled: false },
  { id: 'add_label', category: 'mail', label: 'Add label…', defaultKeys: 'L', enabled: false },
  { id: 'print', category: 'mail', label: 'Print message', defaultKeys: 'Ctrl+P', enabled: true },
  { id: 'select_all', category: 'mail', label: 'Select all messages', defaultKeys: 'Ctrl+Shift+A', enabled: false },

  /* ── Navigation & tabs ────────────────────────────────────────── */
  { id: 'search', category: 'navigation', label: 'Search mail', defaultKeys: 'Ctrl+F', enabled: true },
  { id: 'advanced_search', category: 'navigation', label: 'Advanced search', defaultKeys: 'Ctrl+Shift+K', enabled: false },
  { id: 'refresh', category: 'navigation', label: 'Refresh / sync', defaultKeys: 'F5', enabled: true },
  { id: 'next_message', category: 'navigation', label: 'Next message', defaultKeys: 'J', enabled: false },
  { id: 'previous_message', category: 'navigation', label: 'Previous message', defaultKeys: 'K', enabled: false },
  { id: 'close_tab', category: 'navigation', label: 'Close current tab', defaultKeys: 'Ctrl+W', enabled: true },
  { id: 'go_mail', category: 'navigation', label: 'Go to Mail', defaultKeys: 'Ctrl+Alt+1', enabled: false },
  { id: 'go_calendar', category: 'navigation', label: 'Go to Calendar', defaultKeys: 'Ctrl+Alt+2', enabled: false },
  { id: 'go_contacts', category: 'navigation', label: 'Go to Contacts', defaultKeys: 'Ctrl+Alt+3', enabled: false },
  { id: 'go_todo', category: 'navigation', label: 'Go to Todo', defaultKeys: 'Ctrl+Alt+4', enabled: false },

  /* ── View & layout ────────────────────────────────────────────── */
  { id: 'toggle_fullscreen', category: 'view', label: 'Toggle reading fullscreen', defaultKeys: 'F11', enabled: false },
  { id: 'toggle_thread_view', category: 'view', label: 'Toggle conversation view', defaultKeys: null, enabled: false },
  { id: 'toggle_preview_panel', category: 'view', label: 'Toggle preview panel position', defaultKeys: null, enabled: false },
  { id: 'zoom_in', category: 'view', label: 'Zoom in', defaultKeys: 'Ctrl+=', enabled: false },
  { id: 'zoom_out', category: 'view', label: 'Zoom out', defaultKeys: 'Ctrl+-', enabled: false },
  { id: 'zoom_reset', category: 'view', label: 'Reset zoom', defaultKeys: 'Ctrl+0', enabled: false },

  /* ── Compose (active within a compose view) ───────────────────── */
  { id: 'compose_send', category: 'compose', label: 'Send message', defaultKeys: 'Ctrl+Enter', enabled: true },
  { id: 'compose_save_draft', category: 'compose', label: 'Save draft', defaultKeys: 'Ctrl+S', enabled: true },
  { id: 'compose_discard', category: 'compose', label: 'Discard draft', defaultKeys: null, enabled: false },
  { id: 'compose_open_window', category: 'compose', label: 'Pop out to window', defaultKeys: null, enabled: false },

  /* ── Application ──────────────────────────────────────────────── */
  { id: 'open_settings', category: 'application', label: 'Open settings', defaultKeys: 'Ctrl+,', enabled: true },
  { id: 'lock_screen', category: 'application', label: 'Lock screen', defaultKeys: null, enabled: false },
]

const DEFINITION_BY_ID = SHORTCUT_DEFINITIONS.reduce((acc, def) => {
  acc[def.id] = def
  return acc
}, {})

/* ── Key / combo normalization ────────────────────────────────────── */

const NAMED_KEYS = {
  ' ': 'Space',
  spacebar: 'Space',
  esc: 'Escape',
  escape: 'Escape',
  del: 'Delete',
  delete: 'Delete',
  backspace: 'Backspace',
  enter: 'Enter',
  return: 'Enter',
  tab: 'Tab',
  arrowup: 'ArrowUp',
  arrowdown: 'ArrowDown',
  arrowleft: 'ArrowLeft',
  arrowright: 'ArrowRight',
  up: 'ArrowUp',
  down: 'ArrowDown',
  left: 'ArrowLeft',
  right: 'ArrowRight',
  home: 'Home',
  end: 'End',
  pageup: 'PageUp',
  pagedown: 'PageDown',
}

const MODIFIER_KEY_NAMES = new Set(['Control', 'Shift', 'Alt', 'Meta', 'CapsLock', 'OS'])

/** Convert a raw KeyboardEvent.key into our canonical key token. */
function normalizeKey(rawKey) {
  if (!rawKey) return null
  if (MODIFIER_KEY_NAMES.has(rawKey)) return null
  const lower = rawKey.toLowerCase()
  if (NAMED_KEYS[lower]) return NAMED_KEYS[lower]
  // Function keys F1..F24
  if (/^f\d{1,2}$/.test(lower)) return lower.toUpperCase()
  // Single letters → uppercase; digits & symbols kept verbatim
  if (rawKey.length === 1) {
    return /[a-z]/i.test(rawKey) ? rawKey.toUpperCase() : rawKey
  }
  return rawKey
}

/**
 * Build the canonical combo string from a keyboard event, or null if only
 * modifier keys are held (no actionable key).
 */
export function comboFromEvent(event) {
  const key = normalizeKey(event.key)
  if (!key) return null
  const parts = []
  const primary = isMac ? event.metaKey : event.ctrlKey
  if (primary) parts.push('Ctrl')
  if (event.altKey) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')
  parts.push(key)
  return parts.join('+')
}

/** True when the combo carries a primary (Ctrl/Cmd) or Alt modifier. */
function hasCommandModifier(combo) {
  return /(^|\+)Ctrl(\+|$)/.test(combo) || /(^|\+)Alt(\+|$)/.test(combo)
}

const SYMBOL_LABELS = {
  Space: 'Space',
  ArrowUp: '↑',
  ArrowDown: '↓',
  ArrowLeft: '←',
  ArrowRight: '→',
}

/** Human-readable rendering of a combo for display in the UI. */
export function formatCombo(combo) {
  if (!combo) return ''
  return combo
    .split('+')
    .map((part) => {
      if (part === 'Ctrl') return isMac ? '⌘' : 'Ctrl'
      if (part === 'Alt') return isMac ? '⌥' : 'Alt'
      if (part === 'Shift') return isMac ? '⇧' : 'Shift'
      return SYMBOL_LABELS[part] || part
    })
    .join(isMac ? '' : '+')
}

/* ── Storage ──────────────────────────────────────────────────────── */

function readOverrides() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw)
    return parsed && typeof parsed === 'object' ? parsed : {}
  } catch {
    return {}
  }
}

function writeOverrides(overrides) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(overrides))
  } catch {
    /* storage full / unavailable — ignore */
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event('guvercin-shortcuts-changed'))
  }
}

/**
 * Resolved list of every shortcut merged with the user's overrides. Each item:
 * { id, category, label, keys, enabled, defaultKeys, defaultEnabled, isCustom }
 */
export function getShortcuts() {
  const overrides = readOverrides()
  return SHORTCUT_DEFINITIONS.map((def) => {
    const override = overrides[def.id] || {}
    const keys = Object.prototype.hasOwnProperty.call(override, 'keys')
      ? override.keys
      : def.defaultKeys
    const enabled = typeof override.enabled === 'boolean' ? override.enabled : def.enabled
    return {
      id: def.id,
      category: def.category,
      label: def.label,
      keys: keys || null,
      enabled,
      defaultKeys: def.defaultKeys,
      defaultEnabled: def.enabled,
      isCustom: (keys || null) !== def.defaultKeys || enabled !== def.enabled,
    }
  })
}

/** Runtime lookup map: canonical combo → action id, for enabled shortcuts only. */
export function getActiveComboMap() {
  const map = {}
  for (const s of getShortcuts()) {
    if (s.enabled && s.keys) map[s.keys] = s.id
  }
  return map
}

/** Persist a single shortcut's keys and/or enabled flag. */
export function setShortcut(id, patch) {
  if (!DEFINITION_BY_ID[id]) return
  const overrides = readOverrides()
  const next = { ...(overrides[id] || {}) }
  if (Object.prototype.hasOwnProperty.call(patch, 'keys')) next.keys = patch.keys || null
  if (Object.prototype.hasOwnProperty.call(patch, 'enabled')) next.enabled = Boolean(patch.enabled)
  overrides[id] = next
  writeOverrides(overrides)
}

/** Drop overrides for one shortcut, restoring its defaults. */
export function resetShortcut(id) {
  const overrides = readOverrides()
  if (overrides[id]) {
    delete overrides[id]
    writeOverrides(overrides)
  }
}

/** Restore every shortcut to its shipped default. */
export function resetAllShortcuts() {
  writeOverrides({})
}

/**
 * Find which enabled shortcut (if any) an event matches, honoring the
 * editable-field guard: shortcuts without a command modifier (Ctrl/Cmd/Alt)
 * are ignored while the user is typing in an input, textarea, select, or
 * contenteditable element, so plain keys like `E` or `Delete` don't hijack
 * text entry.
 */
export function matchShortcut(event, comboMap) {
  const combo = comboFromEvent(event)
  if (!combo) return null
  const id = comboMap[combo]
  if (!id) return null
  if (!hasCommandModifier(combo) && isEditableTarget(event.target)) return null
  return id
}

function isEditableTarget(target) {
  if (!target || typeof target !== 'object') return false
  const tag = target.tagName
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true
  if (target.isContentEditable) return true
  return false
}

/**
 * Detect combo collisions: returns the id of another enabled shortcut already
 * bound to `combo`, or null. Used by the settings editor to warn on conflicts.
 */
export function findConflict(combo, excludeId) {
  if (!combo) return null
  for (const s of getShortcuts()) {
    if (s.id === excludeId) continue
    if (s.enabled && s.keys === combo) return s.id
  }
  return null
}
