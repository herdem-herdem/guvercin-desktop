import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { requestNewCompose } from '../utils/mailtoInbox.js'
import { requestNewEvent, requestNewTask } from '../utils/crossLinks.js'
import {
  createContact, createList, deleteContact, deleteList, displayNameOf, emptyCard,
  exportVcf, fetchContacts, fetchLists, fetchSuggestions, googleStatus, googleSyncContacts,
  importVcf, normalizeCard, primaryEmailOf, primaryPhoneOf, renameList, updateContact,
} from '../utils/contactsApi.js'
import './ContactsSection.css'

const EMAIL_LABELS = ['work', 'home', 'other']
const PHONE_LABELS = ['mobile', 'work', 'home', 'main', 'workFax', 'homeFax', 'pager', 'other']
const ADDRESS_LABELS = ['work', 'home', 'other']
const WEBSITE_LABELS = ['work', 'home', 'other']

const AVATAR_COLORS = [
  '#246bce', '#2e7d32', '#b8860b', '#c2185b', '#6a1b9a',
  '#00838f', '#d84315', '#455a64', '#5d4037', '#7cb342',
]

function initialsOf(name) {
  const parts = (name || '').trim().split(/\s+/).filter(Boolean)
  if (parts.length === 0) return '?'
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase()
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase()
}

function colorOf(key) {
  const s = key || ''
  let hash = 0
  for (let i = 0; i < s.length; i++) hash = (hash * 31 + s.charCodeAt(i)) >>> 0
  return AVATAR_COLORS[hash % AVATAR_COLORS.length]
}

function ContactAvatar({ card, size = 40 }) {
  const name = displayNameOf(card)
  const style = {
    width: size, height: size, minWidth: size, borderRadius: '50%',
    fontSize: Math.round(size * 0.4),
  }
  if (card?.photo) {
    return <div className="cs-avatar" style={style}><img src={card.photo} alt={name} /></div>
  }
  return (
    <div className="cs-avatar cs-avatar--initials" style={{ ...style, background: colorOf(name || primaryEmailOf(card)) }}>
      {initialsOf(name)}
    </div>
  )
}

