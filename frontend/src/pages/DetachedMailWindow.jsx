import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { apiUrl } from '../utils/api'
import { normalizeMailboxResponse } from '../utils/mailboxes'
import ExternalLinkPrompt from '../components/ExternalLinkPrompt.jsx'
import MailHeadersPanel from '../components/MailHeadersPanel.jsx'
import {
  copyTextToClipboard,
  getLinkClickBehavior,
  getUrlDomain,
  installIframeLinkInterceptor,
  openExternalUrl,
  sanitizeMailHtml,
  setDomainLinkBehavior,
  setLinkClickBehavior,
} from '../utils/externalLinks.js'
import {
  buildHeaderFallback,
  buildHeadersFileName,
  fullMessageFromRawBytes,
  mailHeadersKey,
} from '../utils/mailHeaders.js'
import './DashboardPage.css'

function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes === 0) return '0 B'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let index = 0
  let value = bytes
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024
    index += 1
  }
  return `${value.toFixed(1)} ${units[index]}`
}

function safeParse(json) {
  try {
    return JSON.parse(json)
  } catch {
    return null
  }
}

function isLabelMailbox(value) {
  return /^(Labels|Labels)(\/|$)/i.test((value || '').trim())
}

function isMoveTargetMailbox(value) {
  const mailbox = (value || '').trim()
  if (!mailbox || ['Folders', 'Labels', 'Labels'].includes(mailbox)) return false
  return !isLabelMailbox(mailbox)
}

function getMailboxNamespacePrefix(mailboxes, namespaceRoots) {
  const root = namespaceRoots.find((candidate) => (
    Array.isArray(mailboxes)
    && mailboxes.some((mailbox) => mailbox === candidate || mailbox.startsWith(`${candidate}/`))
  ))
  return root ? `${root}/` : ''
}

function applyMailboxNamespace(name, prefix) {
  const trimmed = (name || '').trim().replace(/^\/+|\/+$/g, '')
  if (!trimmed) return ''
  if (!prefix || trimmed.startsWith(prefix)) return trimmed
  return `${prefix}${trimmed}`
}

function getDetachedLabelHint() {
  try {
    const hint = typeof window !== 'undefined' ? window.__GUV_DETACHED__ : null
    return typeof hint?.label === 'string' ? hint.label : ''
  } catch {
    return ''
  }
}

