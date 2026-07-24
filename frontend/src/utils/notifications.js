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

// The most recent thing we posted a notification for, awaiting a click. Shape:
// { kind: 'mail' | 'calendar' | 'todo', payload }. `payload` is the mail object
// for mail, or a small { id } descriptor for calendar/todo reminders.
let pendingOpen = null
// True once a notification has been posted and is awaiting a focus-click.
let armed = false

// The Tauri notification plugin identifies active notifications by a numeric id.
// Mail ids can be strings, so we map each mail id to a generated numeric id and
// remember it, so we can remove that exact notification once the mail is opened.
const mailIdToNotificationId = new Map()
let nextNotificationId = 1

function notificationIdForMail(mailId) {
  const key = String(mailId)
  let id = mailIdToNotificationId.get(key)
  if (id == null) {
    id = nextNotificationId++
    mailIdToNotificationId.set(key, id)
  }
  return id
}

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

function dispatchOpen(event) {
  for (const cb of openSubscribers) {
    try {
      cb(event)
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
      if (focused && armed && pendingOpen) {
        const event = pendingOpen
        pendingOpen = null
        armed = false
        dispatchOpen(event)
      }
    })
  } catch {
    // Not inside Tauri or the API is unavailable; click-to-open is disabled.
  }
}

// Posts a native notification for a newly-arrived mail. No-op when the window
// is currently focused (the user is already here) or permission was denied.
//
// options: { showPreview?: boolean, sound?: boolean }
//   showPreview (default true) — show the sender/subject; when false a generic
//     message is shown so the content stays private on the lock screen.
//   sound (default true) — play the OS notification sound.
export async function notifyNewMail(mail, options = {}) {
  if (!isTauri() || !mail || !permissionGranted) return

  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    if (await getCurrentWindow().isFocused()) return
  } catch {
    // If we can't determine focus, err on the side of notifying.
  }

  pendingOpen = { kind: 'mail', payload: mail }
  armed = true

  const showPreview = options.showPreview !== false
  const withSound = options.sound !== false

  try {
    const { sendNotification } = await import('@tauri-apps/plugin-notification')
    const sender = (mail.from_name || mail.name || mail.from || mail.address || '')
      .toString().trim() || 'New message'
    const subject = (mail.subject || '').toString().trim() || '(no subject)'
    const notification = showPreview
      ? { title: sender, body: subject }
      : { title: 'Guvercin', body: 'You have new mail' }
    if (withSound) notification.sound = 'default'
    if (mail.id != null) notification.id = notificationIdForMail(mail.id)
    sendNotification(notification)
  } catch (error) {
    console.error('Failed to send notification:', error)
  }
}

// Removes the OS notification previously posted for a mail, if any. Called when
// the user opens/reads a mail so its notification no longer lingers in the
// notification center. No-op when we never notified for this mail.
export async function dismissNotificationForMail(mailId) {
  if (!isTauri() || mailId == null) return
  const key = String(mailId)
  const id = mailIdToNotificationId.get(key)
  if (id == null) return
  mailIdToNotificationId.delete(key)
  // If the mail being opened is the one awaiting a click, drop the pending state.
  if (pendingOpen?.kind === 'mail' && String(pendingOpen.payload?.id) === key) {
    pendingOpen = null
    armed = false
  }
  try {
    const { removeActive } = await import('@tauri-apps/plugin-notification')
    await removeActive([{ id }])
  } catch (error) {
    console.error('Failed to remove notification:', error)
  }
}

// Posts a native notification for a time-based reminder (a calendar event or a
// due task). Unlike mail, reminders fire even when the window is focused — the
// user asked to be reminded at this moment. Clicking (i.e. re-focusing the app
// while a reminder is pending) routes to the right section via the same
// subscriber channel as mail.
//
// reminder: { kind: 'calendar' | 'todo', id, title, body, sound?: boolean }
export async function notifyReminder(reminder = {}) {
  if (!isTauri() || !permissionGranted) return
  const { kind, id, title, body, sound = true } = reminder
  if (!kind) return

  pendingOpen = { kind, payload: { id } }
  armed = true

  try {
    const { sendNotification } = await import('@tauri-apps/plugin-notification')
    const notification = {
      title: (title || 'Reminder').toString(),
      body: (body || '').toString(),
    }
    if (sound) notification.sound = 'default'
    sendNotification(notification)
  } catch (error) {
    console.error('Failed to send reminder notification:', error)
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

// Subscribe to "user clicked a notification" events. The callback receives
// { kind, payload }: kind 'mail' (payload = mail), 'calendar' or 'todo'
// (payload = { id }). Returns an unsubscribe fn.
export function subscribeNotificationOpen(callback) {
  if (typeof callback !== 'function') return () => {}
  openSubscribers.add(callback)
  return () => {
    openSubscribers.delete(callback)
  }
}
