// Bridges OS-level file associations (`.eml` / `.msg`) to the mail viewer.
//
// When the user double-clicks a message file, the OS launches (or re-focuses)
// Guvercin and hands it the file. macOS delivers this as a `file://` deep link;
// Windows/Linux pass the path as a launch argument that the deep-link plugin
// surfaces the same way. Like mailto links these can arrive during a cold start
// before an account is active, so paths are buffered in a queue and drained by
// whichever consumer (typically DashboardPage) is ready. Listeners install once.

const queue = []
const subscribers = new Set()
let initialized = false

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

// Decodes a `file://` URL (or bare path) into a filesystem path and keeps only
// message files we know how to import. Returns null for anything else so the
// shared deep-link stream (which also carries mailto: links) is ignored safely.
function toEmlPath(url) {
  if (typeof url !== 'string') return null
  let path = url.trim()
  if (!path) return null

  if (/^file:\/\//i.test(path)) {
    path = path.replace(/^file:\/\/(localhost)?/i, '')
    try {
      path = decodeURIComponent(path)
    } catch {
      // Leave the raw path if it isn't valid percent-encoding.
    }
  } else if (/^[a-z][a-z0-9+.-]*:/i.test(path)) {
    // Some other scheme (mailto:, tel:, …) — not a file.
    return null
  }

  const lower = path.toLowerCase()
  if (!lower.endsWith('.eml') && !lower.endsWith('.msg')) return null
  return path
}

function dispatch(path) {
  if (subscribers.size === 0) {
    queue.push(path)
    return
  }
  for (const cb of subscribers) {
    try {
      cb(path)
    } catch (error) {
      console.error('eml subscriber failed:', error)
    }
  }
}

function handleUrls(urls) {
  if (!Array.isArray(urls)) return
  for (const url of urls) {
    const path = toEmlPath(url)
    if (path) dispatch(path)
  }
}

// Installs the deep-link listeners. Safe to call multiple times.
export async function initEmlInbox() {
  if (initialized || !isTauri()) return
  initialized = true
  try {
    const { onOpenUrl, getCurrent } = await import('@tauri-apps/plugin-deep-link')
    // Files the app was launched with (cold start).
    try {
      const current = await getCurrent()
      handleUrls(current)
    } catch {
      // getCurrent is unavailable on some platforms; ignore.
    }
    // Files delivered while the app is running (hot).
    await onOpenUrl(handleUrls)
  } catch (error) {
    console.error('Failed to initialize eml file-association handling:', error)
  }
}

// Subscribe to incoming file paths. Immediately flushes any paths that arrived
// before a subscriber was present. Returns an unsubscribe fn.
export function subscribeEml(callback) {
  if (typeof callback !== 'function') return () => {}
  subscribers.add(callback)
  if (queue.length > 0) {
    const pending = queue.splice(0, queue.length)
    for (const path of pending) {
      try {
        callback(path)
      } catch (error) {
        console.error('eml subscriber failed:', error)
      }
    }
  }
  return () => {
    subscribers.delete(callback)
  }
}