function downloadText(filename, text, mime = 'text/vcard;charset=utf-8') {
  const blob = new Blob([text], { type: mime })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

function slugifyFilename(name) {
  const base = (name || 'contact').trim().replace(/[^\w.-]+/g, '_').replace(/^_+|_+$/g, '')
  return `${base || 'contact'}.vcf`
}

const icon = (name) => <img src={`/img/icons/${name}.svg`} className="svg-icon-inline" alt="" />

export default function ContactsSection({ accountId, toolbarStyle = 'icon_text_small' }) {
  const { t } = useTranslation()
  const [contacts, setContacts] = useState([])
  const [lists, setLists] = useState([])
  const [counts, setCounts] = useState({ total: 0, favorites: 0 })
  const [loading, setLoading] = useState(false)
  const [search, setSearch] = useState('')
  const [showFavorites, setShowFavorites] = useState(false)
  const [activeList, setActiveList] = useState(null) // list_id or null
  const [selectedId, setSelectedId] = useState(null)
  const [mode, setMode] = useState('view') // 'view' | 'edit' | 'create'
  const [draft, setDraft] = useState(null)
  const [saving, setSaving] = useState(false)
  const [toasts, setToasts] = useState([])
  const [creatingList, setCreatingList] = useState(false)
  const [renamingId, setRenamingId] = useState(null)
  const [googleAvailable, setGoogleAvailable] = useState(false)
  const [syncingGoogle, setSyncingGoogle] = useState(false)
  const fileInputRef = useRef(null)
  const syncBusy = useRef(false)

  const pushToast = useCallback((text, type = 'info') => {
    const id = `${Date.now()}-${Math.random()}`
    setToasts((prev) => [...prev, { id, text, type }])
    setTimeout(() => setToasts((prev) => prev.filter((x) => x.id !== id)), type === 'error' ? 5000 : 3200)
  }, [])
  const dismissToast = useCallback((id) => setToasts((prev) => prev.filter((x) => x.id !== id)), [])

  const reloadLists = useCallback(async () => {
    if (!accountId) return
    try {
      const data = await fetchLists(accountId)
      setLists(data.lists || [])
      setCounts({ total: data.total || 0, favorites: data.favorites || 0 })
    } catch {
      // non-fatal
    }
  }, [accountId])

  const reload = useCallback(async () => {
    if (!accountId) return
    setLoading(true)
    try {
      const rows = await fetchContacts(accountId, { search, favorites: showFavorites, list: activeList })
      setContacts(rows.map((r) => ({ ...r, card: normalizeCard(r.card) })))
    } catch (e) {
      pushToast(e.message || 'Failed to load contacts', 'error')
    } finally {
      setLoading(false)
    }
  }, [accountId, search, showFavorites, activeList, pushToast])

  useEffect(() => {
    const id = setTimeout(reload, search ? 220 : 0)
    return () => clearTimeout(id)
  }, [reload, search])

  useEffect(() => { reloadLists() }, [reloadLists])

  useEffect(() => {
    if (!accountId) { setGoogleAvailable(false); return }
    let alive = true
    googleStatus(accountId).then((s) => { if (alive) setGoogleAvailable(!!s.available) }).catch(() => {})
    return () => { alive = false }
  }, [accountId])

  const refreshAll = useCallback(async () => {
    await Promise.all([reload(), reloadLists()])
  }, [reload, reloadLists])

  const selected = useMemo(
    () => contacts.find((c) => c.id === selectedId) || null,
    [contacts, selectedId],
  )

  const grouped = useMemo(() => {
    const groups = new Map()
    for (const c of contacts) {
      const name = displayNameOf(c.card)
      const letter = (name[0] || '#').toUpperCase()
      const key = /[A-Z]/.test(letter) ? letter : '#'
      if (!groups.has(key)) groups.set(key, [])
      groups.get(key).push(c)
    }
    return Array.from(groups.entries()).sort(([a], [b]) => {
      if (a === '#') return 1
      if (b === '#') return -1
      return a.localeCompare(b)
    })
  }, [contacts])

  const startCreate = useCallback((prefill) => {
    const card = prefill ? normalizeCard(prefill) : emptyCard()
    // New contacts inherit the currently-viewed list.
    if (activeList != null) {
      const l = lists.find((x) => x.list_id === activeList)
      if (l && !card.categories.includes(l.name)) card.categories = [...card.categories, l.name]
    }
    setDraft(card)
    setMode('create')
  }, [activeList, lists])

  const startEdit = useCallback(() => {
    if (!selected) return
    setDraft(normalizeCard(selected.card))
    setMode('edit')
  }, [selected])

  const cancelEdit = useCallback(() => { setDraft(null); setMode('view') }, [])

  const handleSave = useCallback(async () => {
    if (!draft) return
    setSaving(true)
    try {
      let rec
      if (mode === 'create') {
        rec = await createContact(accountId, draft)
        pushToast(t('Contact added'))
      } else {
        rec = await updateContact(accountId, selected.id, draft)
        pushToast(t('Contact saved'))
      }
      await refreshAll()
      setSelectedId(rec.id)
      setMode('view')
      setDraft(null)
    } catch (e) {
      pushToast(e.message || 'Save failed', 'error')
    } finally {
      setSaving(false)
    }
  }, [accountId, draft, mode, selected, refreshAll, pushToast, t])

  const handleDelete = useCallback(async (rec) => {
    const target = rec || selected
    if (!target) return
    if (!window.confirm(t('Delete this contact?'))) return
    try {
      await deleteContact(accountId, target.id)
      if (selectedId === target.id) { setSelectedId(null); setMode('view') }
      await refreshAll()
      pushToast(t('Contact deleted'))
    } catch (e) {
      pushToast(e.message || 'Delete failed', 'error')
    }
  }, [accountId, selected, selectedId, refreshAll, pushToast, t])

  const handleToggleFavorite = useCallback(async (rec) => {
    try {
      await updateContact(accountId, rec.id, { ...normalizeCard(rec.card), isFavorite: !rec.card.isFavorite })
      await refreshAll()
    } catch (e) {
      pushToast(e.message || 'Update failed', 'error')
    }
  }, [accountId, refreshAll, pushToast])

  const handleImportFile = useCallback(async (event) => {
    const file = event.target.files?.[0]
    event.target.value = ''
    if (!file) return
    try {
      const text = await file.text()
      if (!/BEGIN:VCARD/i.test(text)) {
        pushToast(t('This file is not a vCard. Choose a .vcf file exported from your phone or another contacts app.'), 'error')
        return
      }
      const result = await importVcf(accountId, text, { merge: true })
      await refreshAll()
      const parts = []
      if (result.imported) parts.push(t('{{n}} added', { n: result.imported }))
      if (result.updated) parts.push(t('{{n}} updated', { n: result.updated }))
      if (result.skipped) parts.push(t('{{n}} skipped', { n: result.skipped }))
      pushToast(parts.join(' · ') || t('Import complete'))
    } catch (e) {
      pushToast(e.message || 'Import failed', 'error')
    }
  }, [accountId, refreshAll, pushToast, t])

  const handleExportAll = useCallback(async () => {
    try {
      downloadText('contacts.vcf', await exportVcf(accountId, []))
    } catch (e) {
      pushToast(e.message || 'Export failed', 'error')
    }
  }, [accountId, pushToast])

  const runSyncRef = useRef(async () => {})
  runSyncRef.current = async ({ silent = false } = {}) => {
    if (!googleAvailable || syncBusy.current) return
    syncBusy.current = true
    setSyncingGoogle(true)
    try {
      const r = await googleSyncContacts(accountId)
      await refreshAll()
      if (!silent) pushToast(t('Synced with Google — {{in}} in, {{out}} out', { in: r.pulled || 0, out: r.pushed || 0 }))
    } catch (e) {
      if (!silent) pushToast(e.message || 'Google sync failed', 'error')
    } finally {
      syncBusy.current = false
      setSyncingGoogle(false)
    }
  }
  const handleGoogleSync = useCallback(() => runSyncRef.current({ silent: false }), [])

  // Auto ("paso") sync: once when Google becomes available, then on an interval.
  useEffect(() => {
    if (!googleAvailable) return undefined
    runSyncRef.current({ silent: true })
    const id = setInterval(() => runSyncRef.current({ silent: true }), 60000)
    return () => clearInterval(id)
  }, [googleAvailable])

  const handleExportOne = useCallback(async (rec) => {
    try {
      downloadText(slugifyFilename(displayNameOf(rec.card)), await exportVcf(accountId, [rec.id]))
    } catch (e) {
      pushToast(e.message || 'Export failed', 'error')
    }
  }, [accountId, pushToast])

  const emailContact = useCallback((rec) => {
    const email = primaryEmailOf(rec.card)
    if (email) requestNewCompose({ to: email })
  }, [])

  // Cross-feature: hand a contact off to Calendar or Todo.
  const newEventForContact = useCallback((rec) => {
    const email = primaryEmailOf(rec.card)
    const name = displayNameOf(rec.card)
    requestNewEvent({
      title: name ? t('Meeting with {{name}}', { name }) : '',
      attendees: email ? [{ name, email, status: '' }] : [],
    })
  }, [t])
  const newTaskForContact = useCallback((rec) => {
    const name = displayNameOf(rec.card)
    const email = primaryEmailOf(rec.card)
    requestNewTask({
      title: name ? t('Follow up with {{name}}', { name }) : '',
      notes: email,
    })
  }, [t])

  // ── List management ──
  const submitCreateList = useCallback(async (name) => {
    const trimmed = (name || '').trim()
    setCreatingList(false)
    if (!trimmed) return
    try {
      const l = await createList(accountId, trimmed)
      await reloadLists()
      setShowFavorites(false)
      setActiveList(l.list_id)
    } catch (e) {
      pushToast(e.message || 'Could not create list', 'error')
    }
  }, [accountId, reloadLists, pushToast])

  const submitRenameList = useCallback(async (listId, name) => {
    const trimmed = (name || '').trim()
    setRenamingId(null)
    if (!trimmed) return
    try {
      await renameList(accountId, listId, trimmed)
      await refreshAll()
    } catch (e) {
      pushToast(e.message || 'Could not rename list', 'error')
    }
  }, [accountId, refreshAll, pushToast])

  const handleDeleteList = useCallback(async (list) => {
    if (!window.confirm(t('Delete the list “{{name}}”? Contacts stay, only the list is removed.', { name: list.name }))) return
    try {
      await deleteList(accountId, list.list_id)
      if (activeList === list.list_id) setActiveList(null)
      await refreshAll()
      pushToast(t('List deleted'))
    } catch (e) {
      pushToast(e.message || 'Could not delete list', 'error')
    }
  }, [accountId, activeList, refreshAll, pushToast, t])

  const selectAll = () => { setShowFavorites(false); setActiveList(null) }
  const selectFavorites = () => { setShowFavorites(true); setActiveList(null) }
  const selectList = (id) => { setShowFavorites(false); setActiveList(id) }

  if (!accountId) {
    return <div className="cs-root cs-empty-account"><p>{t('Select an account to manage contacts.')}</p></div>
  }

  const tb = toolbarStyle || 'icon_text_small'
  const btn = (ic, label) => {
    const showIcon = tb !== 'text_small'
    const showLabel = tb !== 'icon_small' && tb !== 'icon_large'
    return (
      <>
        {showIcon && <span className="db-submenu-main-btn__icon">{ic}</span>}
        {showLabel && <span className="db-submenu-main-btn__text">{label}</span>}
      </>
    )
  }
  const hasEmail = selected && !!primaryEmailOf(selected.card)

  return (
    <div className="cs-root">
      <input ref={fileInputRef} type="file" accept=".vcf,text/vcard,text/x-vcard" style={{ display: 'none' }} onChange={handleImportFile} />

      {/* ── Toolbar (matches the mail ribbon) ── */}
      <div className={`db-submenu db-submenu--${tb} cs-toolbar-bar`}>
        <ul>
          <li><button className="db-submenu-main-btn" onClick={() => startCreate()}>{btn(icon('plus'), t('New contact'))}</button></li>
          <li><button className="db-submenu-main-btn" onClick={() => { setActiveList(null); setShowFavorites(false); setCreatingList(true) }}>{btn(icon('label'), t('New list'))}</button></li>
          <li className="cs-toolbar-sep" aria-hidden="true" />
          <li><button className="db-submenu-main-btn" disabled={!selected} onClick={startEdit}>{btn(icon('new-mail'), t('Edit'))}</button></li>
          <li><button className="db-submenu-main-btn" disabled={!hasEmail} onClick={() => selected && emailContact(selected)}>{btn(icon('mail'), t('Send email'))}</button></li>
          <li><button className="db-submenu-main-btn" disabled={!selected} onClick={() => selected && handleDelete(selected)}>{btn(icon('trash-bin'), t('Delete'))}</button></li>
          <li className="cs-toolbar-sep" aria-hidden="true" />
          <li><button className="db-submenu-main-btn" onClick={() => fileInputRef.current?.click()}>{btn(icon('folder'), t('Import'))}</button></li>
          <li><button className="db-submenu-main-btn" onClick={handleExportAll}>{btn(icon('save'), t('Export'))}</button></li>
          {googleAvailable && <li className="cs-toolbar-sep" aria-hidden="true" />}
          {googleAvailable && (
            <li><button className="db-submenu-main-btn" onClick={handleGoogleSync} disabled={syncingGoogle}>{btn(icon('online'), syncingGoogle ? t('Syncing…') : t('Sync with Google'))}</button></li>
          )}
        </ul>
      </div>

      <div className="cs-body">
        {/* ── Lists navigation ── */}
        <nav className="cs-lists-pane">
          <div className="cs-lists-head">
            <span>{t('Lists')}</span>
            <button className="cs-icon-plus" title={t('New list')} onClick={() => setCreatingList(true)}>＋</button>
          </div>

          <button className={`cs-nav-item ${!showFavorites && activeList == null ? 'active' : ''}`} onClick={selectAll}>
            <span className="cs-nav-ic">{icon('all-mails')}</span>
            <span className="cs-nav-label">{t('All contacts')}</span>
            <span className="cs-nav-count">{counts.total}</span>
          </button>
          <button className={`cs-nav-item ${showFavorites ? 'active' : ''}`} onClick={selectFavorites}>
            <span className="cs-nav-ic cs-nav-star">★</span>
            <span className="cs-nav-label">{t('Favorites')}</span>
            <span className="cs-nav-count">{counts.favorites}</span>
          </button>

          <div className="cs-nav-divider" />

          {creatingList && (
            <ListNameInput
              placeholder={t('List name')}
              onSubmit={submitCreateList}
              onCancel={() => setCreatingList(false)}
            />
          )}

          {lists.length === 0 && !creatingList && (
            <div className="cs-lists-empty">{t('No lists yet.')}</div>
          )}

          {lists.map((l) => (
            renamingId === l.list_id ? (
              <ListNameInput
                key={l.list_id}
                initial={l.name}
                onSubmit={(name) => submitRenameList(l.list_id, name)}
                onCancel={() => setRenamingId(null)}
              />
            ) : (
              <div key={l.list_id} className={`cs-nav-item cs-list-row ${activeList === l.list_id ? 'active' : ''}`}>
                <button className="cs-list-row-main" onClick={() => selectList(l.list_id)}>
                  <span className="cs-nav-ic">{icon('label')}</span>
                  <span className="cs-nav-label">{l.name}</span>
                  <span className="cs-nav-count">{l.count}</span>
                </button>
                <span className="cs-list-row-actions">
                  <button title={t('Rename')} onClick={() => setRenamingId(l.list_id)}>✎</button>
                  <button title={t('Delete list')} onClick={() => handleDeleteList(l)}>✕</button>
                </span>
              </div>
            )
          ))}
        </nav>

        {/* ── Contact list ── */}
        <aside className="cs-list-pane">
          <div className="cs-search-row">
            <input className="cs-search" type="search" placeholder={t('Search contacts')} value={search} onChange={(e) => setSearch(e.target.value)} />
          </div>
          <div className="cs-count">{loading ? t('Loading…') : t('{{n}} contacts', { n: contacts.length })}</div>

          <div className="cs-list">
            {!loading && contacts.length === 0 && (
              <div className="cs-list-empty">
                {search || showFavorites || activeList != null
                  ? t('No matching contacts.')
                  : t('No contacts yet. Add one or import a .vcf file.')}
              </div>
            )}
            {grouped.map(([letter, items]) => (
              <div key={letter} className="cs-group">
                <div className="cs-group-head">{letter}</div>
                {items.map((rec) => (
                  <button key={rec.id} className={`cs-list-item ${selectedId === rec.id ? 'active' : ''}`} onClick={() => { setSelectedId(rec.id); setMode('view') }}>
                    <ContactAvatar card={rec.card} size={38} />
                    <div className="cs-list-item-text">
                      <div className="cs-list-item-name">{displayNameOf(rec.card) || t('(no name)')}</div>
                      <div className="cs-list-item-sub">{primaryEmailOf(rec.card) || rec.card.organization?.company || primaryPhoneOf(rec.card) || ''}</div>
                    </div>
                    <span className={`cs-star ${rec.card.isFavorite ? 'on' : ''}`} role="button" title={t('Toggle favorite')} onClick={(e) => { e.stopPropagation(); handleToggleFavorite(rec) }}>
                      {rec.card.isFavorite ? '★' : '☆'}
                    </span>
                  </button>
                ))}
              </div>
            ))}
          </div>
        </aside>

        {/* ── Detail / editor ── */}
        <section className="cs-detail-pane">
          {mode === 'view' && !selected && (
            <ContactsWelcome onNew={() => startCreate()} t={t} accountId={accountId} onPickSuggestion={startCreate} />
          )}
          {mode === 'view' && selected && (
            <ContactView
              rec={selected} t={t}
              onEdit={startEdit} onDelete={() => handleDelete(selected)}
              onEmail={() => emailContact(selected)} onExport={() => handleExportOne(selected)}
              onToggleFavorite={() => handleToggleFavorite(selected)}
              onNewEvent={() => newEventForContact(selected)} onNewTask={() => newTaskForContact(selected)}
              onOpenList={(name) => { const l = lists.find((x) => x.name === name); if (l) selectList(l.list_id) }}
            />
          )}
          {(mode === 'edit' || mode === 'create') && draft && (
            <ContactEditor t={t} draft={draft} setDraft={setDraft} saving={saving} isNew={mode === 'create'} lists={lists} onCancel={cancelEdit} onSave={handleSave} />
          )}
        </section>
      </div>

      <div className="cs-toasts">
        {toasts.map((toast) => (
          <div key={toast.id} className={`cs-toast cs-toast--${toast.type}`}>
            <span className="cs-toast-text">{toast.text}</span>
            <button className="cs-toast-close" onClick={() => dismissToast(toast.id)} title={t('Dismiss')}>✕</button>
          </div>
        ))}
      </div>
    </div>
  )
}

// ─────────────────────────── List name inline input ───────────────────────────

function ListNameInput({ initial = '', placeholder, onSubmit, onCancel }) {
  const [value, setValue] = useState(initial)
  const ref = useRef(null)
  useEffect(() => { ref.current?.focus(); ref.current?.select() }, [])
  return (
    <input
      ref={ref}
      className="cs-list-name-input"
      value={value}
      placeholder={placeholder}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === 'Enter') onSubmit(value)
        else if (e.key === 'Escape') onCancel()
      }}
      onBlur={() => onSubmit(value)}
    />
  )
}

