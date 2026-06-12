import { apiUrl } from '../utils/api'
import { useState, useEffect, useCallback, useRef } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { hydrateAccountSession } from '../utils/accountStorage.js'
import LanguageSelector from '../components/LanguageSelector.jsx'
import './LoginPage.css'

const DEFAULT_FORM_DATA = {
    email: '',
    displayName: '',
    imapServer: '',
    imapPort: '',
    smtpServer: '',
    smtpPort: '',
    password: '',
    sslMode: 'STARTTLS',
}

const AUTOCONFIG_PROVIDERS = [
    {
        label: 'Proton Mail Bridge',
        domains: ['proton.me', 'protonmail.com', 'pm.me', 'protonmail.ch'],
        configs: [
            {
                label: 'Proton Bridge recommended',
                imapServer: '127.0.0.1',
                imapPort: '1143',
                smtpServer: '127.0.0.1',
                smtpPort: '1025',
                sslMode: 'STARTTLS',
                source: 'Proton Mail Bridge defaults',
            },
        ],
    },
    {
        label: 'Gmail',
        domains: ['gmail.com', 'googlemail.com'],
        configs: [
            {
                label: 'Gmail recommended',
                imapServer: 'imap.gmail.com',
                imapPort: '993',
                smtpServer: 'smtp.gmail.com',
                smtpPort: '587',
                sslMode: 'SSL',
                source: 'Google defaults',
            },
            {
                label: 'Gmail alternate',
                imapServer: 'imap.gmail.com',
                imapPort: '993',
                smtpServer: 'smtp.gmail.com',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'Google alternate SMTP port',
            },
        ],
    },
    {
        label: 'Microsoft 365 / Outlook',
        domains: ['outlook.com', 'hotmail.com', 'live.com', 'msn.com', 'office365.com'],
        configs: [
            {
                label: 'Microsoft recommended',
                imapServer: 'outlook.office365.com',
                imapPort: '993',
                smtpServer: 'smtp.office365.com',
                smtpPort: '587',
                sslMode: 'SSL',
                source: 'Microsoft defaults',
            },
        ],
    },
    {
        label: 'iCloud Mail',
        domains: ['icloud.com', 'me.com', 'mac.com'],
        configs: [
            {
                label: 'iCloud recommended',
                imapServer: 'imap.mail.me.com',
                imapPort: '993',
                smtpServer: 'smtp.mail.me.com',
                smtpPort: '587',
                sslMode: 'SSL',
                source: 'Apple defaults',
            },
        ],
    },
    {
        label: 'Yahoo Mail',
        domains: ['yahoo.com', 'ymail.com', 'rocketmail.com'],
        configs: [
            {
                label: 'Yahoo recommended',
                imapServer: 'imap.mail.yahoo.com',
                imapPort: '993',
                smtpServer: 'smtp.mail.yahoo.com',
                smtpPort: '587',
                sslMode: 'SSL',
                source: 'Yahoo defaults',
            },
        ],
    },
    {
        label: 'AOL Mail',
        domains: ['aol.com'],
        configs: [
            {
                label: 'AOL recommended',
                imapServer: 'imap.aol.com',
                imapPort: '993',
                smtpServer: 'smtp.aol.com',
                smtpPort: '587',
                sslMode: 'SSL',
                source: 'AOL defaults',
            },
        ],
    },
    {
        label: 'GMX',
        domains: ['gmx.com', 'gmx.de', 'gmx.net'],
        configs: [
            {
                label: 'GMX recommended',
                imapServer: 'imap.gmx.com',
                imapPort: '993',
                smtpServer: 'mail.gmx.com',
                smtpPort: '587',
                sslMode: 'SSL',
                source: 'GMX defaults',
            },
        ],
    },
    {
        label: 'WEB.DE',
        domains: ['web.de'],
        configs: [
            {
                label: 'WEB.DE recommended',
                imapServer: 'imap.web.de',
                imapPort: '993',
                smtpServer: 'smtp.web.de',
                smtpPort: '587',
                sslMode: 'SSL',
                source: 'WEB.DE defaults',
            },
        ],
    },
    {
        label: 'Mail.ru',
        domains: ['mail.ru', 'bk.ru', 'inbox.ru', 'list.ru'],
        configs: [
            {
                label: 'Mail.ru recommended',
                imapServer: 'imap.mail.ru',
                imapPort: '993',
                smtpServer: 'smtp.mail.ru',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'Mail.ru defaults',
            },
        ],
    },
    {
        label: 'Yandex',
        domains: ['yandex.ru', 'yandex.com', 'yandex.kz', 'yandex.ua'],
        configs: [
            {
                label: 'Yandex recommended',
                imapServer: 'imap.yandex.com',
                imapPort: '993',
                smtpServer: 'smtp.yandex.com',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'Yandex defaults',
            },
        ],
    },
    {
        label: 'Zoho Mail',
        domains: ['zoho.com', 'zohomail.com', 'zohomail.eu'],
        configs: [
            {
                label: 'Zoho recommended',
                imapServer: 'imap.zoho.com',
                imapPort: '993',
                smtpServer: 'smtp.zoho.com',
                smtpPort: '587',
                sslMode: 'SSL',
                source: 'Zoho defaults',
            },
        ],
    },
    {
        label: 'Fastmail',
        domains: ['fastmail.com'],
        configs: [
            {
                label: 'Fastmail recommended',
                imapServer: 'imap.fastmail.com',
                imapPort: '993',
                smtpServer: 'smtp.fastmail.com',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'Fastmail defaults',
            },
        ],
    },
    {
        label: 'IONOS',
        domains: ['ionos.com', '1and1.com', '1and1.co.uk'],
        configs: [
            {
                label: 'IONOS recommended',
                imapServer: 'imap.ionos.com',
                imapPort: '993',
                smtpServer: 'smtp.ionos.com',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'IONOS defaults',
            },
        ],
    },
    {
        label: 'Namecheap Private Email',
        domains: ['privateemail.com'],
        configs: [
            {
                label: 'Private Email recommended',
                imapServer: 'mail.privateemail.com',
                imapPort: '993',
                smtpServer: 'mail.privateemail.com',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'Namecheap defaults',
            },
        ],
    },
    {
        label: 'Rackspace Email',
        domains: ['emailsrvr.com', 'rackspace.com'],
        configs: [
            {
                label: 'Rackspace recommended',
                imapServer: 'secure.emailsrvr.com',
                imapPort: '993',
                smtpServer: 'secure.emailsrvr.com',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'Rackspace defaults',
            },
        ],
    },
    {
        label: 'Orange Mail',
        domains: ['orange.fr', 'wanadoo.fr'],
        configs: [
            {
                label: 'Orange recommended',
                imapServer: 'imap.orange.fr',
                imapPort: '993',
                smtpServer: 'smtp.orange.fr',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'Orange defaults',
            },
        ],
    },
    {
        label: 'SFR Mail',
        domains: ['sfr.fr'],
        configs: [
            {
                label: 'SFR recommended',
                imapServer: 'imap.sfr.fr',
                imapPort: '993',
                smtpServer: 'smtp.sfr.fr',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'SFR defaults',
            },
        ],
    },
    {
        label: 'KPN',
        domains: ['kpnmail.nl'],
        configs: [
            {
                label: 'KPN recommended',
                imapServer: 'imap.kpnmail.nl',
                imapPort: '993',
                smtpServer: 'smtp.kpnmail.nl',
                smtpPort: '465',
                sslMode: 'SSL',
                source: 'KPN defaults',
            },
        ],
    },
]

