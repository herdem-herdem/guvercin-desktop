// Cross-feature bridge so Mail, Contacts, Calendar and Todo can hand work to one
// another (e.g. "add this email to my calendar", "new task for this contact").
//
// Mirrors the mailtoInbox queue+subscriber pattern: a request made while the
// target workspace isn't mounted yet is buffered and drained once it subscribes.
// `requestNewEvent`/`requestNewTask` also fire a navigation intent so the shell
// (DashboardPage) can switch to the right section first.

function makeChannel() {
  const queue = []
  const subs = new Set()
  const dispatch = (payload) => {
    if (subs.size === 0) { queue.push(payload); return }
    for (const cb of subs) {
      try { cb(payload) } catch (e) { console.error('crossLinks subscriber failed:', e) }
    }
  }
  const subscribe = (cb) => {
    if (typeof cb !== 'function') return () => {}
    subs.add(cb)
    if (queue.length) {
      const pending = queue.splice(0, queue.length)
      for (const p of pending) {
        try { cb(p) } catch (e) { console.error('crossLinks subscriber failed:', e) }
      }
    }
    return () => subs.delete(cb)
  }
  return { dispatch, subscribe }
}

const nav = makeChannel()
const eventCh = makeChannel()
const taskCh = makeChannel()

// Ask the shell to switch to a workspace ('mail' | 'calendar' | 'contacts' | 'todo').
export function navigateTo(section) { nav.dispatch(section) }
export function subscribeNavigate(cb) { return nav.subscribe(cb) }

// Open a pre-filled new-event editor in the Calendar workspace.
export function requestNewEvent(prefill = {}) {
  nav.dispatch('calendar')
  eventCh.dispatch(prefill)
}
export function subscribeNewEvent(cb) { return eventCh.subscribe(cb) }

// Open a pre-filled new-task editor in the Todo workspace.
export function requestNewTask(prefill = {}) {
  nav.dispatch('todo')
  taskCh.dispatch(prefill)
}
export function subscribeNewTask(cb) { return taskCh.subscribe(cb) }
