import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { requestNewCompose } from '../utils/mailtoInbox.js'
import { requestNewTask, subscribeNewEvent } from '../utils/crossLinks.js'
import {
  createCalendar, createEvent, deleteCalendar, deleteEvent, emptyEvent, exportIcs,
  fetchCalendars, fetchEvents, formatIsoLocalDate, formatIsoLocalDateTime, getEvent,
  googleStatus, googleSyncCalendar,
  importIcs, naiveMsLocal, normalizeEvent, parseIsoLocal, updateCalendar, updateEvent,
} from '../utils/calendarApi.js'
import './CalendarSection.css'

const WEEK_START = 1 // Monday
const HOUR_PX = 46
const CALENDAR_COLORS = [
  '#246bce', '#2e7d32', '#c2185b', '#6a1b9a', '#00838f',
  '#d84315', '#455a64', '#b8860b', '#7cb342', '#e53935',
]
const REMINDER_OPTIONS = [0, 5, 10, 15, 30, 60, 120, 1440]
const icon = (name) => <img src={`/img/icons/${name}.svg`} className="svg-icon-inline" alt="" />

// ── date helpers ──
const startOfDay = (d) => { const x = new Date(d); x.setHours(0, 0, 0, 0); return x }
const addDays = (d, n) => { const x = new Date(d); x.setDate(x.getDate() + n); return x }
const addMonths = (d, n) => { const x = new Date(d); x.setMonth(x.getMonth() + n); return x }
const sameDay = (a, b) => a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
const isToday = (d) => sameDay(d, new Date())
function startOfWeek(d) {
  const x = startOfDay(d)
  const diff = (x.getDay() - WEEK_START + 7) % 7
  return addDays(x, -diff)
}
function clampColor(c, fallback = '#246bce') {
  return typeof c === 'string' && /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(c.trim()) ? c.trim() : fallback
}

