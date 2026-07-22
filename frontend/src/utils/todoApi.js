// Thin client for the tasks backend (see rust-backend/src/todo_routes.rs).
//
// A "task card" is the canonical task document shared verbatim with the backend:
//   { uid, listId, title, notes, due:"YYYY-MM-DD"|"YYYY-MM-DDTHH:MM"|"",
//     hasDueTime, priority:'none'|'low'|'medium'|'high', completed, starred,
//     subtasks:[{title,done}] }
//
// Due dates use the same naive wall-clock convention as the calendar.

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

export function emptyTask() {
  return {
    uid: '',
    listId: null,
    title: '',
    notes: '',
    due: '',
    hasDueTime: false,
    priority: 'none',
    completed: false,
    starred: false,
    subtasks: [],
  }
}

export function normalizeTask(card) {
  const base = emptyTask()
  if (!card || typeof card !== 'object') return base
  return {
    ...base,
    ...card,
    subtasks: Array.isArray(card.subtasks) ? card.subtasks : [],
    priority: ['none', 'low', 'medium', 'high'].includes(card.priority) ? card.priority : 'none',
    completed: !!card.completed,
    starred: !!card.starred,
  }
}

// ── Tasks ──

export async function fetchTasks(accountId, { list = null, search = '', completed = null, starred = false } = {}) {
  const params = new URLSearchParams()
  if (list != null) params.set('list', String(list))
  if (search) params.set('search', search)
  if (completed != null) params.set('completed', completed ? 'true' : 'false')
  if (starred) params.set('starred', 'true')
  const qs = params.toString()
  const res = await fetch(apiUrl(`/api/tasks/${accountId}${qs ? `?${qs}` : ''}`))
  return jsonOrThrow(res)
}

export async function createTask(accountId, card) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(card),
  })
  return jsonOrThrow(res)
}

export async function updateTask(accountId, id, card) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}/${id}`), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(card),
  })
  return jsonOrThrow(res)
}

export async function deleteTask(accountId, id) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}/${id}`), { method: 'DELETE' })
  return jsonOrThrow(res)
}

export async function clearCompleted(accountId, list = null) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}/clear-completed`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ list }),
  })
  return jsonOrThrow(res)
}

// ── Lists ──

export async function fetchTaskLists(accountId) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}/lists`))
  return jsonOrThrow(res)
}

export async function createTaskList(accountId, { name, color }) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}/lists`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, color }),
  })
  return jsonOrThrow(res)
}

export async function updateTaskList(accountId, listId, patch) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}/lists/${listId}`), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(patch),
  })
  return jsonOrThrow(res)
}

export async function deleteTaskList(accountId, listId) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}/lists/${listId}`), { method: 'DELETE' })
  return jsonOrThrow(res)
}

// ── Google ──

export async function googleStatus(accountId) {
  try {
    const res = await fetch(apiUrl(`/api/google/${accountId}/status`))
    if (!res.ok) return { available: false }
    return res.json()
  } catch {
    return { available: false }
  }
}

export async function googleSyncTasks(accountId) {
  const res = await fetch(apiUrl(`/api/tasks/${accountId}/google-sync`), { method: 'POST' })
  return jsonOrThrow(res)
}