// ─────────────────────────── Welcome / suggestions ───────────────────────────

function ContactsWelcome({ onNew, onPickSuggestion, accountId, t }) {
  const [suggestions, setSuggestions] = useState([])
  useEffect(() => {
    let alive = true
    fetchSuggestions(accountId).then((s) => { if (alive) setSuggestions(s) })
    return () => { alive = false }
  }, [accountId])

  return (
    <div className="cs-welcome">
      <div className="cs-welcome-icon">👤</div>
      <h2>{t('Contacts')}</h2>
      <p>{t('Select a contact to view details, or add a new one.')}</p>
      <button className="cs-btn cs-btn--primary" onClick={onNew}>{t('New contact')}</button>

      {suggestions.length > 0 && (
        <div className="cs-suggest">
          <div className="cs-suggest-head">{t('People you email often')}</div>
          <p className="cs-suggest-hint">{t('Not in your contacts yet — click to add.')}</p>
          <div className="cs-suggest-list">
            {suggestions.slice(0, 8).map((s) => (
              <button key={s.email} className="cs-suggest-item" onClick={() => {
                const card = emptyCard()
                card.displayName = s.name || ''
                if (s.name) {
                  const parts = s.name.trim().split(/\s+/)
                  card.name.first = parts[0] || ''
                  card.name.last = parts.slice(1).join(' ')
                }
                card.emails = [{ label: 'work', value: s.email }]
                onPickSuggestion(card)
              }}>
                <ContactAvatar card={{ displayName: s.name, emails: [{ value: s.email }] }} size={32} />
                <div className="cs-suggest-item-text">
                  <div className="cs-suggest-item-name">{s.name || s.email}</div>
                  <div className="cs-suggest-item-sub">{s.email} · {t('{{n}} emails', { n: s.count })}</div>
                </div>
                <span className="cs-suggest-add">＋</span>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}

// ─────────────────────────── Read-only view ───────────────────────────

function labelText(t, label) {
  const map = {
    work: t('Work'), home: t('Home'), other: t('Other'), mobile: t('Mobile'),
    main: t('Main'), workFax: t('Work fax'), homeFax: t('Home fax'), pager: t('Pager'), im: t('IM'),
  }
  return map[label] || label
}

function Field({ label, children }) {
  return (
    <div className="cs-field">
      <div className="cs-field-label">{label}</div>
      <div className="cs-field-value">{children}</div>
    </div>
  )
}

function ContactView({ rec, t, onEdit, onDelete, onEmail, onExport, onToggleFavorite, onNewEvent, onNewTask, onOpenList }) {
  const card = rec.card
  const org = card.organization || {}
  const person = card.personal || {}
  const subtitle = [org.jobTitle, org.company].filter(Boolean).join(' · ')
  const hasEmail = !!primaryEmailOf(card)

  const addressText = (a) => [
    a.street, [a.postalCode, a.city].filter(Boolean).join(' '),
    [a.state, a.country].filter(Boolean).join(', '),
  ].filter((s) => (s || '').trim()).join('\n')

  return (
    <div className="cs-view">
      <div className="cs-view-header">
        <ContactAvatar card={card} size={72} />
        <div className="cs-view-heading">
          <div className="cs-view-name">
            {displayNameOf(card) || t('(no name)')}
            <button className={`cs-star ${card.isFavorite ? 'on' : ''}`} onClick={onToggleFavorite} title={t('Toggle favorite')}>
              {card.isFavorite ? '★' : '☆'}
            </button>
          </div>
          {subtitle && <div className="cs-view-sub">{subtitle}</div>}
          {card.name?.nickname && <div className="cs-view-nick">“{card.name.nickname}”</div>}
        </div>
      </div>

      <div className="cs-view-actions">
        <button className="cs-btn cs-btn--primary" disabled={!hasEmail} onClick={onEmail}>{t('Send email')}</button>
        <button className="cs-btn" onClick={onNewEvent}>{t('New event')}</button>
        <button className="cs-btn" onClick={onNewTask}>{t('New task')}</button>
        <button className="cs-btn" onClick={onEdit}>{t('Edit')}</button>
        <button className="cs-btn" onClick={onExport}>{t('Export')}</button>
        <button className="cs-btn cs-btn--danger" onClick={onDelete}>{t('Delete')}</button>
      </div>

      <div className="cs-view-body">
        {card.emails?.some((e) => e.value) && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Email')}</div>
            {card.emails.filter((e) => e.value).map((e, i) => (
              <Field key={i} label={labelText(t, e.label)}>
                <a href={`mailto:${e.value}`} onClick={(ev) => { ev.preventDefault(); requestNewCompose({ to: e.value }) }}>{e.value}</a>
              </Field>
            ))}
          </div>
        )}

        {card.phones?.some((p) => p.value) && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Phone')}</div>
            {card.phones.filter((p) => p.value).map((p, i) => (
              <Field key={i} label={labelText(t, p.label)}><a href={`tel:${p.value}`}>{p.value}</a></Field>
            ))}
          </div>
        )}

        {card.addresses?.some((a) => addressText(a)) && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Address')}</div>
            {card.addresses.filter((a) => addressText(a)).map((a, i) => (
              <Field key={i} label={labelText(t, a.label)}><span className="cs-multiline">{addressText(a)}</span></Field>
            ))}
          </div>
        )}

        {card.websites?.some((w) => w.value) && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Website')}</div>
            {card.websites.filter((w) => w.value).map((w, i) => (
              <Field key={i} label={labelText(t, w.label)}><span className="cs-link-like">{w.value}</span></Field>
            ))}
          </div>
        )}

        {card.im?.some((m) => m.value) && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Instant messaging')}</div>
            {card.im.filter((m) => m.value).map((m, i) => (<Field key={i} label={labelText(t, m.label)}>{m.value}</Field>))}
          </div>
        )}

        {(org.department || org.office || org.profession || org.managerName || org.assistantName) && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Work')}</div>
            {org.department && <Field label={t('Department')}>{org.department}</Field>}
            {org.office && <Field label={t('Office')}>{org.office}</Field>}
            {org.profession && <Field label={t('Profession')}>{org.profession}</Field>}
            {org.managerName && <Field label={t('Manager')}>{org.managerName}</Field>}
            {org.assistantName && <Field label={t('Assistant')}>{org.assistantName}</Field>}
          </div>
        )}

        {(person.birthday || person.spouse) && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Personal')}</div>
            {person.birthday && <Field label={t('Birthday')}>{person.birthday}</Field>}
            {person.spouse && <Field label={t('Spouse / Partner')}>{person.spouse}</Field>}
          </div>
        )}

        {card.categories?.length > 0 && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Lists')}</div>
            <div className="cs-chips">
              {card.categories.map((c, i) => (
                <button key={i} className="cs-chip cs-chip--link" onClick={() => onOpenList(c)}>{c}</button>
              ))}
            </div>
          </div>
        )}

        {person.notes && (
          <div className="cs-view-section">
            <div className="cs-view-section-title">{t('Notes')}</div>
            <div className="cs-notes cs-multiline">{person.notes}</div>
          </div>
        )}
      </div>
    </div>
  )
}