function downloadText(filename, text, mime = 'text/calendar;charset=utf-8') {
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

export default function CalendarSection({ accountId, toolbarStyle = 'icon_text_small' }) {
  const { t, i18n } = useTranslation()
  const locale = i18n.language || 'en'

  const [calendars, setCalendars] = useState([])
  const [events, setEvents] = useState([])
  const [loading, setLoading] = useState(false)
  const [view, setView] = useState(() => localStorage.getItem('calendar_view') || 'month')
  const [cursor, setCursor] = useState(() => startOfDay(new Date()))
  const [search, setSearch] = useState('')
  const [selected, setSelected] = useState(null) // event instance being viewed
  const [draft, setDraft] = useState(null) // event card being edited
  const [draftMode, setDraftMode] = useState('create') // 'create' | 'edit'
  const [saving, setSaving] = useState(false)
  const [toasts, setToasts] = useState([])
  const [creatingCal, setCreatingCal] = useState(false)
  const [renamingCal, setRenamingCal] = useState(null)
  const [googleAvailable, setGoogleAvailable] = useState(false)
  const [syncingGoogle, setSyncingGoogle] = useState(false)
  const fileInputRef = useRef(null)
  const firedReminders = useRef(new Set())
  const reminderTimers = useRef([])

  const pushToast = useCallback((text, type = 'info') => {
    const id = `${Date.now()}-${Math.random()}`
    setToasts((prev) => [...prev, { id, text, type }])
    setTimeout(() => setToasts((prev) => prev.filter((x) => x.id !== id)), type === 'error' ? 5000 : 3200)
  }, [])
  const dismissToast = useCallback((id) => setToasts((prev) => prev.filter((x) => x.id !== id)), [])

  useEffect(() => { localStorage.setItem('calendar_view', view) }, [view])

  const visibleCalIds = useMemo(
    () => calendars.filter((c) => c.isVisible).map((c) => c.calendarId),
    [calendars],
  )
  const calColorMap = useMemo(() => {
    const m = new Map()
    calendars.forEach((c) => m.set(c.calendarId, c.color))
    return m
  }, [calendars])

  // The date range the current view needs to cover.
  const range = useMemo(() => {
    if (view === 'month') {
      const first = new Date(cursor.getFullYear(), cursor.getMonth(), 1)
      const gridStart = startOfWeek(first)
      return { start: gridStart, end: addDays(gridStart, 42) }
    }
    if (view === 'week') {
      const s = startOfWeek(cursor)
      return { start: s, end: addDays(s, 7) }
    }
    if (view === 'day') {
      const s = startOfDay(cursor)
      return { start: s, end: addDays(s, 1) }
    }
    // agenda: 45 days from cursor
    const s = startOfDay(cursor)
    return { start: s, end: addDays(s, 45) }
  }, [view, cursor])

  const reloadCalendars = useCallback(async () => {
    if (!accountId) return
    try {
      const data = await fetchCalendars(accountId)
      setCalendars(data.calendars || [])
    } catch {
      // non-fatal
    }
  }, [accountId])

  const reloadEvents = useCallback(async () => {
    if (!accountId) return
    setLoading(true)
    try {
      const rows = await fetchEvents(accountId, {
        from: naiveMsLocal(range.start),
        to: naiveMsLocal(range.end),
        search,
        calendars: visibleCalIds,
      })
      setEvents((rows || []).map((r) => ({ ...r, card: normalizeEvent(r.card) })))
    } catch (e) {
      pushToast(e.message || 'Failed to load events', 'error')
    } finally {
      setLoading(false)
    }
  }, [accountId, range, search, visibleCalIds, pushToast])

  useEffect(() => { reloadCalendars() }, [reloadCalendars])
  useEffect(() => {
    const id = setTimeout(reloadEvents, search ? 220 : 0)
    return () => clearTimeout(id)
  }, [reloadEvents, search])

  useEffect(() => {
    if (!accountId) { setGoogleAvailable(false); return }
    let alive = true
    googleStatus(accountId).then((s) => { if (alive) setGoogleAvailable(!!s.available) }).catch(() => {})
    return () => { alive = false }
  }, [accountId])

  const refreshAll = useCallback(async () => {
    await Promise.all([reloadEvents(), reloadCalendars()])
  }, [reloadEvents, reloadCalendars])

  // ── Reminder scheduling: fire a notification/toast as reminders come due. ──
  useEffect(() => {
    reminderTimers.current.forEach((tm) => clearTimeout(tm))
    reminderTimers.current = []
    const now = Date.now()
    const HORIZON = 12 * 60 * 60 * 1000
    for (const ev of events) {
      const startDate = parseIsoLocal(ev.card.start)
      if (!startDate) continue
      const startMs = startDate.getTime()
      for (const mins of ev.card.reminders || []) {
        const fireAt = startMs - mins * 60000
        const key = `${ev.instanceKey}:${mins}`
        if (fireAt <= now || fireAt - now > HORIZON || firedReminders.current.has(key)) continue
        const tm = setTimeout(() => {
          firedReminders.current.add(key)
          const when = startDate.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })
          const label = ev.card.title || t('(untitled event)')
          pushToast(`⏰ ${label} — ${when}`, 'info')
          try {
            if (typeof Notification !== 'undefined' && Notification.permission === 'granted') {
              const n = new Notification(label, { body: t('Starts at {{time}}', { time: when }) })
              void n
            }
          } catch { /* ignore */ }
        }, fireAt - now)
        reminderTimers.current.push(tm)
      }
    }
    return () => { reminderTimers.current.forEach((tm) => clearTimeout(tm)) }
  }, [events, locale, t, pushToast])

  useEffect(() => {
    try {
      if (typeof Notification !== 'undefined' && Notification.permission === 'default') {
        Notification.requestPermission().catch(() => {})
      }
    } catch { /* ignore */ }
  }, [])

  // ── Navigation ──
  const goToday = () => setCursor(startOfDay(new Date()))
  const goPrev = () => setCursor((c) => (view === 'month' ? addMonths(c, -1) : view === 'week' ? addDays(c, -7) : view === 'day' ? addDays(c, -1) : addDays(c, -45)))
  const goNext = () => setCursor((c) => (view === 'month' ? addMonths(c, 1) : view === 'week' ? addDays(c, 7) : view === 'day' ? addDays(c, 1) : addDays(c, 45)))

  const rangeTitle = useMemo(() => {
    if (view === 'month') return cursor.toLocaleDateString(locale, { month: 'long', year: 'numeric' })
    if (view === 'day') return cursor.toLocaleDateString(locale, { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' })
    if (view === 'week') {
      const s = startOfWeek(cursor); const e = addDays(s, 6)
      const sameMonth = s.getMonth() === e.getMonth()
      const left = s.toLocaleDateString(locale, { day: 'numeric', month: 'short' })
      const right = e.toLocaleDateString(locale, sameMonth ? { day: 'numeric', month: 'short', year: 'numeric' } : { day: 'numeric', month: 'short', year: 'numeric' })
      return `${left} – ${right}`
    }
    const s = startOfDay(cursor); const e = addDays(s, 44)
    return `${s.toLocaleDateString(locale, { day: 'numeric', month: 'short' })} – ${e.toLocaleDateString(locale, { day: 'numeric', month: 'short', year: 'numeric' })}`
  }, [view, cursor, locale])

  // ── Editor lifecycle ──
  const defaultCalendarId = useMemo(() => {
    const def = calendars.find((c) => c.isDefault) || calendars[0]
    return def ? def.calendarId : null
  }, [calendars])

  const startCreate = useCallback((prefill) => {
    const card = emptyEvent()
    card.calendarId = defaultCalendarId
    if (prefill) Object.assign(card, prefill)
    if (!card.start) {
      const base = new Date()
      base.setMinutes(0, 0, 0)
      base.setHours(base.getHours() + 1)
      card.start = formatIsoLocalDateTime(base)
      card.end = formatIsoLocalDateTime(new Date(base.getTime() + 3600000))
    }
    setSelected(null)
    setDraft(normalizeEvent(card))
    setDraftMode('create')
  }, [defaultCalendarId])

  const startCreateAt = useCallback((date, { allDay = false } = {}) => {
    const card = emptyEvent()
    card.calendarId = defaultCalendarId
    card.allDay = allDay
    if (allDay) {
      card.start = formatIsoLocalDate(date)
      card.end = formatIsoLocalDate(date)
    } else {
      const s = new Date(date)
      if (s.getHours() === 0 && s.getMinutes() === 0) s.setHours(9)
      card.start = formatIsoLocalDateTime(s)
      card.end = formatIsoLocalDateTime(new Date(s.getTime() + 3600000))
    }
    setSelected(null)
    setDraft(normalizeEvent(card))
    setDraftMode('create')
  }, [defaultCalendarId])

  const openEvent = useCallback((inst) => {
    setSelected(inst)
    setDraft(null)
  }, [])

  // Other workspaces (Contacts, Mail, Todo) can ask us to open a pre-filled new
  // event via crossLinks.requestNewEvent.
  useEffect(() => subscribeNewEvent((prefill) => startCreate(prefill)), [startCreate])

  // Turn an event into a follow-up task in the Todo workspace.
  const makeTaskFromEvent = useCallback((inst) => {
    const startDate = parseIsoLocal(inst.card.start)
    requestNewTask({
      title: inst.card.title || t('Untitled event'),
      notes: inst.card.location ? `${t('Location')}: ${inst.card.location}` : '',
      due: startDate ? formatIsoLocalDate(startDate) : '',
    })
  }, [t])

  const startEdit = useCallback(async (inst) => {
    // Recurring instances only carry the occurrence's times; load the master so
    // edits apply to the real series start.
    let card = normalizeEvent(inst.card)
    if (inst.isRecurring) {
      try {
        const master = await getEvent(accountId, inst.id)
        card = normalizeEvent(master.card)
      } catch { /* fall back to the instance card */ }
    }
    setDraft(card)
    setDraftMode('edit')
    setSelected(inst)
  }, [accountId])

  const cancelEdit = useCallback(() => setDraft(null), [])

  const handleSave = useCallback(async () => {
    if (!draft) return
    if (!draft.start) { pushToast(t('Please pick a start date.'), 'error'); return }
    setSaving(true)
    try {
      let rec
      if (draftMode === 'create') {
        rec = await createEvent(accountId, draft)
        pushToast(t('Event added'))
      } else {
        rec = await updateEvent(accountId, selected.id, draft)
        pushToast(t('Event saved'))
      }
      await refreshAll()
      setDraft(null)
      setSelected(null)
      void rec
    } catch (e) {
      pushToast(e.message || 'Save failed', 'error')
    } finally {
      setSaving(false)
    }
  }, [accountId, draft, draftMode, selected, refreshAll, pushToast, t])

  const handleDelete = useCallback(async (inst) => {
    const target = inst || selected
    if (!target) return
    const msg = target.isRecurring
      ? t('Delete the whole repeating series “{{title}}”?', { title: target.card.title || t('(untitled event)') })
      : t('Delete this event?')
    if (!window.confirm(msg)) return
    try {
      await deleteEvent(accountId, target.id)
      setSelected(null)
      setDraft(null)
      await refreshAll()
      pushToast(t('Event deleted'))
    } catch (e) {
      pushToast(e.message || 'Delete failed', 'error')
    }
  }, [accountId, selected, refreshAll, pushToast, t])

  const emailAttendees = useCallback((inst) => {
    const emails = (inst.card.attendees || []).map((a) => a.email).filter(Boolean)
    if (!emails.length) { pushToast(t('This event has no attendees with an email.'), 'error'); return }
    requestNewCompose({ to: emails.join(', '), subject: inst.card.title || '' })
  }, [pushToast, t])

  // ── Import / export ──
  const handleImportFile = useCallback(async (event) => {
    const file = event.target.files?.[0]
    event.target.value = ''
    if (!file) return
    try {
      const text = await file.text()
      if (!/BEGIN:VCALENDAR|BEGIN:VEVENT/i.test(text)) {
        pushToast(t('This file is not a calendar. Choose an .ics file exported from another calendar app.'), 'error')
        return
      }
      const result = await importIcs(accountId, text, { calendarId: defaultCalendarId })
      await refreshAll()
      const parts = []
      if (result.imported) parts.push(t('{{n}} imported', { n: result.imported }))
      if (result.skipped) parts.push(t('{{n}} skipped', { n: result.skipped }))
      pushToast(parts.join(' · ') || t('Import complete'))
    } catch (e) {
      pushToast(e.message || 'Import failed', 'error')
    }
  }, [accountId, defaultCalendarId, refreshAll, pushToast, t])

  const handleExport = useCallback(async () => {
    try {
      downloadText('guvercin-calendar.ics', await exportIcs(accountId, {}))
    } catch (e) {
      pushToast(e.message || 'Export failed', 'error')
    }
  }, [accountId, pushToast])

  const handleGoogleSync = useCallback(async () => {
    setSyncingGoogle(true)
    try {
      const r = await googleSyncCalendar(accountId)
      await refreshAll()
      pushToast(t('Synced {{n}} events from Google Calendar', { n: r.events || 0 }))
    } catch (e) {
      pushToast(e.message || 'Google sync failed', 'error')
    } finally {
      setSyncingGoogle(false)
    }
  }, [accountId, refreshAll, pushToast, t])

  // ── Calendar management ──
  const submitCreateCal = useCallback(async (name) => {
    const trimmed = (name || '').trim()
    setCreatingCal(false)
    if (!trimmed) return
    const used = new Set(calendars.map((c) => c.color))
    const color = CALENDAR_COLORS.find((c) => !used.has(c)) || CALENDAR_COLORS[calendars.length % CALENDAR_COLORS.length]
    try {
      await createCalendar(accountId, { name: trimmed, color })
      await reloadCalendars()
    } catch (e) {
      pushToast(e.message || 'Could not create calendar', 'error')
    }
  }, [accountId, calendars, reloadCalendars, pushToast])

  const submitRenameCal = useCallback(async (calendarId, name) => {
    const trimmed = (name || '').trim()
    setRenamingCal(null)
    if (!trimmed) return
    try {
      await updateCalendar(accountId, calendarId, { name: trimmed })
      await reloadCalendars()
    } catch (e) {
      pushToast(e.message || 'Could not rename calendar', 'error')
    }
  }, [accountId, reloadCalendars, pushToast])

  const toggleCalVisible = useCallback(async (cal) => {
    setCalendars((prev) => prev.map((c) => (c.calendarId === cal.calendarId ? { ...c, isVisible: !c.isVisible } : c)))
    try {
      await updateCalendar(accountId, cal.calendarId, { isVisible: !cal.isVisible })
    } catch (e) {
      pushToast(e.message || 'Update failed', 'error')
      reloadCalendars()
    }
  }, [accountId, pushToast, reloadCalendars])

  const recolorCal = useCallback(async (cal, color) => {
    setCalendars((prev) => prev.map((c) => (c.calendarId === cal.calendarId ? { ...c, color } : c)))
    try {
      await updateCalendar(accountId, cal.calendarId, { color })
      await refreshAll()
    } catch (e) {
      pushToast(e.message || 'Update failed', 'error')
    }
  }, [accountId, refreshAll, pushToast])

  const handleDeleteCal = useCallback(async (cal) => {
    if (!window.confirm(t('Delete the calendar “{{name}}” and all its events?', { name: cal.name }))) return
    try {
      await deleteCalendar(accountId, cal.calendarId)
      await refreshAll()
      pushToast(t('Calendar deleted'))
    } catch (e) {
      pushToast(e.message || 'Could not delete calendar', 'error')
    }
  }, [accountId, refreshAll, pushToast, t])

  // Esc closes editor / detail.
  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape') { setDraft(null); setSelected(null) }
    }
    if (draft || selected) {
      window.addEventListener('keydown', onKey)
      return () => window.removeEventListener('keydown', onKey)
    }
  }, [draft, selected])

  if (!accountId) {
    return <div className="cal-root cal-empty-account"><p>{t('Select an account to use the calendar.')}</p></div>
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

  const VIEW_TABS = [
    { key: 'month', label: t('Month') },
    { key: 'week', label: t('Week') },
    { key: 'day', label: t('Day') },
    { key: 'agenda', label: t('Agenda') },
  ]

  return (
    <div className="cal-root">
      <input ref={fileInputRef} type="file" accept=".ics,text/calendar" style={{ display: 'none' }} onChange={handleImportFile} />

      {/* ── Toolbar (matches the mail ribbon) ── */}
      <div className={`db-submenu db-submenu--${tb} cal-toolbar-bar`}>
        <ul>
          <li><button className="db-submenu-main-btn" onClick={() => startCreate()}>{btn(icon('plus'), t('New event'))}</button></li>
          <li className="cal-toolbar-sep" aria-hidden="true" />
          <li><button className="db-submenu-main-btn" onClick={() => setCreatingCal(true)}>{btn(icon('label'), t('New calendar'))}</button></li>
          <li><button className="db-submenu-main-btn" onClick={refreshAll}>{btn(icon('reload'), t('Refresh'))}</button></li>
          <li className="cal-toolbar-sep" aria-hidden="true" />
          <li><button className="db-submenu-main-btn" onClick={() => fileInputRef.current?.click()}>{btn(icon('folder'), t('Import'))}</button></li>
          <li><button className="db-submenu-main-btn" onClick={handleExport}>{btn(icon('save'), t('Export'))}</button></li>
          {googleAvailable && <li className="cal-toolbar-sep" aria-hidden="true" />}
          {googleAvailable && (
            <li><button className="db-submenu-main-btn" onClick={handleGoogleSync} disabled={syncingGoogle}>{btn(icon('online'), syncingGoogle ? t('Syncing…') : t('Sync with Google'))}</button></li>
          )}
        </ul>
      </div>

      {/* ── Secondary navigation row ── */}
      <div className="cal-navbar">
        <div className="cal-nav-left">
          <button className="cal-today-btn" onClick={goToday}>{t('Today')}</button>
          <button className="cal-nav-arrow" onClick={goPrev} title={t('Previous')}>‹</button>
          <button className="cal-nav-arrow" onClick={goNext} title={t('Next')}>›</button>
          <span className="cal-range-title">{rangeTitle}</span>
          {loading && <span className="cal-loading">•••</span>}
        </div>
        <div className="cal-nav-right">
          <div className="cal-search-wrap">
            <input className="cal-search" type="search" placeholder={t('Search events')} value={search} onChange={(e) => setSearch(e.target.value)} />
          </div>
          <div className="cal-view-tabs">
            {VIEW_TABS.map((v) => (
              <button key={v.key} className={`cal-view-tab ${view === v.key ? 'active' : ''}`} onClick={() => setView(v.key)}>{v.label}</button>
            ))}
          </div>
        </div>
      </div>

      <div className="cal-body">
        {/* ── Sidebar: mini-month + calendars ── */}
        <aside className="cal-sidebar">
          <MiniMonth cursor={cursor} locale={locale} onPick={(d) => setCursor(startOfDay(d))} />

          <div className="cal-cals-head">
            <span>{t('Calendars')}</span>
            <button className="cal-icon-plus" title={t('New calendar')} onClick={() => setCreatingCal(true)}>＋</button>
          </div>

          {creatingCal && (
            <InlineNameInput placeholder={t('Calendar name')} onSubmit={submitCreateCal} onCancel={() => setCreatingCal(false)} />
          )}

          <div className="cal-cals-list">
            {calendars.map((c) => (
              renamingCal === c.calendarId ? (
                <InlineNameInput key={c.calendarId} initial={c.name} onSubmit={(name) => submitRenameCal(c.calendarId, name)} onCancel={() => setRenamingCal(null)} />
              ) : (
                <div key={c.calendarId} className="cal-cal-row">
                  <label className="cal-cal-main">
                    <input type="checkbox" checked={c.isVisible} onChange={() => toggleCalVisible(c)} style={{ accentColor: c.color }} />
                    <ColorDot color={c.color} onPick={(col) => recolorCal(c, col)} />
                    <span className="cal-cal-name" title={c.name}>{c.name}</span>
                    <span className="cal-cal-count">{c.count}</span>
                  </label>
                  <span className="cal-cal-actions">
                    <button title={t('Rename')} onClick={() => setRenamingCal(c.calendarId)}>✎</button>
                    {!c.isDefault && <button title={t('Delete calendar')} onClick={() => handleDeleteCal(c)}>✕</button>}
                  </span>
                </div>
              )
            ))}
          </div>
        </aside>

        {/* ── Main view ── */}
        <section className="cal-main">
          {view === 'month' && (
            <MonthView cursor={cursor} events={events} locale={locale} t={t}
              onDayClick={(d) => startCreateAt(d, { allDay: false })}
              onEventClick={openEvent} onMoreClick={(d) => { setCursor(startOfDay(d)); setView('day') }} />
          )}
          {(view === 'week' || view === 'day') && (
            <TimeGridView days={view === 'week' ? 7 : 1} cursor={cursor} events={events} locale={locale} t={t}
              onSlotClick={(d) => startCreateAt(d)} onAllDayClick={(d) => startCreateAt(d, { allDay: true })}
              onEventClick={openEvent} />
          )}
          {view === 'agenda' && (
            <AgendaView events={events} locale={locale} t={t}
              onEventClick={openEvent} onEmptyNew={() => startCreate()} />
          )}
        </section>
      </div>

      {/* ── Detail popover ── */}
      {selected && !draft && (
        <EventDetail inst={selected} locale={locale} t={t} calColorMap={calColorMap}
          onClose={() => setSelected(null)} onEdit={() => startEdit(selected)}
          onDelete={() => handleDelete(selected)} onEmail={() => emailAttendees(selected)}
          onNewTask={() => { makeTaskFromEvent(selected); setSelected(null) }} />
      )}

      {/* ── Editor modal ── */}
      {draft && (
        <EventEditor t={t} locale={locale} draft={draft} setDraft={setDraft} saving={saving}
          isNew={draftMode === 'create'} calendars={calendars} onCancel={cancelEdit} onSave={handleSave}
          onDelete={draftMode === 'edit' && selected ? () => handleDelete(selected) : null} />
      )}

      <div className="cal-toasts">
        {toasts.map((toast) => (
          <div key={toast.id} className={`cal-toast cal-toast--${toast.type}`}>
            <span className="cal-toast-text">{toast.text}</span>
            <button className="cal-toast-close" onClick={() => dismissToast(toast.id)} title={t('Dismiss')}>✕</button>
          </div>
        ))}
      </div>
    </div>
  )
}

