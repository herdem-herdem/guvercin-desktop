// Bridges OS-level `mailto:` deep links to the compose UI.
//
// Deep links can arrive at any time: while the dashboard is open (hot) or
// during a cold start before the user has even selected an account. We can't
// open a compose window until an account is active, so incoming mailto drafts
// are buffered in a queue and drained by whichever consumer is ready
// (typically DashboardPage). The Tauri listeners are installed exactly once.

import { parseMailtoUri } from './mailto.js'

const queue = []
const subscribers = new Set()
let initialized = false

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

function dispatch(draft) {
  if (subscribers.size === 0) {
    queue.push(draft)
    return
  }
  for (const cb of subscribers) {
    try {
      cb(draft)
    } catch (error) {
      console.error('mailto subscriber failed:', error)
    }
  }
}

function handleUrls(urls) {
  if (!Array.isArray(urls)) return
  for (const url of urls) {
    const draft = parseMailtoUri(url)
    if (draft) dispatch(draft)
  }
}

// Installs the deep-link listeners. Safe to call multiple times.
export async function initMailtoInbox() {
  if (initialized || !isTauri()) return
  initialized = true
  try {
    const { onOpenUrl, getCurrent } = await import('@tauri-apps/plugin-deep-link')
    // URLs the app was launched with (cold start).
    try {
      const current = await getCurrent()
      handleUrls(current)
    } catch {
      // getCurrent is unavailable on some platforms; ignore.
    }
    // URLs delivered while the app is running (hot).
    await onOpenUrl(handleUrls)
  } catch (error) {
    console.error('Failed to initialize mailto deep-link handling:', error)
  }
}

// Request a fresh compose window from anywhere (e.g. the tray "New Mail"
// action). Routes through the same queue as mailto links, so if no consumer is
// mounted yet (the user is on a non-mail section) the request is buffered and
// drained once the mail UI subscribes.
export function requestNewCompose(draft = {}) {
  dispatch({ to: '', cc: '', bcc: '', subject: '', plainBody: '', ...draft })
}

// Subscribe to incoming mailto drafts. Immediately flushes any queued drafts
// that arrived before a subscriber was present. Returns an unsubscribe fn.
export function subscribeMailto(callback) {
  if (typeof callback !== 'function') return () => {}
  subscribers.add(callback)
  if (queue.length > 0) {
    const pending = queue.splice(0, queue.length)
    for (const draft of pending) {
      try {
        callback(draft)
      } catch (error) {
        console.error('mailto subscriber failed:', error)
      }
    }
  }
  return () => {
    subscribers.delete(callback)
  }
}