// ─────────────────────────── Editor ───────────────────────────

function ContactEditor({ t, draft, setDraft, onCancel, onSave, saving, isNew, lists }) {
  const [tab, setTab] = useState('general')
  const photoInputRef = useRef(null)

  const set = (path, value) => {
    setDraft((prev) => {
      const next = structuredClone(prev)
      let obj = next
      const keys = path.split('.')
      for (let i = 0; i < keys.length - 1; i++) obj = obj[keys[i]]
      obj[keys[keys.length - 1]] = value
      if (path.startsWith('name.') && path !== 'name.nickname') next.displayName = ''
      return next
    })
  }
  const setList = (key, idx, field, value) => {
    setDraft((prev) => { const next = structuredClone(prev); next[key][idx][field] = value; return next })
  }
  const addItem = (key, item) => setDraft((prev) => ({ ...structuredClone(prev), [key]: [...prev[key], item] }))
  const removeItem = (key, idx) => setDraft((prev) => { const next = structuredClone(prev); next[key].splice(idx, 1); return next })

  const toggleListMembership = (name) => setDraft((prev) => {
    const next = structuredClone(prev)
    const has = next.categories.some((c) => c.toLowerCase() === name.toLowerCase())
    next.categories = has ? next.categories.filter((c) => c.toLowerCase() !== name.toLowerCase()) : [...next.categories, name]
    return next
  })
  const addNewListName = (name) => setDraft((prev) => {
    const trimmed = (name || '').trim()
    if (!trimmed) return prev
    const next = structuredClone(prev)
    if (!next.categories.some((c) => c.toLowerCase() === trimmed.toLowerCase())) next.categories = [...next.categories, trimmed]
    return next
  })

  const onPhoto = (e) => {
    const file = e.target.files?.[0]
    e.target.value = ''
    if (!file) return
    const reader = new FileReader()
    reader.onload = () => set('photo', reader.result)
    reader.readAsDataURL(file)
  }

  // Lists the contact belongs to but which aren't in the account's list set yet
  // (e.g. freshly typed) so they still show as checked.
  const knownNames = new Set(lists.map((l) => l.name.toLowerCase()))
  const extraNames = draft.categories.filter((c) => !knownNames.has(c.toLowerCase()))

  return (
    <div className="cs-editor">
      <div className="cs-editor-topbar">
        <div className="cs-editor-title">{isNew ? t('New contact') : t('Edit contact')}</div>
        <div className="cs-editor-actions">
          <button className="cs-btn" onClick={onCancel} disabled={saving}>{t('Cancel')}</button>
          <button className="cs-btn cs-btn--primary" onClick={onSave} disabled={saving}>{saving ? t('Saving…') : t('Save')}</button>
        </div>
      </div>

      <div className="cs-editor-photo-row">
        <ContactAvatar card={draft} size={64} />
        <div className="cs-editor-photo-actions">
          <input ref={photoInputRef} type="file" accept="image/*" style={{ display: 'none' }} onChange={onPhoto} />
          <button className="cs-btn cs-btn--sm" onClick={() => photoInputRef.current?.click()}>{t('Change photo')}</button>
          {draft.photo && <button className="cs-btn cs-btn--sm" onClick={() => set('photo', '')}>{t('Remove')}</button>}
          <label className="cs-fav-check">
            <input type="checkbox" checked={draft.isFavorite} onChange={(e) => set('isFavorite', e.target.checked)} />
            {t('Favorite')}
          </label>
        </div>
      </div>

      <div className="cs-tabs">
        <button className={`cs-tab ${tab === 'general' ? 'active' : ''}`} onClick={() => setTab('general')}>{t('General')}</button>
        <button className={`cs-tab ${tab === 'details' ? 'active' : ''}`} onClick={() => setTab('details')}>{t('Details')}</button>
      </div>

      <div className="cs-editor-body">
        {tab === 'general' && (
          <>
            <div className="cs-form-section">
              <div className="cs-form-section-title">{t('Name')}</div>
              <div className="cs-name-grid">
                <input className="cs-input" placeholder={t('First')} value={draft.name.first} onChange={(e) => set('name.first', e.target.value)} />
                <input className="cs-input" placeholder={t('Middle')} value={draft.name.middle} onChange={(e) => set('name.middle', e.target.value)} />
                <input className="cs-input" placeholder={t('Last')} value={draft.name.last} onChange={(e) => set('name.last', e.target.value)} />
              </div>
              <div className="cs-form-row2">
                <label className="cs-labeled">
                  <span>{t('Nickname')}</span>
                  <input className="cs-input" value={draft.name.nickname} onChange={(e) => set('name.nickname', e.target.value)} />
                </label>
              </div>
            </div>

            <div className="cs-form-section">
              <div className="cs-form-section-title">{t('Organization')}</div>
              <div className="cs-form-row2">
                <label className="cs-labeled"><span>{t('Company')}</span>
                  <input className="cs-input" value={draft.organization.company} onChange={(e) => set('organization.company', e.target.value)} /></label>
                <label className="cs-labeled"><span>{t('Job title')}</span>
                  <input className="cs-input" value={draft.organization.jobTitle} onChange={(e) => set('organization.jobTitle', e.target.value)} /></label>
              </div>
            </div>

            <MultiField title={t('Email')} keyName="emails" list={draft.emails} labels={EMAIL_LABELS}
              labelText={(l) => labelText(t, l)} placeholder={t('name@example.com')} type="email"
              addLabel={t('Add email')} onAdd={() => addItem('emails', { label: 'work', value: '' })} onChange={setList} onRemove={removeItem} />

            <MultiField title={t('Phone')} keyName="phones" list={draft.phones} labels={PHONE_LABELS}
              labelText={(l) => labelText(t, l)} placeholder={t('Phone number')} type="tel"
              addLabel={t('Add phone')} onAdd={() => addItem('phones', { label: 'mobile', value: '' })} onChange={setList} onRemove={removeItem} />

            <div className="cs-form-section">
              <div className="cs-form-section-title">{t('Lists')}</div>
              <div className="cs-list-picker">
                {lists.map((l) => {
                  const checked = draft.categories.some((c) => c.toLowerCase() === l.name.toLowerCase())
                  return (
                    <label key={l.list_id} className={`cs-list-chip ${checked ? 'on' : ''}`}>
                      <input type="checkbox" checked={checked} onChange={() => toggleListMembership(l.name)} />
                      {l.name}
                    </label>
                  )
                })}
                {extraNames.map((name) => (
                  <label key={name} className="cs-list-chip on">
                    <input type="checkbox" checked onChange={() => toggleListMembership(name)} />
                    {name}
                  </label>
                ))}
              </div>
              <NewListInline placeholder={t('Add to a new list…')} onAdd={addNewListName} addLabel={t('Add')} />
            </div>
          </>
        )}

        {tab === 'details' && (
          <>
            <AddressList t={t} list={draft.addresses} labels={ADDRESS_LABELS} labelText={(l) => labelText(t, l)}
              onAdd={() => addItem('addresses', { label: 'work', poBox: '', street: '', city: '', state: '', postalCode: '', country: '' })}
              onRemove={(idx) => removeItem('addresses', idx)} onChange={(idx, field, value) => setList('addresses', idx, field, value)} />

            <MultiField title={t('Website')} keyName="websites" list={draft.websites} labels={WEBSITE_LABELS}
              labelText={(l) => labelText(t, l)} placeholder={t('https://example.com')} type="url"
              addLabel={t('Add website')} onAdd={() => addItem('websites', { label: 'work', value: '' })} onChange={setList} onRemove={removeItem} />

            <MultiField title={t('Instant messaging')} keyName="im" list={draft.im} labels={['im', 'work', 'home', 'other']}
              labelText={(l) => labelText(t, l)} placeholder={t('IM address')} type="text"
              addLabel={t('Add IM')} onAdd={() => addItem('im', { label: 'im', value: '' })} onChange={setList} onRemove={removeItem} />

            <div className="cs-form-section">
              <div className="cs-form-section-title">{t('Work details')}</div>
              <div className="cs-form-row2">
                <label className="cs-labeled"><span>{t('Department')}</span>
                  <input className="cs-input" value={draft.organization.department} onChange={(e) => set('organization.department', e.target.value)} /></label>
                <label className="cs-labeled"><span>{t('Office')}</span>
                  <input className="cs-input" value={draft.organization.office} onChange={(e) => set('organization.office', e.target.value)} /></label>
                <label className="cs-labeled"><span>{t('Profession')}</span>
                  <input className="cs-input" value={draft.organization.profession} onChange={(e) => set('organization.profession', e.target.value)} /></label>
                <label className="cs-labeled"><span>{t('Manager')}</span>
                  <input className="cs-input" value={draft.organization.managerName} onChange={(e) => set('organization.managerName', e.target.value)} /></label>
                <label className="cs-labeled"><span>{t('Assistant')}</span>
                  <input className="cs-input" value={draft.organization.assistantName} onChange={(e) => set('organization.assistantName', e.target.value)} /></label>
              </div>
            </div>

            <div className="cs-form-section">
              <div className="cs-form-section-title">{t('Personal')}</div>
              <div className="cs-form-row2">
                <label className="cs-labeled"><span>{t('Birthday')}</span>
                  <input className="cs-input" type="date" value={draft.personal.birthday} onChange={(e) => set('personal.birthday', e.target.value)} /></label>
                <label className="cs-labeled"><span>{t('Spouse / Partner')}</span>
                  <input className="cs-input" value={draft.personal.spouse} onChange={(e) => set('personal.spouse', e.target.value)} /></label>
              </div>
            </div>

            <div className="cs-form-section">
              <div className="cs-form-section-title">{t('Notes')}</div>
              <textarea className="cs-textarea" rows={5} value={draft.personal.notes} onChange={(e) => set('personal.notes', e.target.value)} />
            </div>
          </>
        )}
      </div>
    </div>
  )
}

