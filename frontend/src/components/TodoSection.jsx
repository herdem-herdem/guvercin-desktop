import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { formatIsoLocalDate, formatIsoLocalDateTime, parseIsoLocal } from '../utils/calendarApi.js'
import { requestNewEvent, subscribeNewTask } from '../utils/crossLinks.js'
import {
  clearCompleted, createTask, createTaskList, deleteTask, deleteTaskList, emptyTask,
  fetchTaskLists, fetchTasks, googleStatus, googleSyncTasks, normalizeTask, updateTask,
  updateTaskList,
} from '../utils/todoApi.js'
import './TodoSection.css'

const LIST_COLORS = [
  '#246bce', '#2e7d32', '#c2185b', '#6a1b9a', '#00838f',
  '#d84315', '#455a64', '#b8860b', '#7cb342', '#e53935',
]
const PRIORITIES = ['none', 'low', 'medium', 'high']
const PRIORITY_COLOR = { none: 'transparent', low: '#2e7d32', medium: '#e08600', high: '#e53935' }
const icon = (name) => <img src={`/img/icons/${name}.svg`} className="svg-icon-inline" alt="" />

const startOfDay = (d) => { const x = new Date(d); x.setHours(0, 0, 0, 0); return x }
const addDays = (d, n) => { const x = new Date(d); x.setDate(x.getDate() + n); return x }
const sameDay = (a, b) => a && b && a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()

function bucketOf(task) {
  if (task.card.completed) return 'completed'
  const due = parseIsoLocal(task.card.due)
  if (!due) return 'none'
  const d = startOfDay(due)
  const today = startOfDay(new Date())
  if (d < today) return 'overdue'
  if (sameDay(d, today)) return 'today'
  return 'upcoming'
}

function dueLabel(task, locale, t) {
  const due = parseIsoLocal(task.card.due)
  if (!due) return ''
  const d = startOfDay(due)
  const today = startOfDay(new Date())
  let base
  if (sameDay(d, today)) base = t('Today')
  else if (sameDay(d, addDays(today, 1))) base = t('Tomorrow')
  else if (sameDay(d, addDays(today, -1))) base = t('Yesterday')
  else base = due.toLocaleDateString(locale, { day: 'numeric', month: 'short', year: d.getFullYear() !== today.getFullYear() ? 'numeric' : undefined })
  if (task.card.hasDueTime) base += ` ${due.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })}`
  return base
}