export default function DetachedMailWindow({ initialLabel = '' } = {}) {
  const [windowLabel, setWindowLabel] = useState(() => initialLabel || getDetachedLabelHint())
  const [data, setData] = useState(null)
  const [mailContent, setMailContent] = useState(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [folders, setFolders] = useState([])
  const [isMoveMenuOpen, setIsMoveMenuOpen] = useState(false)
  const [movePopoverStyle, setMovePopoverStyle] = useState(null)
  const [headersPanel, setHeadersPanel] = useState({
    open: false,
    key: '',
    text: '',
    loading: false,
    error: '',
    copied: false,
  })
  const submenuScrollRef = useRef(null)
  const moveMenuRef = useRef(null)
  const iframeRef = useRef(null)
  const linkCleanupRef = useRef(null)

  const [externalLinkPromptUrl, setExternalLinkPromptUrl] = useState(null)

  const handleExternalLink = useCallback(async (url) => {
    const behavior = await getLinkClickBehavior(url)
    if (behavior === 'open') {
      await openExternalUrl(url)
      return
    }
    if (behavior === 'copy') {
      await copyTextToClipboard(url)
      return
    }
    setExternalLinkPromptUrl(url)
  }, [])

  const closeExternalLinkPrompt = useCallback(() => setExternalLinkPromptUrl(null), [])

  const onExternalLinkPromptSelect = useCallback(async (action, remember, rememberDomain) => {
    const url = externalLinkPromptUrl
    setExternalLinkPromptUrl(null)
    if (!url) return
    if (action === 'open') {
      await openExternalUrl(url)
    } else if (action === 'copy') {
      await copyTextToClipboard(url)
    }
    
    if (action === 'open' || action === 'copy') {
      if (remember) {
        await setLinkClickBehavior(action)
      } else if (rememberDomain) {
        const domain = getUrlDomain(url)
        if (domain) {
          await setDomainLinkBehavior(domain, action)
        }
      }
    }
  }, [externalLinkPromptUrl])


  // Resize helper: compute content height and set iframe height so the mail
  // doesn't collapse into a tiny band in detached windows.
  const resizeIframeToContent = useCallback((iframe) => {
    if (!iframe) return
    try {
      const doc = iframe.contentDocument
      if (!doc) return
      const body = doc.body
      const html = doc.documentElement
      const height = Math.max(
        1,
        Math.ceil(
          Math.max(
            html?.scrollHeight || 0,
            body?.scrollHeight || 0,
            html?.getBoundingClientRect?.().height || 0,
            body?.getBoundingClientRect?.().height || 0,
          ),
        ),
      )
      iframe.style.height = `${height}px`
    } catch {
      // ignore (sandbox / cross-origin)
    }
  }, [])

  const attachIframeImageListeners = useCallback((iframe) => {
    if (!iframe) return
    try {
      const doc = iframe.contentDocument
      const images = Array.from(doc?.images || [])
      images.forEach((img) => {
        if (!img) return
        if (img.complete && img.naturalWidth > 0) return
        const done = () => {
          img.removeEventListener('load', done)
          img.removeEventListener('error', done)
          resizeIframeToContent(iframe)
        }
        img.addEventListener('load', done, { once: true })
        img.addEventListener('error', done, { once: true })
      })
    } catch {
      // ignore
    }
  }, [resizeIframeToContent])

  const handleIframeLoad = useCallback(() => {
    const iframe = iframeRef.current
    if (!iframe) return
    resizeIframeToContent(iframe)
    attachIframeImageListeners(iframe)
    if (linkCleanupRef.current) linkCleanupRef.current()
    linkCleanupRef.current = installIframeLinkInterceptor(iframe, (href) => { handleExternalLink(href) })
    window.setTimeout(() => resizeIframeToContent(iframe), 50)
    window.setTimeout(() => resizeIframeToContent(iframe), 250)
  }, [resizeIframeToContent, attachIframeImageListeners, handleExternalLink])

  useEffect(() => {
    return () => {
      if (linkCleanupRef.current) {
        linkCleanupRef.current()
        linkCleanupRef.current = null
      }
    }
  }, [])

  const accountId = data?.accountId
  const mail = data?.mail
  const mailbox = data?.mailbox
  const preferOffline = !!data?.preferOffline
  const isImported = mail?.isImported === true

  const subject = useMemo(() => mailContent?.subject || mail?.subject || '(No Subject)', [mailContent, mail])
  const fromLine = useMemo(() => {
    if (!mail) return '-'
    if (mailContent?.from_name && mailContent?.from_address) {
      return `${mailContent.from_name} <${mailContent.from_address}>`
    }
    return mail?.address || mail?.name || '-'
  }, [mail, mailContent])
  const readToggleLabel = mail?.seen === true ? 'Unread' : 'Read'
  const moveFolderOptions = useMemo(
    () => folders.filter(isMoveTargetMailbox),
    [folders],
  )

  const patchMail = (patch) => {
    setData((prev) => (
      prev?.mail ? { ...prev, mail: { ...prev.mail, ...patch } } : prev
    ))
  }

  const syncPopoverPosition = useCallback((menuRef, setStyle) => {
    const node = menuRef.current
    if (!node) {
      setStyle(null)
      return
    }

    const rect = node.getBoundingClientRect()
    const estimatedWidth = 220
    const left = Math.min(
      Math.max(12, rect.left),
      Math.max(12, window.innerWidth - estimatedWidth - 12),
    )

    setStyle({
      left: `${left}px`,
      top: `${rect.bottom + 6}px`,
    })
  }, [])

  const queueAction = async (actionType, payload = {}) => {
    if (!accountId || !mail?.id) return
    await fetch(apiUrl(`/api/offline/${accountId}/actions`), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        action_type: actionType,
        target_uid: mail.id,
        target_folder: mailbox || 'INBOX',
        payload,
      }),
    })
  }

  useEffect(() => {
    document.body.style.padding = '0'
    document.body.style.margin = '0'
    document.body.style.overflow = 'hidden'
    return () => {
      document.body.style.padding = ''
      document.body.style.margin = ''
      document.body.style.overflow = ''
    }
  }, [])

  useEffect(() => {
    if (windowLabel) return () => {}
    let active = true
    const detectLabel = async () => {
      try {
        const { getCurrentWebviewWindow } = await import('@tauri-apps/api/webviewWindow')
        const label = getCurrentWebviewWindow().label
        if (!active) return
        setWindowLabel(label)
      } catch {
        // Try the next raw source.
      }
    }
    detectLabel()
    return () => { active = false }
  }, [windowLabel])

  useEffect(() => {
    if (!windowLabel) return
    let active = true
    const fetchData = async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        const json = await invoke('get_mail_window_data', { label: windowLabel })
        if (!active) return
        if (json) {
          const parsed = safeParse(json)
          setData(parsed)
          setMailContent(parsed?.mailContent || null)
        }
      } catch {
        // ignore errors while fetching window data
      }
    }
    fetchData()
    return () => { active = false }
  }, [windowLabel])

  useEffect(() => {
    if (!mail || !accountId || mailContent) return
    let active = true
    const timeoutId = setTimeout(() => {
      if (!active) return
      setError('Content loading timed out.')
      setLoading(false)
    }, 20000)
    setLoading(true)
    setError('')
    const mailboxParam = mailbox ? `?mailbox=${encodeURIComponent(mailbox)}` : ''
    const fetchContent = async () => {
      const primaryPath = preferOffline
        ? `/api/offline/${accountId}/local-content/${mail.id}${mailboxParam}`
        : `/api/mail/${accountId}/content/${mail.id}${mailboxParam}`
      const fallbackPath = preferOffline
        ? `/api/mail/${accountId}/content/${mail.id}${mailboxParam}`
        : `/api/offline/${accountId}/local-content/${mail.id}${mailboxParam}`

      const loadFromPath = async (path) => {
        const res = await fetch(apiUrl(path), { cache: 'no-store' })
        if (!res.ok) {
          let message = 'Content could not be loaded'
          try {
            const body = await res.json()
            if (typeof body?.error === 'string' && body.error.trim()) {
              message = body.error
            }
          } catch {
            // ignore JSON parse errors from error bodies
          }
          throw new Error(message)
        }
        return res.json()
      }

      try {
        return await loadFromPath(primaryPath)
      } catch (primaryError) {
        if (primaryPath === fallbackPath) {
          throw primaryError
        }
        return loadFromPath(fallbackPath)
      }
    }

    fetchContent()
      .then((json) => {
        if (!active) return
        clearTimeout(timeoutId)
        setMailContent(json)
        setLoading(false)
      })
      .catch((err) => {
        if (!active) return
        clearTimeout(timeoutId)
        setError(err?.message || 'Unknown error')
        setLoading(false)
      })
    return () => {
      active = false
      clearTimeout(timeoutId)
    }
  }, [accountId, mail, mailContent, mailbox, preferOffline])

  useEffect(() => {
    if (!accountId) return
    let active = true
    const loadFolders = async () => {
      try {
        let res = await fetch(apiUrl(`/api/offline/${accountId}/local-mailboxes`), { cache: 'no-store' })
        if (res.ok && active) {
          const json = await res.json()
          const normalized = normalizeMailboxResponse(json)
          setFolders(normalized.allMailboxes)
        }
        res = await fetch(apiUrl(`/api/mail/${accountId}/mailboxes`), { cache: 'no-store' })
        if (res.ok && active) {
          const json = await res.json()
          const normalized = normalizeMailboxResponse(json)
          setFolders(normalized.allMailboxes)
        }
      } catch {
        // ignore folder loading errors
      }
    }
    loadFolders()
    return () => { active = false }
  }, [accountId])

  useEffect(() => {
    if (!mail?.id || mail.seen === true || loading) return
    queueAction('mark_read').then(() => patchMail({ seen: true })).catch(() => {})
  }, [loading, mail?.id, mail?.seen])

  useEffect(() => {
    if (!isMoveMenuOpen) {
      setMovePopoverStyle(null)
      return
    }

    const sync = () => {
      syncPopoverPosition(moveMenuRef, setMovePopoverStyle)
    }

    const frameId = window.requestAnimationFrame(sync)
    const scrollNode = submenuScrollRef.current

    window.addEventListener('resize', sync)
    window.addEventListener('scroll', sync, true)
    scrollNode?.addEventListener('scroll', sync)

    return () => {
      window.cancelAnimationFrame(frameId)
      window.removeEventListener('resize', sync)
      window.removeEventListener('scroll', sync, true)
      scrollNode?.removeEventListener('scroll', sync)
    }
  }, [isMoveMenuOpen, syncPopoverPosition])

  const closeWindow = async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      if (!windowLabel) {
        window.close()
        return
      }
      await invoke('close_mail_window', { label: windowLabel })
    } catch {
      window.close()
    }
  }

  const createMailbox = async (name) => {
    const mailboxName = (name || '').trim()
    if (!accountId || !mailboxName) return false
    try {
      const res = await fetch(apiUrl(`/api/mail/${accountId}/mailboxes`), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: mailboxName }),
      })
      if (!res.ok) return false
      setFolders((prev) => (prev.includes(mailboxName) ? prev : [...prev, mailboxName]))
      return true
    } catch {
      return false
    }
  }

  const openMailto = (params) => {
    const search = new URLSearchParams()
    Object.entries(params).forEach(([key, value]) => {
      if (value) search.set(key, value)
    })
    window.location.href = `mailto:${params.to || ''}?${search.toString()}`
  }

  const triggerBrowserDownload = useCallback((fileName, mimeType, bytes) => {
    const blob = new Blob([bytes], { type: mimeType || 'application/octet-stream' })
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = fileName || 'download'
    document.body.appendChild(link)
    link.click()
    link.remove()
    window.setTimeout(() => URL.revokeObjectURL(url), 250)
  }, [])

  const fetchMailRawBytes = useCallback(async () => {
    if (!accountId || !mail?.id || isImported) return null
    const safeMailbox = mailbox || 'INBOX'
    const candidates = preferOffline
      ? [
        `/api/offline/${accountId}/local-raw/${encodeURIComponent(mail.id)}?mailbox=${encodeURIComponent(safeMailbox)}`,
        `/api/mail/${accountId}/raw/${encodeURIComponent(mail.id)}?mailbox=${encodeURIComponent(safeMailbox)}`,
      ]
      : [
        `/api/mail/${accountId}/raw/${encodeURIComponent(mail.id)}?mailbox=${encodeURIComponent(safeMailbox)}`,
        `/api/offline/${accountId}/local-raw/${encodeURIComponent(mail.id)}?mailbox=${encodeURIComponent(safeMailbox)}`,
      ]

    for (const endpoint of candidates) {
      try {
        const res = await fetch(apiUrl(endpoint), { cache: 'no-store' })
        if (!res.ok) continue
        return new Uint8Array(await res.arrayBuffer())
      } catch {
      // ignore transient network errors while attempting to fetch raw bytes
      }
    }
    return null
  }, [accountId, isImported, mail?.id, mailbox, preferOffline])

  const openHeadersPanel = useCallback(async () => {
    if (!mail) return
    const safeMailbox = mailbox || 'INBOX'
    const key = mailHeadersKey(mail, safeMailbox)
    setHeadersPanel({
      open: true,
      key,
      text: '',
      loading: true,
      error: '',
      copied: false,
    })

    try {
      const rawBytes = await fetchMailRawBytes()
      const rawMessage = fullMessageFromRawBytes(rawBytes)
      const text = rawMessage || buildHeaderFallback(mail, mailContent, (value) => value || '')
      setHeadersPanel((prev) => (
        prev.key === key ? { ...prev, text, loading: false, error: '' } : prev
      ))
    } catch (error) {
      const fallback = buildHeaderFallback(mail, mailContent, (value) => value || '')
      setHeadersPanel((prev) => (
        prev.key === key
          ? { ...prev, text: fallback, loading: false, error: fallback ? '' : (error?.message || 'Message source could not be loaded.') }
          : prev
      ))
    }
  }, [fetchMailRawBytes, mail, mailContent, mailbox])

  const closeHeadersPanel = useCallback(() => {
    setHeadersPanel((prev) => ({ ...prev, open: false, copied: false }))
  }, [])

  const copyHeadersPanelText = useCallback(async () => {
    if (!headersPanel.text) return
    await copyTextToClipboard(headersPanel.text)
    setHeadersPanel((prev) => ({ ...prev, copied: true }))
    window.setTimeout(() => {
      setHeadersPanel((prev) => ({ ...prev, copied: false }))
    }, 1400)
  }, [headersPanel.text])

  const downloadHeadersPanelText = useCallback(() => {
    if (!headersPanel.text) return
    const binary = new TextEncoder().encode(headersPanel.text)
    triggerBrowserDownload(
      buildHeadersFileName(mail, mailContent),
      'text/plain;charset=utf-8',
      binary,
    )
  }, [headersPanel.text, mail, mailContent, triggerBrowserDownload])

  const downloadAttachmentFromBase64 = useCallback((attachment) => {
    const base64 = attachment?.data_base64
    if (!base64) return
    const fileName = attachment?.filename || 'attachment'
    const mimeType = attachment?.content_type || 'application/octet-stream'
    try {
      const binary = atob(base64)
      const bytes = new Uint8Array(binary.length)
      for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i)
      triggerBrowserDownload(fileName, mimeType, bytes)
    } catch (error) {
      console.error('Failed to download base64 attachment:', error)
    }
  }, [triggerBrowserDownload])

  const attachmentHref = useCallback((uid, attachmentId) => {
    if (!accountId) return '#'
    const safeMailbox = mailbox || 'INBOX'
    const path = preferOffline
      ? `/api/offline/${accountId}/local-content/${encodeURIComponent(uid)}/attachments/${attachmentId}`
      : `/api/mail/${accountId}/content/${encodeURIComponent(uid)}/attachments/${attachmentId}`
    return apiUrl(`${path}?mailbox=${encodeURIComponent(safeMailbox)}`)
  }, [accountId, mailbox, preferOffline])

  const buildQuotedBody = () => {
    const body = mailContent?.plain_body || '(No content)'
    return [
      '',
      '',
      `From: ${fromLine}`,
      `Subject: ${subject}`,
      '',
      body,
    ].join('\n')
  }

  const handleDelete = async () => {
    if (isImported) return
    await queueAction('delete')
    closeWindow()
  }

  const handleMove = async (destination) => {
    if (isImported) return
    await queueAction('move', { destination })
    closeWindow()
  }

  const handleReply = () => {
    if (isImported) return
    openMailto({
      to: mail?.address || '',
      subject: `Re: ${subject}`,
      body: buildQuotedBody(),
    })
  }

  const handleForward = () => {
    if (isImported) return
    openMailto({
      subject: `Fwd: ${subject}`,
      body: buildQuotedBody(),
    })
  }


  const handleApplyLabels = async () => {
    if (isImported) return
    const current = Array.isArray(mail?.labels) ? mail.labels : []
    const input = window.prompt('Labels (comma separated)', current.join(', '))
    if (!input) return
    const labels = input.split(',').map((s) => s.trim()).filter(Boolean)
    if (labels.length === 0) return
    try {
      await queueAction('label', { labels })
      patchMail({ labels: Array.from(new Set([...(current || []), ...labels])) })
    } catch {
      // ignore
    }
  }

  const handleReadToggle = async () => {
    if (isImported) return
    const nextSeen = mail?.seen !== true
    await queueAction(nextSeen ? 'mark_read' : 'mark_unread')
    patchMail({ seen: nextSeen })
  }

  const handleCreateFolderAndMove = async () => {
    const name = window.prompt('New folder name')
    if (!name) return
    const mailboxName = applyMailboxNamespace(name, getMailboxNamespacePrefix(folders, ['Folders']))
    const created = await createMailbox(mailboxName)
    if (created) {
      await handleMove(mailboxName)
    }
  }

  if (!mail || !accountId) {
    return (
      <div className="startup-router">
        <p>Email data not found for this window.</p>
        <div className="startup-router__actions">
          <button type="button" onClick={closeWindow}>Close</button>
        </div>
      </div>
    )
  }

  return (
    <div className="dashboard-page" style={{ height: '100vh' }}>
      <ExternalLinkPrompt
        open={!!externalLinkPromptUrl}
        url={externalLinkPromptUrl || ''}
        onCancel={closeExternalLinkPrompt}
        onSelect={onExternalLinkPromptSelect}
      />
      <div className="db-navbar">
        <button className="db-logo-btn" style={{ padding: 0, height: '40px', background: 'transparent', minWidth: '140px', border: 'none' }}>
          <img src="/img/logo/guvercin-righttext-nobackground.svg" alt="Guvercin" style={{ height: '100%', width: 'auto', display: 'block' }} />
        </button>
        <div style={{ marginLeft: 'auto', display: 'flex', gap: 8 }}>
          <button className="db-icon-btn" title="Close" onClick={closeWindow}><img src="/img/icons/close.svg" className="svg-icon-inline" alt="close"/></button>
        </div>
      </div>

      <div className="db-section-area">
        <div className="db-right-panel" style={{ flex: 1 }}>
          <div className="db-submenu db-submenu--icon_text_small">
            <div className="db-submenu-scroll" ref={submenuScrollRef}>
            <ul>
              <li>
                <button className="db-submenu-main-btn" type="button" disabled={isImported} onClick={handleDelete}>
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/recycle-bin.svg" className="svg-icon-inline" alt="Delete"/></span>
                  <span className="db-submenu-main-btn__text">Delete</span>
                </button>
              </li>
              <li>
                <button className="db-submenu-main-btn" type="button" disabled={isImported} onClick={() => handleMove('Trash')}>
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/move-to-folder.svg" className="svg-icon-inline" alt="Move to Trash"/></span>
                  <span className="db-submenu-main-btn__text">Move to Trash</span>
                </button>
              </li>
              <li>
                <button className="db-submenu-main-btn" type="button" disabled={isImported} onClick={() => handleMove('Archive')}>
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/archive.svg" className="svg-icon-inline" alt="Archive"/></span>
                  <span className="db-submenu-main-btn__text">Archive</span>
                </button>
              </li>
              <li>
                <button className="db-submenu-main-btn" type="button" disabled={isImported} onClick={handleReply}>
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/reply.svg" className="svg-icon-inline" alt="Reply"/></span>
                  <span className="db-submenu-main-btn__text">Reply</span>
                </button>
              </li>
              <li>
                <button className="db-submenu-main-btn" type="button" disabled={isImported} onClick={handleForward}>
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/forward.svg" className="svg-icon-inline" alt="Forward"/></span>
                  <span className="db-submenu-main-btn__text">Forward</span>
                </button>
              </li>

              <li className="db-submenu-menu-wrap" ref={moveMenuRef}>
                <button
                  type="button"
                  disabled={isImported}
                  className={`db-submenu-main-btn ${isMoveMenuOpen ? 'submenu-open' : ''}`.trim()}
                  onClick={() => setIsMoveMenuOpen((prev) => !prev)}
                >
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/folder.svg" className="svg-icon-inline" alt="Move"/></span>
                  <span className="db-submenu-main-btn__text">Move</span>
                </button>
                {isMoveMenuOpen && (
                  <div 
                    className="db-submenu-popover" 
                    style={movePopoverStyle || undefined}
                    onWheel={(e) => e.stopPropagation()}
                  >
                    {moveFolderOptions.map((folder) => (
                      <button key={folder} type="button" className="db-submenu-popover__item" onClick={() => handleMove(folder)}>
                        {folder}
                      </button>
                    ))}
                    <div className="db-submenu-popover__divider" />
                    <button type="button" className="db-submenu-popover__item" onClick={handleCreateFolderAndMove}>
                      + New Folder
                    </button>
                  </div>
                )}
              </li>

              <li>
                <button className="db-submenu-main-btn" type="button" disabled={isImported} onClick={handleApplyLabels}>
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/label.svg" className="svg-icon-inline" alt="Labels"/></span>
                  <span className="db-submenu-main-btn__text">Labels</span>
                </button>
              </li>

              <li>
                <button className="db-submenu-main-btn" type="button" disabled={isImported} onClick={handleReadToggle}>
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/read.svg" className="svg-icon-inline" alt="Read/Unread"/></span>
                  <span className="db-submenu-main-btn__text">{readToggleLabel}</span>
                </button>
              </li>

              <li>
                <button className="db-submenu-main-btn" type="button" onClick={openHeadersPanel}>
                  <span className="db-submenu-main-btn__icon"><img src="/img/icons/mail.svg" className="svg-icon-inline" alt="Source"/></span>
                  <span className="db-submenu-main-btn__text">Source</span>
                </button>
              </li>
            </ul>
            </div>
          </div>
          {loading ? (
            <div className="db-loading" style={{ paddingTop: 60 }}>
              <div className="db-spinner" />
              Loading content...
            </div>
          ) : error ? (
            <div className="db-empty-state">
              <div className="db-empty-icon"></div>
              <div className="db-empty-text">{error}</div>
            </div>
          ) : (
            <div className="db-mail-content">
              <div className="db-mail-content-header">
                <div className="db-mail-content-subject">{subject}</div>
              </div>
              {headersPanel.open && headersPanel.key === mailHeadersKey(mail, mailbox || 'INBOX') && (
                <MailHeadersPanel
                  text={headersPanel.text}
                  loading={headersPanel.loading}
                  error={headersPanel.error}
                  copied={headersPanel.copied}
                  onCopy={copyHeadersPanelText}
                  onDownload={downloadHeadersPanelText}
                  onClose={closeHeadersPanel}
                />
              )}
              <div className="db-mail-meta"><strong>From:</strong> {fromLine}</div>
              <hr className="db-mail-divider" />
              {mailContent?.html_body ? (
                <div className="db-mail-body-html">
                  <iframe
                    key={mail?.id}
                    ref={iframeRef}
                    title="mail-content"
                    sandbox="allow-same-origin allow-scripts"
                    srcDoc={sanitizeMailHtml(mailContent.html_body)}
                    onLoad={handleIframeLoad}
                  />
                </div>
              ) : (
                <div className="db-mail-body">{mailContent?.plain_body || '(No content)'}</div>
              )}
              {mailContent?.attachments?.length > 0 && (
                <div className="db-attachments">
                  <div className="db-attachments__header">Attachments ({mailContent.attachments.length})</div>
                  <ul className="db-attachments__list">
                    {mailContent.attachments.map((at) => (
                      <li key={at.id} className="db-attachments__item">
                        <div className="db-attachments__info">
                          <span className="db-attachments__name">{at.filename}</span>
                          <span className="db-attachments__meta">{at.content_type} · {formatBytes(at.size)}</span>
                        </div>
                        {isImported || at.data_base64 ? (
                          <button
                            type="button"
                            className="db-attachments__link"
                            onClick={() => downloadAttachmentFromBase64(at)}
                          >
                            Download
                          </button>
                        ) : (
                          <a
                            className="db-attachments__link"
                            href={attachmentHref(mailContent.id, at.id)}
                            download={at.filename}
                          >
                            Download
                          </a>
                        )}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