function NewListInline({ placeholder, onAdd, addLabel }) {
  const [value, setValue] = useState('')
  const submit = () => { onAdd(value); setValue('') }
  return (
    <div className="cs-newlist-row">
      <input className="cs-input" placeholder={placeholder} value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); submit() } }} />
      <button className="cs-add-btn" onClick={submit} disabled={!value.trim()}>{addLabel}</button>
    </div>
  )
}

function MultiField({ title, keyName, list, labels, labelText, placeholder, type, addLabel, onAdd, onChange, onRemove }) {
  return (
    <div className="cs-form-section">
      <div className="cs-form-section-title">{title}</div>
      {list.map((item, idx) => (
        <div key={idx} className="cs-multi-row">
          <select className="cs-select" value={item.label} onChange={(e) => onChange(keyName, idx, 'label', e.target.value)}>
            {labels.map((l) => <option key={l} value={l}>{labelText(l)}</option>)}
          </select>
          <input className="cs-input" type={type} placeholder={placeholder} value={item.value} onChange={(e) => onChange(keyName, idx, 'value', e.target.value)} />
          <button className="cs-icon-btn" title="Remove" onClick={() => onRemove(keyName, idx)}>✕</button>
        </div>
      ))}
      <button className="cs-add-btn" onClick={onAdd}>＋ {addLabel}</button>
    </div>
  )
}