export default function TodoSection({ accountId, toolbarStyle = 'icon_text_small' }) {
  const { t, i18n } = useTranslation()
  const locale = i18n.language || 'en'

  const [lists, setLists] = useState([])
  const [counts, setCounts] = useState({ total: 0, today: 0, starred: 0 })
  const [tasks, setTasks] = useState([])
  const [loading, setLoading] = useState(false)
  const [view, setView] = useState({ kind: 'all', listId: null })
  const [search, setSearch] = useState('')
  const [draft, setDraft] = useState(null)
  const [draftId, setDraftId] = useState(null)
  const [saving, setSaving] = useState(false)
  const [quickTitle, setQuickTitle] = useState('')
  const [showCompleted, setShowCompleted] = useState(false)
  const [creatingList, setCreatingList] = useState(false)
  const [renamingId, setRenamingId] = useState(null)
  const [googleAvailable, setGoogleAvailable] = useState(false)
  const [syncingGoogle, setSyncingGoogle] = useState(false)
  const [toasts, setToasts] = useState([])
  const syncBusy = useRef(false)

  const pushToast = useCallback((text, type = 'info') => {
    const id = `${Date.now()}-${Math.random()}`
    setToasts((prev) => [...prev, { id, text, type }])
    setTimeout(() => setToasts((prev) => prev.filter((x) => x.id !== id)), type === 'error' ? 5000 : 3000)
  }, [])
  const dismissToast = useCallback((id) => setToasts((prev) => prev.filter((x) => x.id !== id)), [])

  const reloadLists = useCallback(async () => {
    if (!accountId) return
    try {
      const data = await fetchTaskLists(accountId)
      setLists(data.lists || [])
      setCounts({ total: data.total || 0, today: data.today || 0, starred: data.starred || 0 })
    } catch { /* non-fatal */ }
  }, [accountId])

  const reload = useCallback(async () => {
    if (!accountId) return
    setLoading(true)
    try {
      const list = view.kind === 'list' ? view.listId : null
      const rows = await fetchTasks(accountId, { list, search })
      setTasks((rows || []).map((r) => ({ ...r, card: normalizeTask(r.card) })))
    } catch (e) {
      pushToast(e.message || 'Failed to load tasks', 'error')
    } finally {
      setLoading(false)
    }
  }, [accountId, view, search, pushToast])

  useEffect(() => { reloadLists() }, [reloadLists])
  useEffect(() => {
    const id = setTimeout(reload, search ? 220 : 0)
    return () => clearTimeout(id)
  }, [reload, search])
  useEffect(() => {
    if (!accountId) { setGoogleAvailable(false); return }
    let alive = true
    googleStatus(accountId).then((s) => { if (alive) setGoogleAvailable(!!s.available) }).catch(() => {})
    return () => { alive = false }
  }, [accountId])

  const refreshAll = useCallback(async () => { await Promise.all([reload(), reloadLists()]) }, [reload, reloadLists])

  const defaultListId = useMemo(() => {
    const def = lists.find((l) => l.isDefault) || lists[0]
    return def ? def.listId : null
  }, [lists])

  const activeListId = view.kind === 'list' ? view.listId : defaultListId

  // Apply the smart-view filter on top of the fetched rows.
  const filtered = useMemo(() => {
    if (view.kind === 'today') return tasks.filter((x) => !x.card.completed && ['overdue', 'today'].includes(bucketOf(x)))
    if (view.kind === 'important') return tasks.filter((x) => x.card.starred)
    return tasks
  }, [tasks, view])

  const active = filtered.filter((x) => !x.card.completed)
  const completed = filtered.filter((x) => x.card.completed)

  const buckets = useMemo(() => {
    const order = ['overdue', 'today', 'upcoming', 'none']
    const map = { overdue: [], today: [], upcoming: [], none: [] }
    for (const task of active) map[bucketOf(task)]?.push(task)
    return order.map((key) => ({ key, items: map[key] })).filter((b) => b.items.length)
  }, [active])

  const bucketTitle = (key) => ({
    overdue: t('Overdue'), today: t('Today'), upcoming: t('Upcoming'), none: t('No date'),
  }[key] || key)

  // ── Task ops ──
  const openEditor = useCallback((rec) => { setDraft(normalizeTask(rec.card)); setDraftId(rec.id) }, [])
  const openNew = useCallback(() => {
    const card = emptyTask()
    card.listId = activeListId
    if (view.kind === 'today') card.due = formatIsoLocalDate(new Date())
    if (view.kind === 'important') card.starred = true
    setDraft(card); setDraftId(null)
  }, [activeListId, view])

  // Other workspaces (Contacts, Mail, Calendar) can ask us to open a pre-filled
  // new task via crossLinks.requestNewTask.
  useEffect(() => subscribeNewTask((prefill) => {
    const card = emptyTask()
    Object.assign(card, prefill)
    if (card.listId == null) card.listId = activeListId
    setDraft(normalizeTask(card))
    setDraftId(null)
  }), [activeListId])

  // Turn a task into a calendar event (on its due date, or today).
  const addTaskToCalendar = useCallback((card) => {
    const due = parseIsoLocal(card.due) || new Date()
    requestNewEvent({
      title: card.title,
      description: card.notes || '',
      allDay: !card.hasDueTime,
      start: card.hasDueTime ? formatIsoLocalDateTime(due) : formatIsoLocalDate(due),
      end: card.hasDueTime ? formatIsoLocalDateTime(new Date(due.getTime() + 3600000)) : formatIsoLocalDate(due),
    })
  }, [])

  const handleSave = useCallback(async () => {
    if (!draft) return
    if (!draft.title.trim()) { pushToast(t('Give the task a title.'), 'error'); return }
    setSaving(true)
    try {
      if (draftId) { await updateTask(accountId, draftId, draft); pushToast(t('Task saved')) }
      else { await createTask(accountId, draft); pushToast(t('Task added')) }
      await refreshAll()
      setDraft(null); setDraftId(null)
    } catch (e) {
      pushToast(e.message || 'Save failed', 'error')
    } finally {
      setSaving(false)
    }
  }, [accountId, draft, draftId, refreshAll, pushToast, t])

  const handleDelete = useCallback(async (rec) => {
    const id = rec ? rec.id : draftId
    if (!id) return
    if (rec && !window.confirm(t('Delete this task?'))) return
    try {
      await deleteTask(accountId, id)
      setDraft(null); setDraftId(null)
      await refreshAll()
      pushToast(t('Task deleted'))
    } catch (e) {
      pushToast(e.message || 'Delete failed', 'error')
    }
  }, [accountId, draftId, refreshAll, pushToast, t])

  const toggleComplete = useCallback(async (rec) => {
    const next = !rec.card.completed
    setTasks((prev) => prev.map((x) => (x.id === rec.id ? { ...x, card: { ...x.card, completed: next } } : x)))
    try {
      await updateTask(accountId, rec.id, { ...normalizeTask(rec.card), completed: next })
      reloadLists()
    } catch (e) {
      pushToast(e.message || 'Update failed', 'error')
      reload()
    }
  }, [accountId, reloadLists, reload, pushToast])

  const toggleStar = useCallback(async (rec) => {
    const next = !rec.card.starred
    setTasks((prev) => prev.map((x) => (x.id === rec.id ? { ...x, card: { ...x.card, starred: next } } : x)))
    try {
      await updateTask(accountId, rec.id, { ...normalizeTask(rec.card), starred: next })
      reloadLists()
    } catch (e) {
      pushToast(e.message || 'Update failed', 'error')
      reload()
    }
  }, [accountId, reloadLists, reload, pushToast])

  const quickAdd = useCallback(async () => {
    const title = quickTitle.trim()
    if (!title) return
    setQuickTitle('')
    const card = emptyTask()
    card.title = title
    card.listId = activeListId
    if (view.kind === 'today') card.due = formatIsoLocalDate(new Date())
    if (view.kind === 'important') card.starred = true
    try {
      await createTask(accountId, card)
      await refreshAll()
    } catch (e) {
      pushToast(e.message || 'Could not add task', 'error')
    }
  }, [quickTitle, activeListId, view, accountId, refreshAll, pushToast])

  const handleClearCompleted = useCallback(async () => {
    if (!completed.length) return
    if (!window.confirm(t('Remove all completed tasks here?'))) return
    try {
      await clearCompleted(accountId, view.kind === 'list' ? view.listId : null)
      await refreshAll()
      pushToast(t('Completed tasks removed'))
    } catch (e) {
      pushToast(e.message || 'Failed', 'error')
    }
  }, [accountId, completed.length, view, refreshAll, pushToast, t])

  const runSyncRef = useRef(async () => {})
  runSyncRef.current = async ({ silent = false } = {}) => {
    if (!googleAvailable || syncBusy.current) return
    syncBusy.current = true
    setSyncingGoogle(true)
    try {
      const r = await googleSyncTasks(accountId)
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

  // ── List management ──
  const submitCreateList = useCallback(async (name) => {
    const trimmed = (name || '').trim()
    setCreatingList(false)
    if (!trimmed) return
    const used = new Set(lists.map((l) => l.color))
    const color = LIST_COLORS.find((c) => !used.has(c)) || LIST_COLORS[lists.length % LIST_COLORS.length]
    try {
      const l = await createTaskList(accountId, { name: trimmed, color })
      await reloadLists()
      setView({ kind: 'list', listId: l.listId })
    } catch (e) {
      pushToast(e.message || 'Could not create list', 'error')
    }
  }, [accountId, lists, reloadLists, pushToast])

  const submitRenameList = useCallback(async (listId, name) => {
    const trimmed = (name || '').trim()
    setRenamingId(null)
    if (!trimmed) return
    try { await updateTaskList(accountId, listId, { name: trimmed }); await reloadLists() }
    catch (e) { pushToast(e.message || 'Could not rename list', 'error') }
  }, [accountId, reloadLists, pushToast])

  const recolorList = useCallback(async (list, color) => {
    setLists((prev) => prev.map((l) => (l.listId === list.listId ? { ...l, color } : l)))
    try { await updateTaskList(accountId, list.listId, { color }) }
    catch (e) { pushToast(e.message || 'Update failed', 'error'); reloadLists() }
  }, [accountId, pushToast, reloadLists])

  const handleDeleteList = useCallback(async (list) => {
    if (!window.confirm(t('Delete the list “{{name}}” and all its tasks?', { name: list.name }))) return
    try {
      await deleteTaskList(accountId, list.listId)
      if (view.kind === 'list' && view.listId === list.listId) setView({ kind: 'all', listId: null })
      await refreshAll()
      pushToast(t('List deleted'))
    } catch (e) {
      pushToast(e.message || 'Could not delete list', 'error')
    }
  }, [accountId, view, refreshAll, pushToast, t])

  useEffect(() => {
    const onKey = (e) => { if (e.key === 'Escape') { setDraft(null); setDraftId(null) } }
    if (draft) { window.addEventListener('keydown', onKey); return () => window.removeEventListener('keydown', onKey) }
  }, [draft])

  if (!accountId) {
    return <div className="td-root td-empty-account"><p>{t('Select an account to manage tasks.')}</p></div>
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

  const headerTitle = view.kind === 'all' ? t('All tasks')
    : view.kind === 'today' ? t('Today')
      : view.kind === 'important' ? t('Important')
        : (lists.find((l) => l.listId === view.listId)?.name || t('Tasks'))

  return (
    <div className="td-root">
      {/* Toolbar */}
      <div className={`db-submenu db-submenu--${tb} td-toolbar-bar`}>
        <ul>
          <li><button className="db-submenu-main-btn" onClick={openNew}>{btn(icon('plus'), t('New task'))}</button></li>
          <li><button className="db-submenu-main-btn" onClick={() => setCreatingList(true)}>{btn(icon('label'), t('New list'))}</button></li>
          <li className="td-toolbar-sep" aria-hidden="true" />
          <li><button className="db-submenu-main-btn" onClick={refreshAll}>{btn(icon('reload'), t('Refresh'))}</button></li>
          {googleAvailable && <li className="td-toolbar-sep" aria-hidden="true" />}
          {googleAvailable && (
            <li><button className="db-submenu-main-btn" onClick={handleGoogleSync} disabled={syncingGoogle}>{btn(icon('online'), syncingGoogle ? t('Syncing…') : t('Sync with Google'))}</button></li>
          )}
        </ul>
      </div>

      <div className="td-body">
        {/* Sidebar */}
        <nav className="td-lists-pane">
          <button className={`td-nav-item ${view.kind === 'all' ? 'active' : ''}`} onClick={() => setView({ kind: 'all', listId: null })}>
            <span className="td-nav-ic">{icon('all-mails')}</span>
            <span className="td-nav-label">{t('All tasks')}</span>
            <span className="td-nav-count">{counts.total}</span>
          </button>
          <button className={`td-nav-item ${view.kind === 'today' ? 'active' : ''}`} onClick={() => setView({ kind: 'today', listId: null })}>
            <span className="td-nav-ic">📅</span>
            <span className="td-nav-label">{t('Today')}</span>
            <span className="td-nav-count">{counts.today}</span>
          </button>
          <button className={`td-nav-item ${view.kind === 'important' ? 'active' : ''}`} onClick={() => setView({ kind: 'important', listId: null })}>
            <span className="td-nav-ic td-nav-star">★</span>
            <span className="td-nav-label">{t('Important')}</span>
            <span className="td-nav-count">{counts.starred}</span>
          </button>

          <div className="td-nav-divider" />
          <div className="td-lists-head">
            <span>{t('Lists')}</span>
            <button className="td-icon-plus" title={t('New list')} onClick={() => setCreatingList(true)}>＋</button>
          </div>

          {creatingList && <InlineNameInput placeholder={t('List name')} onSubmit={submitCreateList} onCancel={() => setCreatingList(false)} />}

          {lists.map((l) => (
            renamingId === l.listId ? (
              <InlineNameInput key={l.listId} initial={l.name} onSubmit={(name) => submitRenameList(l.listId, name)} onCancel={() => setRenamingId(null)} />
            ) : (
              <div key={l.listId} className={`td-nav-item td-list-row ${view.kind === 'list' && view.listId === l.listId ? 'active' : ''}`}>
                <button className="td-list-row-main" onClick={() => setView({ kind: 'list', listId: l.listId })}>
                  <ColorDot color={l.color} onPick={(c) => recolorList(l, c)} />
                  <span className="td-nav-label">{l.name}</span>
                  <span className="td-nav-count">{l.count}</span>
                </button>
                <span className="td-list-row-actions">
                  <button title={t('Rename')} onClick={() => setRenamingId(l.listId)}>✎</button>
                  {!l.isDefault && <button title={t('Delete list')} onClick={() => handleDeleteList(l)}>✕</button>}
                </span>
              </div>
            )
          ))}
        </nav>

        {/* Main */}
        <section className="td-main">
          <div className="td-main-head">
            <h2 className="td-main-title">{headerTitle}{loading && <span className="td-loading">•••</span>}</h2>
            <div className="td-main-tools">
              <input className="td-search" type="search" placeholder={t('Search tasks')} value={search} onChange={(e) => setSearch(e.target.value)} />
            </div>
          </div>

          <div className="td-quickadd">
            <span className="td-quickadd-plus">＋</span>
            <input className="td-quickadd-input" placeholder={t('Add a task and press Enter')} value={quickTitle}
              onChange={(e) => setQuickTitle(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') quickAdd() }} />
          </div>

          <div className="td-scroll">
            {!loading && active.length === 0 && completed.length === 0 && (
              <div className="td-empty">
                <div className="td-empty-icon">✅</div>
                <p>{search ? t('No matching tasks.') : t('Nothing here yet. Add your first task above.')}</p>
              </div>
            )}

            {buckets.map((b) => (
              <div key={b.key} className="td-bucket">
                <div className={`td-bucket-head ${b.key}`}>{bucketTitle(b.key)} <span className="td-bucket-count">{b.items.length}</span></div>
                {b.items.map((rec) => (
                  <TaskRow key={rec.id} rec={rec} locale={locale} t={t} showList={view.kind !== 'list'} lists={lists}
                    onToggle={() => toggleComplete(rec)} onStar={() => toggleStar(rec)} onOpen={() => openEditor(rec)} />
                ))}
              </div>
            ))}

            {completed.length > 0 && (
              <div className="td-bucket td-completed">
                <button className="td-bucket-head td-completed-head" onClick={() => setShowCompleted((s) => !s)}>
                  <span>{showCompleted ? '▾' : '▸'} {t('Completed')}</span>
                  <span className="td-bucket-count">{completed.length}</span>
                  <button className="td-clear-btn" onClick={(e) => { e.stopPropagation(); handleClearCompleted() }}>{t('Clear')}</button>
                </button>
                {showCompleted && completed.map((rec) => (
                  <TaskRow key={rec.id} rec={rec} locale={locale} t={t} showList={view.kind !== 'list'} lists={lists}
                    onToggle={() => toggleComplete(rec)} onStar={() => toggleStar(rec)} onOpen={() => openEditor(rec)} />
                ))}
              </div>
            )}
          </div>
        </section>
      </div>

      {draft && (
        <TaskEditor t={t} draft={draft} setDraft={setDraft} saving={saving} isNew={!draftId} lists={lists}
          onCancel={() => { setDraft(null); setDraftId(null) }} onSave={handleSave}
          onAddToCalendar={() => addTaskToCalendar(draft)}
          onDelete={draftId ? () => handleDelete(null) : null} />
      )}

      <div className="td-toasts">
        {toasts.map((toast) => (
          <div key={toast.id} className={`td-toast td-toast--${toast.type}`}>
            <span className="td-toast-text">{toast.text}</span>
            <button className="td-toast-close" onClick={() => dismissToast(toast.id)} title={t('Dismiss')}>✕</button>
          </div>
        ))}
      </div>
    </div>
  )
}

// ─────────────────────────── Task row ───────────────────────────

function TaskRow({ rec, locale, t, showList, lists, onToggle, onStar, onOpen }) {
  const card = rec.card
  const overdue = !card.completed && bucketOf(rec) === 'overdue'
  const list = lists.find((l) => l.listId === card.listId)
  const subDone = (card.subtasks || []).filter((s) => s.done).length
  const subTotal = (card.subtasks || []).length
  const due = dueLabel(rec, locale, t)

  return (
    <div className={`td-task ${card.completed ? 'done' : ''}`}>
      <button className={`td-check ${card.completed ? 'on' : ''}`} onClick={onToggle} title={t('Toggle complete')} aria-label={t('Toggle complete')}>
        {card.completed ? '✓' : ''}
      </button>
      {card.priority !== 'none' && <span className="td-prio" style={{ background: PRIORITY_COLOR[card.priority] }} title={card.priority} />}
      <button className="td-task-main" onClick={onOpen}>
        <span className="td-task-title">{card.title || t('(untitled task)')}</span>
        <span className="td-task-meta">
          {due && <span className={`td-due ${overdue ? 'overdue' : ''}`}>{due}</span>}
          {subTotal > 0 && <span className="td-sub">☑ {subDone}/{subTotal}</span>}
          {showList && list && <span className="td-task-list"><span className="td-task-list-dot" style={{ background: list.color }} />{list.name}</span>}
          {card.notes && <span className="td-note-ic" title={t('Has notes')}>📝</span>}
        </span>
      </button>
      <button className={`td-star ${card.starred ? 'on' : ''}`} onClick={onStar} title={t('Toggle important')}>{card.starred ? '★' : '☆'}</button>
    </div>
  )
}

// ─────────────────────────── Editor ───────────────────────────

function splitDue(due) {
  const d = parseIsoLocal(due)
  if (!d) return { date: '', time: '09:00' }
  return {
    date: formatIsoLocalDate(d),
    time: `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`,
  }
}

function TaskEditor({ t, draft, setDraft, saving, isNew, lists, onCancel, onSave, onDelete, onAddToCalendar }) {
  const set = (patch) => setDraft((d) => ({ ...d, ...patch }))
  const { date, time } = splitDue(draft.due)
  const [newSub, setNewSub] = useState('')

  const setDate = (val) => {
    if (!val) { set({ due: '', hasDueTime: false }); return }
    if (draft.hasDueTime) set({ due: `${val}T${time}` })
    else set({ due: val })
  }
  const setTime = (val) => set({ due: `${date || formatIsoLocalDate(new Date())}T${val}`, hasDueTime: true })
  const toggleTime = (checked) => {
    if (checked) set({ due: `${date || formatIsoLocalDate(new Date())}T09:00`, hasDueTime: true })
    else set({ due: date || draft.due.split('T')[0] || '', hasDueTime: false })
  }

  const setSub = (i, patch) => set({ subtasks: draft.subtasks.map((s, j) => (j === i ? { ...s, ...patch } : s)) })
  const addSub = () => { const v = newSub.trim(); if (!v) return; set({ subtasks: [...draft.subtasks, { title: v, done: false }] }); setNewSub('') }

  return (
    <div className="td-modal-overlay" onClick={onCancel}>
      <div className="td-editor" onClick={(e) => e.stopPropagation()}>
        <div className="td-editor-topbar">
          <div className="td-editor-title">{isNew ? t('New task') : t('Edit task')}</div>
          <div className="td-editor-actions">
            <button className="td-btn" onClick={onCancel}>{t('Cancel')}</button>
            <button className="td-btn td-btn--primary" onClick={onSave} disabled={saving}>{saving ? t('Saving…') : t('Save')}</button>
          </div>
        </div>

        <div className="td-editor-body">
          <input className="td-input td-title-input" placeholder={t('Task title')} value={draft.title} autoFocus onChange={(e) => set({ title: e.target.value })} />

          <div className="td-form-row2">
            <label className="td-labeled">
              <span>{t('List')}</span>
              <select className="td-select" value={draft.listId ?? ''} onChange={(e) => set({ listId: e.target.value ? Number(e.target.value) : null })}>
                {lists.map((l) => <option key={l.listId} value={l.listId}>{l.name}</option>)}
              </select>
            </label>
            <label className="td-labeled">
              <span>{t('Priority')}</span>
              <select className="td-select" value={draft.priority} onChange={(e) => set({ priority: e.target.value })}>
                {PRIORITIES.map((p) => <option key={p} value={p}>{t(p === 'none' ? 'None' : p.charAt(0).toUpperCase() + p.slice(1))}</option>)}
              </select>
            </label>
          </div>

          <div className="td-form-row2">
            <label className="td-labeled">
              <span>{t('Due date')}</span>
              <div className="td-datetime">
                <input className="td-input" type="date" value={date} onChange={(e) => setDate(e.target.value)} />
                {draft.hasDueTime && <input className="td-input" type="time" value={time} onChange={(e) => setTime(e.target.value)} />}
              </div>
            </label>
            <div className="td-labeled td-due-toggles">
              <span>&nbsp;</span>
              <div>
                <label className="td-check"><input type="checkbox" checked={draft.hasDueTime} onChange={(e) => toggleTime(e.target.checked)} /><span>{t('Set time')}</span></label>
                <label className="td-check"><input type="checkbox" checked={draft.starred} onChange={(e) => set({ starred: e.target.checked })} /><span>{t('Important')}</span></label>
              </div>
            </div>
          </div>

          <label className="td-labeled">
            <span>{t('Notes')}</span>
            <textarea className="td-textarea" rows={3} placeholder={t('Add notes')} value={draft.notes} onChange={(e) => set({ notes: e.target.value })} />
          </label>

          <div className="td-form-section">
            <div className="td-form-section-title">{t('Subtasks')}</div>
            {draft.subtasks.map((s, i) => (
              <div key={i} className="td-subrow">
                <button className={`td-check td-check--sm ${s.done ? 'on' : ''}`} onClick={() => setSub(i, { done: !s.done })}>{s.done ? '✓' : ''}</button>
                <input className="td-input" value={s.title} onChange={(e) => setSub(i, { title: e.target.value })} />
                <button className="td-icon-btn" onClick={() => set({ subtasks: draft.subtasks.filter((_, j) => j !== i) })}>✕</button>
              </div>
            ))}
            <div className="td-subrow td-subrow--add">
              <span className="td-check td-check--sm td-check--ghost" />
              <input className="td-input" placeholder={t('Add a subtask')} value={newSub} onChange={(e) => setNewSub(e.target.value)}
                onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addSub() } }} />
              <button className="td-add-btn" onClick={addSub} disabled={!newSub.trim()}>＋</button>
            </div>
          </div>
        </div>

        {(onDelete || onAddToCalendar) && (
          <div className="td-editor-footer">
            {onAddToCalendar && draft.title.trim() && (
              <button className="td-btn" onClick={onAddToCalendar}>{t('Add to calendar')}</button>
            )}
            {onDelete && <button className="td-btn td-btn--danger td-footer-right" onClick={onDelete}>{t('Delete task')}</button>}
          </div>
        )}
      </div>
    </div>
  )
}

// ─────────────────────────── shared bits ───────────────────────────

function InlineNameInput({ initial = '', placeholder, onSubmit, onCancel }) {
  const [value, setValue] = useState(initial)
  const ref = useRef(null)
  useEffect(() => { ref.current?.focus(); ref.current?.select() }, [])
  return (
    <input ref={ref} className="td-name-input" value={value} placeholder={placeholder}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => { if (e.key === 'Enter') onSubmit(value); else if (e.key === 'Escape') onCancel() }}
      onBlur={() => onSubmit(value)} />
  )
}

function ColorDot({ color, onPick }) {
  const [open, setOpen] = useState(false)
  return (
    <span className="td-colordot-wrap" onClick={(e) => e.preventDefault()}>
      <button type="button" className="td-colordot" style={{ background: color }} onClick={(e) => { e.preventDefault(); e.stopPropagation(); setOpen((o) => !o) }} />
      {open && (
        <span className="td-colorpop" onMouseLeave={() => setOpen(false)}>
          {LIST_COLORS.map((c) => (
            <button key={c} type="button" className="td-colorpop-swatch" style={{ background: c }}
              onClick={(e) => { e.preventDefault(); e.stopPropagation(); onPick(c); setOpen(false) }} />
          ))}
        </span>
      )}
    </span>
  )
}