function safeParseJson(raw, fallback) {
    try {
        return JSON.parse(raw)
    } catch {
        return fallback
    }
}

function cloneFormData(data = {}) {
    return { ...DEFAULT_FORM_DATA, ...data }
}

function getEmailDomain(email) {
    const trimmed = (email || '').trim().toLowerCase()
    const atIndex = trimmed.lastIndexOf('@')
    if (atIndex <= 0 || atIndex >= trimmed.length - 1) {
        return ''
    }
    return trimmed.slice(atIndex + 1).replace(/\.+$/, '')
}

function matchesDomain(domain, patterns) {
    return patterns.some((pattern) => domain === pattern || domain.endsWith(`.${pattern}`))
}

function buildCandidate(providerLabel, config, domain, suffix = '') {
    const id = [
        providerLabel,
        config.imapServer,
        config.imapPort,
        config.smtpServer,
        config.smtpPort,
        config.sslMode,
        suffix,
    ]
        .join('|')
        .toLowerCase()

    return {
        id,
        providerLabel,
        label: config.label,
        source: config.source,
        domain,
        imapServer: config.imapServer,
        imapPort: config.imapPort,
        smtpServer: config.smtpServer,
        smtpPort: config.smtpPort,
        sslMode: config.sslMode,
    }
}

