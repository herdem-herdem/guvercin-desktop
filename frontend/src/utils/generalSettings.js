/**
 * General and behavior preferences.
 *
 * Stored in localStorage for client-side settings, with some preferences
 * requiring Tauri IPC calls for system-level changes (launch at login, tray behavior).
 */

export const GENERAL_SETTINGS_DEFAULTS = {
  // Language preference: 'en', 'tr', etc
  language: 'en',
  // Close behavior: 'tray' = close to tray, 'quit' = quit app
  closeAction: 'tray',
  // Auto-sync interval in minutes (0 = disabled)
  autoSyncInterval: 5,
  // Show notifications for sync events
  showSyncNotifications: true,
  // Auto-check for updates
  autoCheckUpdates: true,
}

function storageKey(accountId) {
  return accountId ? `general_settings_${accountId}` : 'general_settings_global'
}

function clampNumber(value, fallback, min, max) {
  const n = Number(value)
  if (!Number.isFinite(n)) return fallback
  return Math.min(max, Math.max(min, Math.round(n)))
}

function coerceSettings(raw) {
  const base = { ...GENERAL_SETTINGS_DEFAULTS, ...(raw && typeof raw === 'object' ? raw : {}) }
  return {
    language: typeof base.language === 'string' ? base.language : 'en',
    closeAction: ['tray', 'quit'].includes(base.closeAction) ? base.closeAction : 'tray',
    autoSyncInterval: clampNumber(base.autoSyncInterval, GENERAL_SETTINGS_DEFAULTS.autoSyncInterval, 0, 60),
    showSyncNotifications: Boolean(base.showSyncNotifications),
    autoCheckUpdates: Boolean(base.autoCheckUpdates),
  }
}

export function getGeneralSettings(accountId) {
  try {
    const raw = localStorage.getItem(storageKey(accountId))
    if (!raw) return { ...GENERAL_SETTINGS_DEFAULTS }
    return coerceSettings(JSON.parse(raw))
  } catch {
    return { ...GENERAL_SETTINGS_DEFAULTS }
  }
}

export function saveGeneralSettings(accountId, settings) {
  const coerced = coerceSettings(settings)
  try {
    localStorage.setItem(storageKey(accountId), JSON.stringify(coerced))
  } catch {
    /* storage full / unavailable — ignore */
  }
  return coerced
}

/**
 * Request Tauri to enable/disable launch at login.
 * Requires: invoke permission for 'set_launch_at_login' command.
 */
export async function setLaunchAtLogin(enabled) {
  try {
    if (!window.__TAURI__) {
      console.warn('Tauri not available')
      return false
    }
    const { invoke } = window.__TAURI__
    await invoke('set_launch_at_login', { enabled })
    return true
  } catch (e) {
    console.error('Failed to set launch at login:', e)
    return false
  }
}

/**
 * Query current launch-at-login status.
 * Requires: invoke permission for 'get_launch_at_login' command.
 */
export async function getLaunchAtLogin() {
  try {
    if (!window.__TAURI__) return false
    const { invoke } = window.__TAURI__
    return await invoke('get_launch_at_login')
  } catch (e) {
    console.error('Failed to get launch at login:', e)
    return false
  }
}

/**
 * Language display names mapping
 */
const LANGUAGE_NAMES = {
  'ar': 'العربية',
  'ar-bh': 'العربية (البحرين)',
  'ar-ps': 'العربية (فلسطين)',
  'az': 'Azərbaycanca',
  'bg': 'Български',
  'bn': 'বাংলা',
  'bs': 'Bosanski',
  'ca': 'Català',
  'cs': 'Čeština',
  'da': 'Dansk',
  'de': 'Deutsch',
  'el': 'Ελληνικά',
  'en': 'English',
  'es': 'Español',
  'et': 'Eesti',
  'fa': 'فارسی',
  'fi': 'Suomi',
  'fil': 'Filipino',
  'fr': 'Français',
  'hi': 'हिन्दी',
  'hr': 'Hrvatski',
  'hu': 'Magyar',
  'hy': 'Հայերեն',
  'id': 'Bahasa Indonesia',
  'is': 'Íslenska',
  'it': 'Italiano',
  'ja': '日本語',
  'ka': 'ქართული',
  'kk': 'Қазақ',
  'ko': '한국어',
  'ky': 'Кыргызча',
  'lo': 'ລາວ',
  'lt': 'Lietuvių',
  'lv': 'Latviešu',
  'mk': 'Македонски',
  'mn': 'Монгол',
  'ms': 'Bahasa Melayu',
  'my': 'မြန်မာ',
  'ne': 'नेपाली',
  'nl': 'Nederlands',
  'no': 'Norsk',
  'pa': 'ਪੰਜਾਬੀ',
  'pl': 'Polski',
  'ps': 'پښتو',
  'pt': 'Português',
  'ro': 'Română',
  'ru': 'Русский',
  'si': 'සිංහල',
  'sk': 'Slovenčina',
  'sl': 'Slovenščina',
  'so': 'Soomaali',
  'sq': 'Shqip',
  'sq-xk': 'Shqip (Kosovo)',
  'sr': 'Српски',
  'sv': 'Svenska',
  'sw': 'Swahili',
  'th': 'ไทย',
  'tk': 'Türkmençe',
  'tr': 'Türkçe',
  'uk': 'Українська',
  'ur': 'اردو',
  'uz': 'Oʻzbekcha',
  'vi': 'Tiếng Việt',
  'zh': '中文',
}

/**
 * Get list of available languages for the UI (dynamically from i18n resources).
 * Falls back to hardcoded language names mapping if not available.
 */
export function getAvailableLanguages() {
  try {
    // Try to import i18n module dynamically to get available languages
    let availableLangs = []
    try {
      // This will work if called from a React component context
      // For utility functions, we use a fallback approach
      const iframeCheck = typeof window !== 'undefined' && window.__AVAILABLE_LANGUAGES__
      if (iframeCheck) {
        availableLangs = window.__AVAILABLE_LANGUAGES__
      }
    } catch (e) {
      // Fallback: return hardcoded commonly used languages
      availableLangs = Object.keys(LANGUAGE_NAMES)
    }

    if (!availableLangs.length) {
      availableLangs = Object.keys(LANGUAGE_NAMES)
    }

    return availableLangs
      .map(code => ({
        code,
        label: LANGUAGE_NAMES[code] || code
      }))
      .sort((a, b) => a.label.localeCompare(b.label, 'en', { numeric: true }))
  } catch (e) {
    console.error('Failed to get available languages:', e)
    // Fallback to a minimal set
    return [
      { code: 'en', label: 'English' },
      { code: 'tr', label: 'Türkçe' },
    ]
  }
}
