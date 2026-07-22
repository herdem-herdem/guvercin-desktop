// Thin client for the contacts backend (see rust-backend/src/contacts_routes.rs).
//
// A "card" is the canonical contact document shared verbatim with the backend:
//   { uid, name:{prefix,first,middle,last,suffix,nickname,fileAs}, displayName,
//     emails:[{label,value}], phones:[{label,value}],
//     addresses:[{label,poBox,street,city,state,postalCode,country}],
//     websites:[{label,value}], im:[{label,value}],
//     organization:{company,department,jobTitle,office,profession,managerName,assistantName},
//     personal:{birthday,anniversary,spouse,notes},
//     categories:[], isFavorite:bool, photo:"data:..." }

import { apiUrl } from './api.js'

async function jsonOrThrow(res) {
  if (!res.ok) {
    let message = `Request failed (${res.status})`
    try {
      const body = await res.json()
      // The backend error shape is { status, message }; tolerate { error } too.
      if (body && (body.message || body.error)) message = body.message || body.error
    } catch {
      // non-JSON error body — keep the generic message
    }
    throw new Error(message)
  }
  return res.json()
}

export function emptyCard() {
  return {
    uid: '',
    name: { prefix: '', first: '', middle: '', last: '', suffix: '', nickname: '', fileAs: '' },
    displayName: '',
    emails: [],
    phones: [],
    addresses: [],
    websites: [],
    im: [],
    organization: {
      company: '', department: '', jobTitle: '', office: '',
      profession: '', managerName: '', assistantName: '',
    },
    personal: { birthday: '', spouse: '', notes: '' },
    categories: [],
    isFavorite: false,
    photo: '',
  }
}

// Fill in any keys a partial/legacy card may be missing so the form never sees
// `undefined`.
export function normalizeCard(card) {
  const base = emptyCard()
  if (!card || typeof card !== 'object') return base
  return {
    ...base,
    ...card,
    name: { ...base.name, ...(card.name || {}) },
    organization: { ...base.organization, ...(card.organization || {}) },
    personal: { ...base.personal, ...(card.personal || {}) },
    emails: Array.isArray(card.emails) ? card.emails : [],
    phones: Array.isArray(card.phones) ? card.phones : [],
    addresses: Array.isArray(card.addresses) ? card.addresses : [],
    websites: Array.isArray(card.websites) ? card.websites : [],
    im: Array.isArray(card.im) ? card.im : [],
    categories: Array.isArray(card.categories) ? card.categories : [],
    isFavorite: !!card.isFavorite,
  }
}

export function displayNameOf(card) {
  if (!card) return ''
  const dn = (card.displayName || '').trim()
  if (dn) return dn
  const n = card.name || {}
  const full = [n.first, n.middle, n.last].map((s) => (s || '').trim()).filter(Boolean).join(' ')
  if (full) return full
  if ((n.fileAs || '').trim()) return n.fileAs.trim()
  if ((card.organization?.company || '').trim()) return card.organization.company.trim()
  const email = (card.emails || []).find((e) => (e.value || '').trim())
  if (email) return email.value.trim()
  const phone = (card.phones || []).find((p) => (p.value || '').trim())
  if (phone) return phone.value.trim()
  return ''
}

export function primaryEmailOf(card) {
  const e = (card?.emails || []).find((x) => (x.value || '').trim())
  return e ? e.value.trim() : ''
}

export function primaryPhoneOf(card) {
  const p = (card?.phones || []).find((x) => (x.value || '').trim())
  return p ? p.value.trim() : ''
}

export async function fetchContacts(accountId, { search = '', favorites = false, list = null } = {}) {
  const params = new URLSearchParams()
  if (search) params.set('search', search)
  if (favorites) params.set('favorites', 'true')
  if (list != null) params.set('list', String(list))
  const qs = params.toString()
  const res = await fetch(apiUrl(`/api/contacts/${accountId}${qs ? `?${qs}` : ''}`))
  return jsonOrThrow(res)
}

// ── Lists ──

export async function fetchLists(accountId) {
  const res = await fetch(apiUrl(`/api/contacts/${accountId}/lists`))
  return jsonOrThrow(res)
}

export async function createList(accountId, name) {
  const res = await fetch(apiUrl(`/api/contacts/${accountId}/lists`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  })
  return jsonOrThrow(res)
}

export async function renameList(accountId, listId, name) {
  const res = await fetch(apiUrl(`/api/contacts/${accountId}/lists/${listId}`), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  })
  return jsonOrThrow(res)
}

export async function deleteList(accountId, listId) {
  const res = await fetch(apiUrl(`/api/contacts/${accountId}/lists/${listId}`), { method: 'DELETE' })
  return jsonOrThrow(res)
}

export async function createContact(accountId, card) {
  const res = await fetch(apiUrl(`/api/contacts/${accountId}`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(card),
  })
  return jsonOrThrow(res)
}

export async function updateContact(accountId, id, card) {
  const res = await fetch(apiUrl(`/api/contacts/${accountId}/${id}`), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(card),
  })
  return jsonOrThrow(res)
}

export async function deleteContact(accountId, id) {
  const res = await fetch(apiUrl(`/api/contacts/${accountId}/${id}`), { method: 'DELETE' })
  return jsonOrThrow(res)
}

export async function importVcf(accountId, vcfText, { merge = true } = {}) {
  const res = await fetch(apiUrl(`/api/contacts/${accountId}/import`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ vcf: vcfText, merge }),
  })
  return jsonOrThrow(res)
}

// Returns the raw vCard text for the given ids (or all contacts when ids is empty).
export async function exportVcf(accountId, ids = []) {
  const params = new URLSearchParams()
  if (ids.length) params.set('ids', ids.join(','))
  const qs = params.toString()
  const res = await fetch(apiUrl(`/api/contacts/${accountId}/export${qs ? `?${qs}` : ''}`))
  if (!res.ok) throw new Error(`Export failed (${res.status})`)
  return res.text()
}

export async function fetchSuggestions(accountId) {
  try {
    const res = await fetch(apiUrl(`/api/contacts/${accountId}/suggestions`))
    if (!res.ok) return []
    return res.json()
  } catch {
    return []
  }
}
