import { apiUrl } from '../utils/api'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import './AccountSelectionPage.css'
import { hydrateAccountSession } from '../utils/accountStorage.js'
import LanguageSelector from '../components/LanguageSelector.jsx'

// Default main-window size (matches src-tauri/tauri.conf.json) restored once
// the user leaves the account-selection screen.
const MAIN_WINDOW_WIDTH = 1000
const MAIN_WINDOW_HEIGHT = 700

// Fixed size for the account-selection screen (matches the app's normal
// minimum window size in src-tauri/tauri.conf.json).
const SELECTION_WINDOW_WIDTH = 850
const SELECTION_WINDOW_HEIGHT = 600

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

// Shrinks and centers the native window to a fixed size and locks resizing,
// so this screen stays at exactly its minimum size instead of full-screen
// or user-resizable. Scoped entirely to this screen — restoreMainWindow()
// below puts the main window's own size/resizable settings back once the
// user leaves, so this never leaks into the dashboard/login window.
async function setSelectionWindowSize() {
  if (!isTauri()) return
  try {
    const { getCurrentWindow, LogicalSize } = await import('@tauri-apps/api/window')
    const win = getCurrentWindow()
    await win.unmaximize()
    await win.setSize(new LogicalSize(SELECTION_WINDOW_WIDTH, SELECTION_WINDOW_HEIGHT))
    await win.center()
    await win.setResizable(false)
  } catch {
    // Not inside Tauri or the API is unavailable; keep current window size.
  }
}

// Restores the normal main-window size and resizability when navigating
// away to login/dashboard.
async function restoreMainWindow() {
  if (!isTauri()) return
  try {
    const { getCurrentWindow, LogicalSize } = await import('@tauri-apps/api/window')
    const win = getCurrentWindow()
    await win.setResizable(true)
    await win.setSize(new LogicalSize(MAIN_WINDOW_WIDTH, MAIN_WINDOW_HEIGHT))
    await win.center()
  } catch {
    // Not inside Tauri or the API is unavailable.
  }
}

function AccountSelectionPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [accounts, setAccounts] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    setSelectionWindowSize()
    return () => {
      restoreMainWindow()
    }
  }, [])

  useEffect(() => {
    let active = true

    const loadAccounts = async () => {
      setLoading(true)
      try {
        const response = await fetch(apiUrl('/api/auth/accounts'))
        if (!response.ok) {
          throw new Error('Unable to fetch accounts')
        }
        const data = await response.json()
        if (!active) {
          return
        }
        setAccounts(Array.isArray(data.accounts) ? data.accounts : [])
      } catch {
        if (!active) {
          return
        }
        setError(t('Unable to load accounts. Please try again.'))
      } finally {
        if (active) {
          setLoading(false)
        }
      }
    }

    loadAccounts()
    return () => {
      active = false
    }
  }, [t])

  const handleSelect = async (account) => {
    hydrateAccountSession(account)
    navigate('/dashboard')
  }

  const statusContent = () => {
    if (loading) {
      return <p className="status">{t('Loading accounts...')}</p>
    }

    if (error) {
      return (
        <div className="status status-error">
          <p>{error}</p>
          <div className="status-actions">
            <button type="button" onClick={() => window.location.reload()}>
              {t('Try again')}
            </button>
            <button type="button" onClick={() => navigate('/login')}>
              {t('Back to Login')}
            </button>
          </div>
        </div>
      )
    }

    if (!accounts.length) {
      return (
        <div className="status status-empty">
          <p>{t('No accounts found yet. Create one to get started.')}</p>
          <button type="button" onClick={() => navigate('/login')}>
            {t('Add new account')}
          </button>
        </div>
      )
    }

    return (
      <>
        <div className="accounts-grid">
          {accounts.map((account) => (
            <button
              key={account.account_id}
              type="button"
              className="account-card"
              onClick={() => handleSelect(account)}
            >
              <div className="account-card__title">
                {account.display_name || account.email_address}
              </div>
              <div className="account-card__email">{account.email_address}</div>
              <div className="account-card__meta">
                {account.provider_type || 'IMAP'}
                {account.language ? ` • ${account.language}` : ''}
              </div>
            </button>
          ))}
        </div>
        <div className="selection-panel__footer">
          <button type="button" className="ghost-btn" onClick={() => navigate('/login')}>
            {t('Add new account')}
          </button>
        </div>
      </>
    )
  }

  return (
    <div className="account-selection-page">
      <div className="selection-panel">
        <LanguageSelector />
        <div className="heading">
          <p className="eyebrow">{t('Registered Accounts')}</p>
          <h1>{t('Account Selection')}</h1>
          <p className="subtitle">{t('Select an account to continue')}</p>
        </div>
        {statusContent()}
      </div>
    </div>
  )
}

export default AccountSelectionPage
