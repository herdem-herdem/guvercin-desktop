// Native system notifications for newly-arrived mail, plus the OS unread badge.
//
// New mail can land while the app is hidden in the tray or sitting in the
// background. When that happens we post a notification to the OS notification
// center (macOS Notification Center / Windows Action Center / Linux) so the
// user is alerted without keeping the window open.
//
// Click-to-open: the desktop notification plugin doesn't expose a reliable
// per-notification click callback, so we approximate it — clicking a
// notification brings the app to the foreground, and we treat "the window
// regained focus while a notification was pending" as the click, opening the
// most recently notified mail. Consumers subscribe via subscribeNotificationOpen.

let initialized = false
let permissionGranted = false
const openSubscribers = new Set()

// The most recent mail we posted a notification for, awaiting a click.
let pendingMail = null
// True once a notification has been posted while the window was unfocused.
let armed = false

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

function dispatchOpen(mail) {
  for (const cb of openSubscribers) {
    try {
      cb(mail)
    } catch (error) {
      console.error('notification-open subscriber failed:', error)
    }
  }
}

// Requests notification permission (once) and installs the focus listener that
// drains a pending click. Safe to call multiple times.
export async function initNotifications() {
  if (initialized || !isTauri()) return
  initialized = true

  try {
    const { isPermissionGranted, requestPermission } = await import('@tauri-apps/plugin-notification')
    permissionGranted = await isPermissionGranted()
    if (!permissionGranted) {
      const perm = await requestPermission()
      permissionGranted = perm === 'granted'
    }
  } catch (error) {
    console.error('Failed to initialize notifications:', error)
  }

  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    const win = getCurrentWindow()
    await win.onFocusChanged(({ payload: focused }) => {
      if (focused && armed && pendingMail) {
        const mail = pendingMail
        pendingMail = null
        armed = false
        dispatchOpen(mail)
      }
    })
  } catch {
    // Not inside Tauri or the API is unavailable; click-to-open is disabled.
  }
}

// Posts a native notification for a newly-arrived mail. No-op when the window
// is currently focused (the user is already here) or permission was denied.
export async function notifyNewMail(mail) {
  if (!isTauri() || !mail || !permissionGranted) return

  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    if (await getCurrentWindow().isFocused()) return
  } catch {
    // If we can't determine focus, err on the side of notifying.
  }

  pendingMail = mail
  armed = true

  try {
    const { sendNotification } = await import('@tauri-apps/plugin-notification')
    const sender = (mail.from_name || mail.name || mail.from || mail.address || '')
      .toString().trim() || 'New message'
    const subject = (mail.subject || '').toString().trim() || '(no subject)'
    sendNotification({ title: sender, body: subject })
  } catch (error) {
    console.error('Failed to send notification:', error)
  }
}

// Updates the OS unread badge (macOS dock / Linux launcher) and tray tooltip.
export async function setUnreadBadge(count) {
  if (!isTauri()) return
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke('set_unread_badge', { count: Math.max(0, Math.floor(count) || 0) })
  } catch (error) {
    console.error('Failed to set unread badge:', error)
  }
}

// Subscribe to "user clicked a mail notification" events. Returns an unsubscribe fn.
export function subscribeNotificationOpen(callback) {
  if (typeof callback !== 'function') return () => {}
  openSubscribers.add(callback)
  return () => {
    openSubscribers.delete(callback)
  }
}