// ─────────────────────────── Mini month ───────────────────────────

function MiniMonth({ cursor, locale, onPick }) {
  const [month, setMonth] = useState(() => new Date(cursor.getFullYear(), cursor.getMonth(), 1))
  useEffect(() => { setMonth(new Date(cursor.getFullYear(), cursor.getMonth(), 1)) }, [cursor])

  const gridStart = startOfWeek(new Date(month.getFullYear(), month.getMonth(), 1))
  const cells = Array.from({ length: 42 }, (_, i) => addDays(gridStart, i))
  const dow = Array.from({ length: 7 }, (_, i) => addDays(gridStart, i).toLocaleDateString(locale, { weekday: 'narrow' }))

  return (
    <div className="cal-mini">
      <div className="cal-mini-head">
        <button onClick={() => setMonth(addMonths(month, -1))}>‹</button>
        <span>{month.toLocaleDateString(locale, { month: 'long', year: 'numeric' })}</span>
        <button onClick={() => setMonth(addMonths(month, 1))}>›</button>
      </div>
      <div className="cal-mini-grid">
        {dow.map((d, i) => <div key={`h${i}`} className="cal-mini-dow">{d}</div>)}
        {cells.map((d, i) => {
          const dim = d.getMonth() !== month.getMonth()
          return (
            <button key={i} className={`cal-mini-cell ${dim ? 'dim' : ''} ${isToday(d) ? 'today' : ''} ${sameDay(d, cursor) ? 'sel' : ''}`} onClick={() => onPick(d)}>
              {d.getDate()}
            </button>
          )
        })}
      </div>
    </div>
  )
}

