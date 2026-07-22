// Thin client for the calendar backend (see rust-backend/src/calendar_routes.rs).
//
// An "event card" is the canonical event document shared verbatim with the backend:
//   { uid, calendarId, title, location, description, allDay,
//     start:"YYYY-MM-DDTHH:MM" | "YYYY-MM-DD", end, color, status, busy,
//     attendees:[{name,email,status}], reminders:[minutesBefore],
//     recurrence:{ freq, interval, until, count, byWeekday:[0..6] } }
//
// Times are naive wall-clock: the backend parses the ISO-local string as if it were
// UTC, and every window bound we send is computed the same way (see naiveMsLocal),
// so month/week/day math lines up regardless of the machine's timezone.

import { apiUrl } from './api.js'

async function jsonOrThrow(res) {
  if (!res.ok) {
    let message = `Request failed (${res.status})`
    try {
      const body = await res.json()
      if (body && (body.message || body.error)) message = body.message || body.error
    } catch {
      // non-JSON error body — keep the generic message
    }
    throw new Error(message)
  }
  return res.json()
}

// ── Time helpers (naive wall-clock) ──

const pad = (n) => String(n).padStart(2, '0')

// Naive epoch millis for a local wall-clock Date (matches the Rust side, which
// parses the ISO-local string as UTC).
export function naiveMsLocal(date) {
  return Date.UTC(
    date.getFullYear(), date.getMonth(), date.getDate(),
    date.getHours(), date.getMinutes(), date.getSeconds(),
  )
}

// Parse an ISO-local string ("YYYY-MM-DD" or "YYYY-MM-DDTHH:MM") into a local Date.
export function parseIsoLocal(str) {
  if (!str) return null
  const s = String(str).trim()
  const m = s.match(/^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2}))?/)
  if (!m) return null
  const [, y, mo, d, h, mi] = m
  return new Date(Number(y), Number(mo) - 1, Number(d), h ? Number(h) : 0, mi ? Number(mi) : 0, 0, 0)
}

export function formatIsoLocalDate(date) {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

export function formatIsoLocalDateTime(date) {
  return `${formatIsoLocalDate(date)}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

export function emptyEvent() {
  return {
    uid: '',
    calendarId: null,
    title: '',
    location: '',
    description: '',
    allDay: false,
    start: '',
    end: '',
    color: '',
    status: 'confirmed',
    busy: true,
    attendees: [],
    reminders: [],
    recurrence: { freq: 'none', interval: 1, until: '', count: 0, byWeekday: [] },
  }
}

export function normalizeEvent(card) {
  const base = emptyEvent()
  if (!card || typeof card !== 'object') return base
  return {
    ...base,
    ...card,
    recurrence: { ...base.recurrence, ...(card.recurrence || {}) },
    attendees: Array.isArray(card.attendees) ? card.attendees : [],
    reminders: Array.isArray(card.reminders) ? card.reminders : [],
    busy: card.busy !== false,
  }
}

// ── Events ──

export async function fetchEvents(accountId, { from, to, search = '', calendars = null } = {}) {
  const params = new URLSearchParams()
  if (from != null) params.set('from', String(from))
  if (to != null) params.set('to', String(to))
  if (search) params.set('search', search)
  if (Array.isArray(calendars) && calendars.length) params.set('calendars', calendars.join(','))
  const qs = params.toString()
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/events${qs ? `?${qs}` : ''}`))
  return jsonOrThrow(res)
}

export async function getEvent(accountId, id) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/events/${id}`))
  return jsonOrThrow(res)
}

export async function createEvent(accountId, card) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/events`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(card),
  })
  return jsonOrThrow(res)
}

export async function updateEvent(accountId, id, card) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/events/${id}`), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(card),
  })
  return jsonOrThrow(res)
}

export async function deleteEvent(accountId, id) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/events/${id}`), { method: 'DELETE' })
  return jsonOrThrow(res)
}

// ── Calendars ──

export async function fetchCalendars(accountId) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/calendars`))
  return jsonOrThrow(res)
}

export async function createCalendar(accountId, { name, color }) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/calendars`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, color }),
  })
  return jsonOrThrow(res)
}

export async function updateCalendar(accountId, calendarId, patch) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/calendars/${calendarId}`), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(patch),
  })
  return jsonOrThrow(res)
}

export async function deleteCalendar(accountId, calendarId) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/calendars/${calendarId}`), { method: 'DELETE' })
  return jsonOrThrow(res)
}

// ── Import / export ──

export async function importIcs(accountId, icsText, { calendarId = null } = {}) {
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/import`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ics: icsText, calendarId }),
  })
  return jsonOrThrow(res)
}

export async function exportIcs(accountId, { ids = [], calendars = [] } = {}) {
  const params = new URLSearchParams()
  if (ids.length) params.set('ids', ids.join(','))
  if (calendars.length) params.set('calendars', calendars.join(','))
  const qs = params.toString()
  const res = await fetch(apiUrl(`/api/calendar/${accountId}/export${qs ? `?${qs}` : ''}`))
  if (!res.ok) throw new Error(`Export failed (${res.status})`)
  return res.text()
}