function AddressList({ t, list, labels, labelText, onAdd, onRemove, onChange }) {
  return (
    <div className="cs-form-section">
      <div className="cs-form-section-title">{t('Address')}</div>
      {list.map((a, idx) => (
        <div key={idx} className="cs-address-block">
          <div className="cs-address-head">
            <select className="cs-select" value={a.label} onChange={(e) => onChange(idx, 'label', e.target.value)}>
              {labels.map((l) => <option key={l} value={l}>{labelText(l)}</option>)}
            </select>
            <button className="cs-icon-btn" title="Remove" onClick={() => onRemove(idx)}>✕</button>
          </div>
          <input className="cs-input" placeholder={t('Street address')} value={a.street} onChange={(e) => onChange(idx, 'street', e.target.value)} />
          <div className="cs-address-grid">
            <input className="cs-input" placeholder={t('City')} value={a.city} onChange={(e) => onChange(idx, 'city', e.target.value)} />
            <input className="cs-input" placeholder={t('State / Province')} value={a.state} onChange={(e) => onChange(idx, 'state', e.target.value)} />
            <input className="cs-input" placeholder={t('ZIP / Postal code')} value={a.postalCode} onChange={(e) => onChange(idx, 'postalCode', e.target.value)} />
            <input className="cs-input" placeholder={t('Country / Region')} value={a.country} onChange={(e) => onChange(idx, 'country', e.target.value)} />
          </div>
        </div>
      ))}
      <button className="cs-add-btn" onClick={onAdd}>＋ {t('Add address')}</button>
    </div>
  )
}