// ─────────────────────────── Month view ───────────────────────────

function eventStart(ev) { return parseIsoLocal(ev.card.start) || new Date(ev.startMs) }
function eventEnd(ev) { return parseIsoLocal(ev.card.end) || new Date(ev.endMs) }

function MonthView({ cursor, events, locale, t, onDayClick, onEventClick, onMoreClick }) {
  const first = new Date(cursor.getFullYear(), cursor.getMonth(), 1)
  const gridStart = startOfWeek(first)
  const cells = Array.from({ length: 42 }, (_, i) => addDays(gridStart, i))
  const weekdayNames = Array.from({ length: 7 }, (_, i) => addDays(gridStart, i).toLocaleDateString(locale, { weekday: 'short' }))

  const byDay = useMemo(() => {
    const map = new Map()
    for (const ev of events) {
      const s = startOfDay(eventStart(ev))
      const e = startOfDay(eventEnd(ev))
      let d = new Date(s)
      let guard = 0
      while (d <= e && guard < 60) {
        const key = formatIsoLocalDate(d)
        if (!map.has(key)) map.set(key, [])
        map.get(key).push(ev)
        d = addDays(d, 1)
        guard++
      }
    }
    return map
  }, [events])

  return (
    <div className="cal-month">
      <div className="cal-month-dow">
        {weekdayNames.map((w, i) => <div key={i} className="cal-month-dow-cell">{w}</div>)}
      </div>
      <div className="cal-month-grid">
        {cells.map((d, i) => {
          const key = formatIsoLocalDate(d)
          const dayEvents = (byDay.get(key) || []).slice().sort((a, b) => a.startMs - b.startMs)
          const dim = d.getMonth() !== cursor.getMonth()
          const shown = dayEvents.slice(0, 3)
          const extra = dayEvents.length - shown.length
          return (
            <div key={i} className={`cal-month-cell ${dim ? 'dim' : ''} ${isToday(d) ? 'today' : ''} ${[0, 6].includes(d.getDay()) ? 'weekend' : ''}`}
              onClick={() => onDayClick(d)}>
              <div className="cal-month-cell-head">
                <span className={`cal-month-daynum ${isToday(d) ? 'today' : ''}`}>{d.getDate()}</span>
              </div>
              <div className="cal-month-events">
                {shown.map((ev) => (
                  <button key={ev.instanceKey} className={`cal-chip ${ev.card.allDay ? 'allday' : ''}`}
                    style={ev.card.allDay ? { background: ev.calendarColor, color: '#fff' } : undefined}
                    onClick={(e) => { e.stopPropagation(); onEventClick(ev) }} title={ev.card.title}>
                    {!ev.card.allDay && <span className="cal-chip-dot" style={{ background: ev.calendarColor }} />}
                    {!ev.card.allDay && <span className="cal-chip-time">{eventStart(ev).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })}</span>}
                    <span className="cal-chip-title">{ev.card.title || t('(untitled event)')}</span>
                    {ev.isRecurring && <span className="cal-chip-recur">⟳</span>}
                  </button>
                ))}
                {extra > 0 && (
                  <button className="cal-chip-more" onClick={(e) => { e.stopPropagation(); onMoreClick(d) }}>
                    {t('+{{n}} more', { n: extra })}
                  </button>
                )}
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}

// ─────────────────────────── Week / Day (time grid) ───────────────────────────

function TimeGridView({ days, cursor, events, locale, t, onSlotClick, onAllDayClick, onEventClick }) {
  const scrollRef = useRef(null)
  const startDay = days === 7 ? startOfWeek(cursor) : startOfDay(cursor)
  const cols = Array.from({ length: days }, (_, i) => addDays(startDay, i))
  const hours = Array.from({ length: 24 }, (_, i) => i)

  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 7 * HOUR_PX
  }, [days])

  const [nowTop, setNowTop] = useState(() => (new Date().getHours() + new Date().getMinutes() / 60) * HOUR_PX)
  useEffect(() => {
    const id = setInterval(() => {
      const n = new Date()
      setNowTop((n.getHours() + n.getMinutes() / 60) * HOUR_PX)
    }, 60000)
    return () => clearInterval(id)
  }, [])

  const allDayByCol = cols.map((d) => events.filter((ev) => ev.card.allDay && startOfDay(eventStart(ev)) <= startOfDay(d) && startOfDay(eventEnd(ev)) >= startOfDay(d)))
  const timedByCol = cols.map((d) => events.filter((ev) => !ev.card.allDay && sameDay(eventStart(ev), d)))
  const hasAllDay = allDayByCol.some((list) => list.length)

  return (
    <div className="cal-timegrid">
      <div className="cal-tg-header">
        <div className="cal-tg-gutter" />
        {cols.map((d, i) => (
          <div key={i} className={`cal-tg-colhead ${isToday(d) ? 'today' : ''}`}>
            <span className="cal-tg-dow">{d.toLocaleDateString(locale, { weekday: 'short' })}</span>
            <span className={`cal-tg-daynum ${isToday(d) ? 'today' : ''}`}>{d.getDate()}</span>
          </div>
        ))}
      </div>

      {hasAllDay && (
        <div className="cal-tg-allday">
          <div className="cal-tg-gutter cal-tg-allday-label">{t('All day')}</div>
          {cols.map((d, i) => (
            <div key={i} className="cal-tg-allday-col" onClick={() => onAllDayClick(d)}>
              {allDayByCol[i].map((ev) => (
                <button key={ev.instanceKey} className="cal-chip allday" style={{ background: ev.calendarColor, color: '#fff' }}
                  onClick={(e) => { e.stopPropagation(); onEventClick(ev) }} title={ev.card.title}>
                  <span className="cal-chip-title">{ev.card.title || t('(untitled event)')}</span>
                </button>
              ))}
            </div>
          ))}
        </div>
      )}

      <div className="cal-tg-scroll" ref={scrollRef}>
        <div className="cal-tg-body" style={{ height: 24 * HOUR_PX }}>
          <div className="cal-tg-hours">
            {hours.map((h) => (
              <div key={h} className="cal-tg-hour" style={{ height: HOUR_PX }}>
                <span className="cal-tg-hour-label">{formatHour(h, locale)}</span>
              </div>
            ))}
          </div>
          {cols.map((d, ci) => (
            <div key={ci} className={`cal-tg-col ${isToday(d) ? 'today' : ''}`}>
              {hours.map((h) => (
                <div key={h} className="cal-tg-slot" style={{ height: HOUR_PX }}
                  onClick={() => { const dt = new Date(d); dt.setHours(h, 0, 0, 0); onSlotClick(dt) }} />
              ))}
              {layoutColumns(timedByCol[ci]).map(({ ev, left, width }) => {
                const s = eventStart(ev); const e = eventEnd(ev)
                const top = (s.getHours() + s.getMinutes() / 60) * HOUR_PX
                const endH = e.getHours() + e.getMinutes() / 60
                const height = Math.max(0.5, (sameDay(s, e) ? endH : 24) - (s.getHours() + s.getMinutes() / 60)) * HOUR_PX
                return (
                  <button key={ev.instanceKey} className="cal-event"
                    style={{ top, height: Math.max(height, 18), left: `${left}%`, width: `calc(${width}% - 3px)`, background: ev.calendarColor }}
                    onClick={(e2) => { e2.stopPropagation(); onEventClick(ev) }} title={ev.card.title}>
                    <span className="cal-event-time">{s.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })}</span>
                    <span className="cal-event-title">{ev.card.title || t('(untitled event)')} {ev.isRecurring ? '⟳' : ''}</span>
                    {ev.card.location && <span className="cal-event-loc">{ev.card.location}</span>}
                  </button>
                )
              })}
              {isToday(d) && <div className="cal-now-line" style={{ top: nowTop }}><span className="cal-now-dot" /></div>}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

// Simple side-by-side layout for overlapping events within a single day column.
function layoutColumns(list) {
  const sorted = list.slice().sort((a, b) => a.startMs - b.startMs || b.endMs - a.endMs)
  const result = []
  let cluster = []
  let clusterEnd = -Infinity
  const flush = () => {
    const n = cluster.length
    cluster.forEach((ev, idx) => result.push({ ev, left: (idx / n) * 100, width: 100 / n }))
    cluster = []
  }
  for (const ev of sorted) {
    if (ev.startMs >= clusterEnd && cluster.length) flush()
    cluster.push(ev)
    clusterEnd = Math.max(clusterEnd, ev.endMs)
  }
  if (cluster.length) flush()
  return result
}

function formatHour(h, locale) {
  const d = new Date(2020, 0, 1, h, 0, 0)
  return d.toLocaleTimeString(locale, { hour: 'numeric' })
}

// ─────────────────────────── Agenda view ───────────────────────────

function AgendaView({ events, locale, t, onEventClick, onEmptyNew }) {
  const groups = useMemo(() => {
    const map = new Map()
    for (const ev of events.slice().sort((a, b) => a.startMs - b.startMs)) {
      const key = formatIsoLocalDate(eventStart(ev))
      if (!map.has(key)) map.set(key, [])
      map.get(key).push(ev)
    }
    return Array.from(map.entries()).sort(([a], [b]) => a.localeCompare(b))
  }, [events])

  if (!groups.length) {
    return (
      <div className="cal-agenda-empty">
        <div className="cal-agenda-empty-icon">📅</div>
        <p>{t('No events in this period.')}</p>
        <button className="cal-btn cal-btn--primary" onClick={onEmptyNew}>{t('New event')}</button>
      </div>
    )
  }

  return (
    <div className="cal-agenda">
      {groups.map(([key, list]) => {
        const d = parseIsoLocal(key)
        return (
          <div key={key} className="cal-agenda-group">
            <div className={`cal-agenda-date ${isToday(d) ? 'today' : ''}`}>
              <span className="cal-agenda-daynum">{d.getDate()}</span>
              <span className="cal-agenda-dow">{d.toLocaleDateString(locale, { weekday: 'long' })}</span>
              <span className="cal-agenda-mon">{d.toLocaleDateString(locale, { month: 'long', year: 'numeric' })}</span>
            </div>
            <div className="cal-agenda-list">
              {list.map((ev) => (
                <button key={ev.instanceKey} className="cal-agenda-item" onClick={() => onEventClick(ev)}>
                  <span className="cal-agenda-bar" style={{ background: ev.calendarColor }} />
                  <span className="cal-agenda-time">
                    {ev.card.allDay ? t('All day') : eventStart(ev).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })}
                  </span>
                  <span className="cal-agenda-title">{ev.card.title || t('(untitled event)')} {ev.isRecurring ? '⟳' : ''}</span>
                  {ev.card.location && <span className="cal-agenda-loc">{ev.card.location}</span>}
                </button>
              ))}
            </div>
          </div>
        )
      })}
    </div>
  )
}

// ─────────────────────────── Event detail popover ───────────────────────────

function EventDetail({ inst, locale, t, onClose, onEdit, onDelete, onEmail, onNewTask }) {
  const card = inst.card
  const s = eventStart(inst); const e = eventEnd(inst)
  const dateText = card.allDay
    ? (sameDay(s, e) ? s.toLocaleDateString(locale, { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' })
      : `${s.toLocaleDateString(locale, { day: 'numeric', month: 'short' })} – ${e.toLocaleDateString(locale, { day: 'numeric', month: 'short', year: 'numeric' })}`)
    : `${s.toLocaleDateString(locale, { weekday: 'long', day: 'numeric', month: 'long' })} · ${s.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })} – ${e.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })}`
  const attendees = (card.attendees || []).filter((a) => a.email || a.name)

  return (
    <div className="cal-modal-overlay" onClick={onClose}>
      <div className="cal-detail" onClick={(ev) => ev.stopPropagation()}>
        <div className="cal-detail-head" style={{ borderColor: inst.calendarColor }}>
          <span className="cal-detail-dot" style={{ background: inst.calendarColor }} />
          <h3>{card.title || t('(untitled event)')}</h3>
          <button className="cal-detail-close" onClick={onClose} title={t('Close')}>✕</button>
        </div>
        <div className="cal-detail-body">
          <div className="cal-detail-row"><span className="cal-detail-ic">🕑</span><span>{dateText}{card.allDay ? ` · ${t('All day')}` : ''}</span></div>
          {inst.isRecurring && <div className="cal-detail-row"><span className="cal-detail-ic">⟳</span><span>{t('Repeating event')}</span></div>}
          {inst.calendarName && <div className="cal-detail-row"><span className="cal-detail-ic">🗂</span><span>{inst.calendarName}</span></div>}
          {card.location && <div className="cal-detail-row"><span className="cal-detail-ic">📍</span><span>{card.location}</span></div>}
          {card.description && <div className="cal-detail-row"><span className="cal-detail-ic">📝</span><span className="cal-detail-desc">{card.description}</span></div>}
          {attendees.length > 0 && (
            <div className="cal-detail-row"><span className="cal-detail-ic">👥</span>
              <span className="cal-detail-attendees">
                {attendees.map((a, i) => <span key={i} className="cal-detail-att">{a.name || a.email}</span>)}
              </span>
            </div>
          )}
          {(card.reminders || []).length > 0 && (
            <div className="cal-detail-row"><span className="cal-detail-ic">⏰</span>
              <span>{card.reminders.map((m) => reminderLabel(m, t)).join(', ')}</span>
            </div>
          )}
        </div>
        <div className="cal-detail-actions">
          <button className="cal-btn cal-btn--primary" onClick={onEdit}>{t('Edit')}</button>
          {attendees.some((a) => a.email) && <button className="cal-btn" onClick={onEmail}>{t('Email attendees')}</button>}
          <button className="cal-btn" onClick={onNewTask}>{t('New task')}</button>
          <button className="cal-btn cal-btn--danger" onClick={onDelete}>{t('Delete')}</button>
        </div>
      </div>
    </div>
  )
}

// ─────────────────────────── Event editor ───────────────────────────

function splitDateTime(iso, allDay) {
  const d = parseIsoLocal(iso)
  if (!d) return { date: '', time: '09:00' }
  const date = formatIsoLocalDate(d)
  const time = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
  return { date, time: allDay ? '00:00' : time }
}

function EventEditor({ t, locale, draft, setDraft, saving, isNew, calendars, onCancel, onSave, onDelete }) {
  const set = (patch) => setDraft((d) => ({ ...d, ...patch }))
  const setRec = (patch) => setDraft((d) => ({ ...d, recurrence: { ...d.recurrence, ...patch } }))

  const start = splitDateTime(draft.start, draft.allDay)
  const end = splitDateTime(draft.end, draft.allDay)

  const applyStart = (date, time) => {
    if (draft.allDay) {
      set({ start: date, end: draft.end && parseIsoLocal(draft.end) >= parseIsoLocal(date) ? draft.end : date })
    } else {
      const startIso = `${date}T${time}`
      const sd = parseIsoLocal(startIso)
      let endIso = draft.end
      if (!parseIsoLocal(endIso) || parseIsoLocal(endIso) <= sd) endIso = formatIsoLocalDateTime(new Date(sd.getTime() + 3600000))
      set({ start: startIso, end: endIso })
    }
  }
  const applyEnd = (date, time) => {
    if (draft.allDay) set({ end: date })
    else set({ end: `${date}T${time}` })
  }
  const toggleAllDay = (checked) => {
    if (checked) {
      const sd = parseIsoLocal(draft.start) || new Date()
      const ed = parseIsoLocal(draft.end) || sd
      set({ allDay: true, start: formatIsoLocalDate(sd), end: formatIsoLocalDate(ed) })
    } else {
      const sd = parseIsoLocal(draft.start) || new Date()
      sd.setHours(9, 0, 0, 0)
      set({ allDay: false, start: formatIsoLocalDateTime(sd), end: formatIsoLocalDateTime(new Date(sd.getTime() + 3600000)) })
    }
  }

  const weekdays = Array.from({ length: 7 }, (_, i) => {
    const wd = (WEEK_START + i) % 7
    const d = new Date(2021, 7, 1 + ((wd - new Date(2021, 7, 1).getDay() + 7) % 7))
    return { wd, label: d.toLocaleDateString(locale, { weekday: 'narrow' }) }
  })

  const toggleWeekday = (wd) => {
    const cur = new Set(draft.recurrence.byWeekday || [])
    if (cur.has(wd)) cur.delete(wd); else cur.add(wd)
    setRec({ byWeekday: Array.from(cur).sort((a, b) => a - b) })
  }

  const endMode = draft.recurrence.count > 0 ? 'count' : (draft.recurrence.until ? 'until' : 'never')

  return (
    <div className="cal-modal-overlay" onClick={onCancel}>
      <div className="cal-editor" onClick={(e) => e.stopPropagation()}>
        <div className="cal-editor-topbar">
          <div className="cal-editor-title">{isNew ? t('New event') : t('Edit event')}</div>
          <div className="cal-editor-actions">
            <button className="cal-btn" onClick={onCancel}>{t('Cancel')}</button>
            <button className="cal-btn cal-btn--primary" onClick={onSave} disabled={saving}>{saving ? t('Saving…') : t('Save')}</button>
          </div>
        </div>

        <div className="cal-editor-body">
          <input className="cal-input cal-title-input" placeholder={t('Add a title')} value={draft.title} autoFocus
            onChange={(e) => set({ title: e.target.value })} />

          <div className="cal-form-row2">
            <label className="cal-labeled">
              <span>{t('Calendar')}</span>
              <select className="cal-select" value={draft.calendarId ?? ''} onChange={(e) => set({ calendarId: e.target.value ? Number(e.target.value) : null })}>
                {calendars.map((c) => <option key={c.calendarId} value={c.calendarId}>{c.name}</option>)}
              </select>
            </label>
            <label className="cal-labeled">
              <span>{t('Status')}</span>
              <select className="cal-select" value={draft.status || 'confirmed'} onChange={(e) => set({ status: e.target.value })}>
                <option value="confirmed">{t('Busy')}</option>
                <option value="tentative">{t('Tentative')}</option>
                <option value="cancelled">{t('Cancelled')}</option>
              </select>
            </label>
          </div>

          <label className="cal-check">
            <input type="checkbox" checked={draft.allDay} onChange={(e) => toggleAllDay(e.target.checked)} />
            <span>{t('All day')}</span>
          </label>

          <div className="cal-form-row2">
            <label className="cal-labeled">
              <span>{t('Starts')}</span>
              <div className="cal-datetime">
                <input className="cal-input" type="date" value={start.date} onChange={(e) => applyStart(e.target.value, start.time)} />
                {!draft.allDay && <input className="cal-input" type="time" value={start.time} onChange={(e) => applyStart(start.date, e.target.value)} />}
              </div>
            </label>
            <label className="cal-labeled">
              <span>{t('Ends')}</span>
              <div className="cal-datetime">
                <input className="cal-input" type="date" value={end.date} onChange={(e) => applyEnd(e.target.value, end.time)} />
                {!draft.allDay && <input className="cal-input" type="time" value={end.time} onChange={(e) => applyEnd(end.date, e.target.value)} />}
              </div>
            </label>
          </div>

          <label className="cal-labeled">
            <span>{t('Location')}</span>
            <input className="cal-input" placeholder={t('Add a location')} value={draft.location} onChange={(e) => set({ location: e.target.value })} />
          </label>

          {/* Recurrence */}
          <div className="cal-form-section">
            <div className="cal-form-section-title">{t('Repeat')}</div>
            <div className="cal-form-row2">
              <label className="cal-labeled">
                <span>{t('Frequency')}</span>
                <select className="cal-select" value={draft.recurrence.freq} onChange={(e) => setRec({ freq: e.target.value })}>
                  <option value="none">{t('Does not repeat')}</option>
                  <option value="daily">{t('Daily')}</option>
                  <option value="weekly">{t('Weekly')}</option>
                  <option value="monthly">{t('Monthly')}</option>
                  <option value="yearly">{t('Yearly')}</option>
                </select>
              </label>
              {draft.recurrence.freq !== 'none' && (
                <label className="cal-labeled">
                  <span>{t('Every')}</span>
                  <input className="cal-input" type="number" min="1" value={draft.recurrence.interval}
                    onChange={(e) => setRec({ interval: Math.max(1, Number(e.target.value) || 1) })} />
                </label>
              )}
            </div>

            {draft.recurrence.freq === 'weekly' && (
              <div className="cal-weekday-row">
                {weekdays.map(({ wd, label }) => (
                  <button key={wd} type="button" className={`cal-weekday ${draft.recurrence.byWeekday?.includes(wd) ? 'on' : ''}`} onClick={() => toggleWeekday(wd)}>{label}</button>
                ))}
              </div>
            )}

            {draft.recurrence.freq !== 'none' && (
              <div className="cal-form-row2">
                <label className="cal-labeled">
                  <span>{t('Ends')}</span>
                  <select className="cal-select" value={endMode}
                    onChange={(e) => {
                      const m = e.target.value
                      if (m === 'never') setRec({ until: '', count: 0 })
                      else if (m === 'until') setRec({ count: 0, until: formatIsoLocalDate(addMonths(parseIsoLocal(draft.start) || new Date(), 3)) })
                      else setRec({ until: '', count: 10 })
                    }}>
                    <option value="never">{t('Never')}</option>
                    <option value="until">{t('On date')}</option>
                    <option value="count">{t('After N times')}</option>
                  </select>
                </label>
                {endMode === 'until' && (
                  <label className="cal-labeled">
                    <span>{t('Until')}</span>
                    <input className="cal-input" type="date" value={draft.recurrence.until} onChange={(e) => setRec({ until: e.target.value })} />
                  </label>
                )}
                {endMode === 'count' && (
                  <label className="cal-labeled">
                    <span>{t('Occurrences')}</span>
                    <input className="cal-input" type="number" min="1" value={draft.recurrence.count} onChange={(e) => setRec({ count: Math.max(1, Number(e.target.value) || 1) })} />
                  </label>
                )}
              </div>
            )}
          </div>

          {/* Reminders */}
          <label className="cal-labeled">
            <span>{t('Reminder')}</span>
            <ReminderPicker reminders={draft.reminders} onChange={(r) => set({ reminders: r })} t={t} />
          </label>

          {/* Attendees */}
          <div className="cal-form-section">
            <div className="cal-form-section-title">{t('Attendees')}</div>
            <AttendeeEditor attendees={draft.attendees} onChange={(a) => set({ attendees: a })} t={t} />
          </div>

          <label className="cal-labeled">
            <span>{t('Description')}</span>
            <textarea className="cal-textarea" rows={4} placeholder={t('Add notes')} value={draft.description} onChange={(e) => set({ description: e.target.value })} />
          </label>
        </div>

        {onDelete && (
          <div className="cal-editor-footer">
            <button className="cal-btn cal-btn--danger" onClick={onDelete}>{t('Delete event')}</button>
          </div>
        )}
      </div>
    </div>
  )
}

function reminderLabel(m, t) {
  if (m === 0) return t('At time of event')
  if (m < 60) return t('{{n}} min before', { n: m })
  if (m < 1440) return t('{{n}} h before', { n: m / 60 })
  return t('{{n}} day before', { n: m / 1440 })
}

function ReminderPicker({ reminders, onChange, t }) {
  const active = new Set(reminders || [])
  return (
    <div className="cal-reminder-row">
      {REMINDER_OPTIONS.map((m) => (
        <button key={m} type="button" className={`cal-reminder-chip ${active.has(m) ? 'on' : ''}`}
          onClick={() => {
            const next = new Set(active)
            if (next.has(m)) next.delete(m); else next.add(m)
            onChange(Array.from(next).sort((a, b) => a - b))
          }}>
          {reminderLabel(m, t)}
        </button>
      ))}
    </div>
  )
}

function AttendeeEditor({ attendees, onChange, t }) {
  const [name, setName] = useState('')
  const [email, setEmail] = useState('')
  const add = () => {
    const e = email.trim()
    if (!e) return
    onChange([...(attendees || []), { name: name.trim(), email: e, status: '' }])
    setName(''); setEmail('')
  }
  return (
    <div className="cal-attendees">
      {(attendees || []).map((a, i) => (
        <div key={i} className="cal-attendee">
          <span className="cal-attendee-info">{a.name ? `${a.name} · ` : ''}{a.email}</span>
          <button type="button" className="cal-icon-btn" onClick={() => onChange(attendees.filter((_, j) => j !== i))}>✕</button>
        </div>
      ))}
      <div className="cal-attendee-add">
        <input className="cal-input" placeholder={t('Name')} value={name} onChange={(e) => setName(e.target.value)} />
        <input className="cal-input" placeholder={t('Email')} type="email" value={email} onChange={(e) => setEmail(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); add() } }} />
        <button type="button" className="cal-add-btn" onClick={add} disabled={!email.trim()}>＋</button>
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
    <input ref={ref} className="cal-name-input" value={value} placeholder={placeholder}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => { if (e.key === 'Enter') onSubmit(value); else if (e.key === 'Escape') onCancel() }}
      onBlur={() => onSubmit(value)} />
  )
}

function ColorDot({ color, onPick }) {
  const [open, setOpen] = useState(false)
  return (
    <span className="cal-colordot-wrap" onClick={(e) => e.preventDefault()}>
      <button type="button" className="cal-colordot" style={{ background: color }} onClick={(e) => { e.preventDefault(); setOpen((o) => !o) }} />
      {open && (
        <span className="cal-colorpop" onMouseLeave={() => setOpen(false)}>
          {CALENDAR_COLORS.map((c) => (
            <button key={c} type="button" className="cal-colorpop-swatch" style={{ background: c }}
              onClick={(e) => { e.preventDefault(); onPick(clampColor(c)); setOpen(false) }} />
          ))}
        </span>
      )}
    </span>
  )
}