function buildAutoConfigCandidates(email) {
    const domain = getEmailDomain(email)
    if (!domain) {
        return []
    }

    const matches = []
    AUTOCONFIG_PROVIDERS.forEach((provider) => {
        if (!matchesDomain(domain, provider.domains)) {
            return
        }

        provider.configs.forEach((config) => {
            matches.push(buildCandidate(provider.label, config, domain))
        })
    })

    if (matches.length > 0) {
        return matches
    }

    return []
}

function hasConnectionDetails(data) {
    return Boolean(
        data.imapServer
        || data.imapPort
        || data.smtpServer
        || data.smtpPort
        || data.password,
    )
}

function LoginPage() {
    const { t } = useTranslation()
    const navigate = useNavigate()
    const location = useLocation()
    const passwordRef = useRef(null)
    const [loading, setLoading] = useState(false)
    const [loadingText] = useState(t('Testing Connection and Authentication...'))
    const [responseMessage, setResponseMessage] = useState(null)
    const [accounts, setAccounts] = useState([])
    const [showAccounts, setShowAccounts] = useState(false)
    const [screenMode, setScreenMode] = useState(() => {
        const fromState = location?.state?.formData
        if (fromState && typeof fromState === 'object' && hasConnectionDetails(fromState)) {
            return 'manual'
        }
        return 'auto'
    })
    const [configMode, setConfigMode] = useState(() => {
        const fromState = location?.state?.formData
        if (fromState && typeof fromState === 'object' && hasConnectionDetails(fromState)) {
            return 'manual'
        }
        return 'auto'
    })
    const [autoCandidates, setAutoCandidates] = useState([])
    const [activeAutoIndex, setActiveAutoIndex] = useState(0)
    const [passwordVisible, setPasswordVisible] = useState(false)

    const [formData, setFormData] = useState(() => {
        const fromState = location?.state?.formData
        if (fromState && typeof fromState === 'object') {
            return cloneFormData(fromState)
        }
        return cloneFormData()
    })

    const clearDraft = useCallback(() => {
        // Draft persistence is intentionally disabled.
    }, [])

    const loadRegisteredAccounts = useCallback(async () => {
        try {
            const response = await fetch(apiUrl('/api/auth/accounts'))
            const data = await response.json()

            if (data.accounts && data.accounts.length > 0) {
                setAccounts(data.accounts)
                setShowAccounts(true)
            } else {
                setShowAccounts(false)
            }
        } catch (error) {
            console.error('Error loading accounts:', error)
            setShowAccounts(false)
        }
    }, [])

    useEffect(() => {
        loadRegisteredAccounts()
    }, [loadRegisteredAccounts])

    useEffect(() => {
        if (screenMode !== 'manual' || configMode !== 'auto' || !passwordVisible) {
            return
        }

        passwordRef.current?.focus()
    }, [passwordVisible, screenMode, configMode])

    const applyCandidate = useCallback((candidate) => {
        if (!candidate) {
            return
        }

        setFormData((prev) => {
            const next = {
                ...prev,
                imapServer: candidate.imapServer,
                imapPort: candidate.imapPort,
                smtpServer: candidate.smtpServer,
                smtpPort: candidate.smtpPort,
                sslMode: candidate.sslMode,
            }
            return next
        })
    }, [])

    const handleInputChange = (e) => {
        const { name, value } = e.target
        setFormData((prev) => {
            return { ...prev, [name]: value }
        })
    }

    const handleRegisteredAccountOpen = (account) => {
        hydrateAccountSession(account)
        clearDraft()
        navigate('/dashboard')
    }

    const handleGoogleStub = () => {
        setResponseMessage({
            type: 'error',
            text: t('Google sign-in is not available yet.'),
        })
    }

    const handleMicrosoftStub = () => {
        setResponseMessage({
            type: 'error',
            text: t('Microsoft sign-in is not available yet.'),
        })
    }

    const handleManualSetup = () => {
        setResponseMessage(null)
        setScreenMode('manual')
        setConfigMode('manual')
        setPasswordVisible(false)
    }

    const handleAutoSetup = async () => {
        const email = formData.email.trim()
        if (!email || !email.includes('@')) {
            setResponseMessage({
                type: 'error',
                text: t('Enter a valid email address first.'),
            })
            return
        }

        const candidates = buildAutoConfigCandidates(email)
        setResponseMessage(null)
        setLoading(true)

        if (!candidates.length) {
            setAutoCandidates([])
            setActiveAutoIndex(0)
            setFormData((prev) => ({
                ...prev,
                imapServer: '',
                imapPort: '',
                smtpServer: '',
                smtpPort: '',
                password: '',
                sslMode: 'STARTTLS',
            }))
            setScreenMode('manual')
            setConfigMode('manual')
            setPasswordVisible(false)
            setLoading(false)
            return
        }

        setFormData((prev) => ({
            ...prev,
            imapServer: '',
            imapPort: '',
            smtpServer: '',
            smtpPort: '',
            password: '',
            sslMode: 'STARTTLS',
        }))
        setScreenMode('auto')
        setConfigMode('auto')
        setPasswordVisible(false)
        setAutoCandidates(candidates)
        setActiveAutoIndex(0)
        applyCandidate(candidates[0])
        setLoading(false)
    }

    const handlePickCandidate = (index) => {
        const candidate = autoCandidates[index]
        if (!candidate) {
            return
        }

        setActiveAutoIndex(index)
        setPasswordVisible(false)
        applyCandidate(candidate)
    }

    const handleEditConfiguration = () => {
        setScreenMode('manual')
        setConfigMode('manual')
        setPasswordVisible(false)
    }

    const handleReturnToLanding = () => {
        setResponseMessage(null)
        setScreenMode('landing')
        setConfigMode('manual')
        setPasswordVisible(false)
    }

    const handleRevealPassword = () => {
        setPasswordVisible(true)
    }

    const handleSubmit = async (e) => {
        e.preventDefault()

        if (screenMode === 'landing' || (screenMode === 'auto' && !activeAutoCandidate)) {
            await handleAutoSetup()
            return
        }

        if (!passwordVisible) {
            handleRevealPassword()
            return
        }

        setResponseMessage(null)
        setLoading(true)

        const urlParams = new URLSearchParams()
        urlParams.append('EMAIL_ADDRESS', formData.email)
        urlParams.append('DISPLAY_NAME', formData.displayName)
        urlParams.append('IMAP_SERVER', formData.imapServer)
        urlParams.append('IMAP_PORT', formData.imapPort)
        urlParams.append('SMTP_SERVER', formData.smtpServer)
        urlParams.append('SMTP_PORT', formData.smtpPort)
        urlParams.append('PASSWORD', formData.password)
        urlParams.append('SSL_MODE', formData.sslMode)

        try {
            const response = await fetch(apiUrl('/api/auth/setup'), {
                method: 'POST',
                headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
                body: urlParams.toString(),
            })

            setLoading(false)

            const responseText = await response.text()
            let json = null

            try {
                json = JSON.parse(responseText)
            } catch {
                navigate('/not_auth', {
                    state: {
                        formData,
                        errorMessage: t('Authorization failed. The server returned an error page.'),
                    },
                })
                return
            }

            if (!response.ok) {
                if (response.status === 409 && json.status === 'already_exists') {
                    setResponseMessage({ type: 'already-exists', text: json.message })
                    return
                }
                if (response.status === 401 && json.status === 'failure') {
                    navigate('/not_auth', {
                        state: {
                            formData: json.formData || formData,
                            errorMessage: json.message,
                        },
                    })
                    return
                }
                throw json
            }

            setResponseMessage({ type: 'success', text: json.message })

            localStorage.setItem('temp_account_form', JSON.stringify(formData))
            clearDraft()

            setTimeout(() => {
                navigate('/font')
            }, 2000)
        } catch (error) {
            console.error('Error Details:', error)
            setLoading(false)

            let msg = t('An unknown error occurred. Please check the console logs.')

            if (error instanceof TypeError) {
                msg = t('The backend service may not be running.')
            } else if (error.message) {
                msg = error.message
                if (msg.includes('record layer failure')) {
                    msg += ' - ' + t('Server Name, Port, or SSL Mode might be incorrect.')
                } else if (msg.includes('no such user')) {
                    msg += ' - ' + t('Your Email Address or Password is incorrect.')
                }
            }

            setResponseMessage({ type: 'error', text: msg })
        }
    }

    const activeAutoCandidate = autoCandidates[activeAutoIndex] || null
    const autoPasswordPromptVisible = screenMode === 'auto' && configMode === 'auto' && passwordVisible
    const manualPasswordPromptVisible = screenMode === 'manual' && configMode === 'manual' && passwordVisible

    return (
        <div className="login-page">
            <div className="form-container">
                <LanguageSelector />
                <h2>{t('Email Account Settings')}</h2>
                <p className="login-page__subtitle">
                    {screenMode === 'landing'
                        ? t('Enter your name and email first, then choose how you want to continue.')
                        : t('We will help you find the mail server settings before asking for your password.')}
                </p>

                {loading && (
                    <div className="loading-overlay">
                        <div className="spinner" />
                        <p>{loadingText}</p>
                    </div>
                )}

                <form id="setupForm" onSubmit={handleSubmit}>
                    <div className="identity-card">
                        <div className="column-group">
                            <div className="column">
                                <div className="input-group">
                                    <label htmlFor="displayName">{t('Name')}</label>
                                    <input
                                        type="text"
                                        id="displayName"
                                                            name="displayName"
                                                            required
                                                            value={formData.displayName}
                                                            onChange={handleInputChange}
                                                        />
                                </div>
                            </div>
                            <div className="column">
                                <div className="input-group">
                                    <label htmlFor="email">{t('Email Address:')}</label>
                                                        <input
                                                            type="email"
                                                            id="email"
                                                            name="email"
                                                            required
                                                            value={formData.email}
                                                            onChange={handleInputChange}
                                                        />
                                </div>
                            </div>
                        </div>

                        {screenMode === 'landing' && (
                            <div className="login-page__landing-actions">
                                <button type="submit" className="setup-primary-button" id="autoSetupButton">
                                    {t('Continue')}
                                </button>
                                <button type="button" className="text-link-button text-link-button--centered" onClick={handleManualSetup}>
                                    {t('Manual configuration')}
                                </button>
                            </div>
                        )}

                        {screenMode === 'auto' && !activeAutoCandidate && (
                            <div className="login-page__landing-actions">
                                <button
                                    type="button"
                                    className="setup-primary-button"
                                    id="autoSetupButton"
                                    onClick={handleAutoSetup}
                                    disabled={loading}
                                >
                                    {t('Continue')}
                                </button>
                                <button type="button" className="text-link-button text-link-button--centered" onClick={handleManualSetup}>
                                    {t('Manual configuration')}
                                </button>
                            </div>
                        )}

                        {screenMode === 'landing' && (
                            <div className="login-page__social-actions">
                                <button type="button" className="shortcut-button shortcut-button--alt" onClick={handleGoogleStub}>
                                    <img src="/icon-google.png" alt="Google Icon" className="button-icon" />
                                    {t('Continue with Google')}
                                </button>
                                <button type="button" className="shortcut-button shortcut-button--alt" onClick={handleMicrosoftStub}>
                                    <img src="/icon-microsoft.png" alt="Microsoft Icon" className="button-icon" />
                                    {t('Continue with Microsoft')}
                                </button>
                            </div>
                        )}
                    </div>

                    {screenMode === 'manual' && (
                        <>
                            {configMode === 'manual' && (
                                <>
                                    <div className="section-block">
                                        <div className="section-block__header">
                                            <div>
                                                <p className="section-eyebrow">{t('Manual configuration')}</p>
                                                <h3>{t('Server settings')}</h3>
                                                <p className="section-description">
                                                    {t('Enter the IMAP and SMTP settings manually if you already know them.')}
                                                </p>
                                            </div>
                                        </div>

                                        <div className="manual-grid">
                                            <div className="column-group">
                                                <div className="column">
                                                    <div className="input-group">
                                                        <label htmlFor="imapServer">{t('IMAP Server:')}</label>
                                                        <input
                                                            type="text"
                                                            id="imapServer"
                                                            name="imapServer"
                                                            required
                                                            value={formData.imapServer}
                                                            onChange={handleInputChange}
                                                        />
                                                    </div>
                                                </div>
                                                <div className="column">
                                                    <div className="input-group">
                                                        <label htmlFor="imapPort">{t('IMAP Port:')}</label>
                                                        <input
                                                            type="number"
                                                            id="imapPort"
                                                            name="imapPort"
                                                            step="1"
                                                            required
                                                            value={formData.imapPort}
                                                            onChange={handleInputChange}
                                                        />
                                                    </div>
                                                </div>
                                            </div>

                                            <div className="column-group">
                                                <div className="column">
                                                    <div className="input-group">
                                                        <label htmlFor="smtpServer">{t('SMTP Server:')}</label>
                                                        <input
                                                            type="text"
                                                            id="smtpServer"
                                                            name="smtpServer"
                                                            required
                                                            value={formData.smtpServer}
                                                            onChange={handleInputChange}
                                                        />
                                                    </div>
                                                </div>
                                                <div className="column">
                                                    <div className="input-group">
                                                        <label htmlFor="smtpPort">{t('SMTP Port:')}</label>
                                                        <input
                                                            type="number"
                                                            id="smtpPort"
                                                            name="smtpPort"
                                                            step="1"
                                                            required
                                                            value={formData.smtpPort}
                                                            onChange={handleInputChange}
                                                        />
                                                    </div>
                                                </div>
                                            </div>

                                            <div className="input-group">
                                                <label>{t('Connection Encryption Mode:')}</label>
                                                <div className="radio-group">
                                                    {['STARTTLS', 'SSL', 'NONE'].map((mode) => (
                                                        <label key={mode} className="radio-label">
                                                            <input
                                                                type="radio"
                                                                name="sslMode"
                                                                value={mode}
                                                                checked={formData.sslMode === mode}
                                                                onChange={handleInputChange}
                                                            />
                                                            {mode === 'SSL' ? 'SSL/TLS' : mode}
                                                        </label>
                                                    ))}
                                                </div>
                                            </div>

                                            {manualPasswordPromptVisible && (
                                                <div className="input-group">
                                                    <label htmlFor="password">{t('Password')}</label>
                                                    <input
                                                        ref={passwordRef}
                                                        type="password"
                                                        id="password"
                                                        name="password"
                                                        required
                                                        value={formData.password}
                                                        onChange={handleInputChange}
                                                    />
                                                </div>
                                            )}
                                        </div>

                                        <button type="submit" className="setup-primary-button" id="submitButton" disabled={loading}>
                                            {manualPasswordPromptVisible ? t('Save Settings and Test') : t('Continue')}
                                        </button>
                                    </div>

                                    <button
                                        type="button"
                                        className="text-link-button text-link-button--centered"
                                        onClick={handleReturnToLanding}
                                    >
                                        {t('Automatic configuration')}
                                    </button>
                                </>
                            )}

                            {configMode === 'manual' && (
                                <>
                                    <div className="login-page__social-actions">
                                        <button type="button" className="shortcut-button shortcut-button--alt" onClick={handleGoogleStub}>
                                            <img src="/icon-google.png" alt="Google Icon" className="button-icon" />
                                            {t('Continue with Google')}
                                        </button>
                                        <button type="button" className="shortcut-button shortcut-button--alt" onClick={handleMicrosoftStub}>
                                            <img src="/icon-microsoft.png" alt="Microsoft Icon" className="button-icon" />
                                            {t('Continue with Microsoft')}
                                        </button>
                                    </div>
                                </>
                            )}
                        </>
                    )}

                    {screenMode === 'auto' && activeAutoCandidate && (
                        <div className="section-block">
                            <div className="section-block__header">
                                <div>
                                    <p className="section-eyebrow">{t('Automatic configuration')}</p>
                                    <h3>{t('Suggested configuration')}</h3>
                                    <p className="section-description">
                                        {t('Review the suggested settings and continue to enter your password.')}
                                    </p>
                                </div>
                                <button type="button" className="ghost-action-button" onClick={handleEditConfiguration}>
                                    {t('Edit configuration')}
                                </button>
                            </div>

                            <div className="config-preview-card">
                                <div className="config-preview-card__title">
                                    <span className="config-preview-card__badge">{t('Recommended')}</span>
                                    <strong>{activeAutoCandidate.providerLabel}</strong>
                                </div>
                                <div className="config-preview-grid">
                                    <div>
                                        <span>{t('IMAP Server:')}</span>
                                        <strong>{activeAutoCandidate.imapServer}</strong>
                                    </div>
                                    <div>
                                        <span>{t('IMAP Port:')}</span>
                                        <strong>{activeAutoCandidate.imapPort}</strong>
                                    </div>
                                    <div>
                                        <span>{t('SMTP Server:')}</span>
                                        <strong>{activeAutoCandidate.smtpServer}</strong>
                                    </div>
                                    <div>
                                        <span>{t('SMTP Port:')}</span>
                                        <strong>{activeAutoCandidate.smtpPort}</strong>
                                    </div>
                                    <div className="config-preview-grid__full">
                                        <span>{t('Connection Encryption Mode:')}</span>
                                        <strong>{activeAutoCandidate.sslMode === 'SSL' ? 'SSL/TLS' : activeAutoCandidate.sslMode}</strong>
                                    </div>
                                </div>
                                <p className="config-preview-card__source">
                                    {t('Source:')} {activeAutoCandidate.source}
                                </p>
                            </div>

                            {autoCandidates.length > 1 && (
                                <div className="candidate-pills">
                                    {autoCandidates.map((candidate, index) => (
                                        <button
                                            key={candidate.id}
                                            type="button"
                                            className={`candidate-pill ${index === activeAutoIndex ? 'active' : ''}`}
                                            onClick={() => handlePickCandidate(index)}
                                        >
                                            {candidate.providerLabel} · {t(candidate.label.toLowerCase().includes('alternate') ? 'Alternate' : 'Recommended')}
                                        </button>
                                    ))}
                                </div>
                            )}

                            {!autoPasswordPromptVisible ? (
                                <>
                                    <button type="button" className="setup-primary-button" onClick={handleRevealPassword}>
                                        {t('Continue')}
                                    </button>
                                    <button
                                        type="button"
                                        className="text-link-button text-link-button--centered"
                                        onClick={handleEditConfiguration}
                                    >
                                        {t('Manual configuration')}
                                    </button>
                                </>
                            ) : (
                                <>
                                    <div className="input-group auto-password-field">
                                        <label htmlFor="password">{t('Password')}</label>
                                        <input
                                            ref={passwordRef}
                                            type="password"
                                            id="password"
                                            name="password"
                                            required
                                            value={formData.password}
                                            onChange={handleInputChange}
                                        />
                                    </div>

                                    <button type="submit" className="setup-primary-button" id="submitButton" disabled={loading}>
                                        {t('Save Settings and Test')}
                                    </button>
                                    <button
                                        type="button"
                                        className="text-link-button text-link-button--centered"
                                        onClick={handleEditConfiguration}
                                    >
                                        {t('Manual configuration')}
                                    </button>
                                </>
                            )}
                        </div>
                    )}

                    {responseMessage && (
                        <div className={`response-message ${responseMessage.type}-message`}>
                            {responseMessage.type === 'already-exists' ? (
                                <div className="already-exists-message">{responseMessage.text}</div>
                            ) : (
                                responseMessage.text
                            )}
                        </div>
                    )}
                </form>
            </div>

            {showAccounts && (
                <div className="registered-accounts-container" id="registeredAccountsContainer">
                    <div className="container-header">
                        <h3>{t('Registered Accounts')}</h3>
                        <button
                            type="button"
                            className="settings-btn"
                            title={t('Settings')}
                            onClick={() => navigate('/account-settings')}
                        >
                            {t('Settings')}
                        </button>
                    </div>
                    <div className="accounts-list">
                        {accounts.map((account) => (
                            <div
                                key={account.account_id}
                                className="account-item clickable"
                                onClick={() => handleRegisteredAccountOpen(account)}
                            >
                                <strong>{account.display_name}</strong>
                                <span>{account.email_address}</span>
                            </div>
                        ))}
                    </div>
                </div>
            )}
        </div>
    )
}

export default LoginPage
