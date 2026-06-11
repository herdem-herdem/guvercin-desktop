import { useState, useMemo, useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import './LanguageSelector.css'

const LANGUAGES = [
    { code: 'tr', flag: 'tr', native: 'Türkçe', english: 'Turkish', alt: 'Turkey' },
    { code: 'en', flag: 'gb', native: 'English', english: 'English', alt: 'English' },
    { code: 'ar-ps', flag: 'ps', native: 'العربية', english: 'Arabic – Palestine', alt: 'Palestine' },
    { code: 'az', flag: 'az', native: 'Azərbaycanca', english: 'Azerbaijani', alt: 'Azerbaijan' },
    { code: 'kk', flag: 'kz', native: 'Қазақша', english: 'Kazakh', alt: 'Kazakhstan' },
    { code: 'ky', flag: 'kg', native: 'Кыргызча', english: 'Kyrgyz', alt: 'Kyrgyzstan' },
    { code: 'tk', flag: 'tm', native: 'Türkmençe', english: 'Turkmen', alt: 'Turkmenistan' },
    { code: 'uz', flag: 'uz', native: "O'zbek", english: 'Uzbek', alt: 'Uzbekistan' },
    { code: 'sq-xk', flag: 'xk', native: 'Shqip', english: 'Albanian – Kosovo', alt: 'Kosovo' },
    { code: 'af', flag: 'ad', native: 'Català', english: 'Catalan', alt: 'Andorra' },
    { code: 'ar', flag: 'ae', native: 'العربية', english: 'Arabic', alt: 'UAE' },
    { code: 'ps', flag: 'af', native: 'پښتو', english: 'Pashto', alt: 'Afghanistan' },
    { code: 'sq', flag: 'al', native: 'Shqip', english: 'Albanian', alt: 'Albania' },
    { code: 'hy', flag: 'am', native: 'Հայերեն', english: 'Armenian', alt: 'Armenia' },
    { code: 'pt', flag: 'pt', native: 'Português', english: 'Portuguese', alt: 'Portugal' },
    { code: 'es', flag: 'es', native: 'Español', english: 'Spanish', alt: 'Spain' },
    { code: 'de', flag: 'de', native: 'Deutsch', english: 'German', alt: 'Germany' },
    { code: 'bs', flag: 'ba', native: 'Bosanski', english: 'Bosnian', alt: 'Bosnia' },
    { code: 'bn', flag: 'bd', native: 'বাংলা', english: 'Bengali', alt: 'Bangladesh' },
    { code: 'bg', flag: 'bg', native: 'Български', english: 'Bulgarian', alt: 'Bulgaria' },
    { code: 'ar-bh', flag: 'bh', native: 'العربية', english: 'Arabic – Bahrain', alt: 'Bahrain' },
    { code: 'zh', flag: 'cn', native: '中文', english: 'Chinese', alt: 'China' },
    { code: 'cs', flag: 'cz', native: 'Čeština', english: 'Czech', alt: 'Czech Republic' },
    { code: 'da', flag: 'dk', native: 'Dansk', english: 'Danish', alt: 'Denmark' },
    { code: 'et', flag: 'ee', native: 'Eesti', english: 'Estonian', alt: 'Estonia' },
    { code: 'fi', flag: 'fi', native: 'Suomi', english: 'Finnish', alt: 'Finland' },
    { code: 'fr', flag: 'fr', native: 'Français', english: 'French', alt: 'France' },
    { code: 'ka', flag: 'ge', native: 'ქართული', english: 'Georgian', alt: 'Georgia' },
    { code: 'el', flag: 'gr', native: 'Ελληνικά', english: 'Greek', alt: 'Greece' },
    { code: 'hr', flag: 'hr', native: 'Hrvatski', english: 'Croatian', alt: 'Croatia' },
    { code: 'hu', flag: 'hu', native: 'Magyar', english: 'Hungarian', alt: 'Hungary' },
    { code: 'id', flag: 'id', native: 'Bahasa Indonesia', english: 'Indonesian', alt: 'Indonesia' },
    { code: 'sw', flag: 'ke', native: 'Kiswahili', english: 'Swahili', alt: 'Kenya' },
    { code: 'hi', flag: 'in', native: 'हिन्दी', english: 'Hindi', alt: 'India' },
    { code: 'fa', flag: 'ir', native: 'فارسی', english: 'Persian', alt: 'Iran' },
    { code: 'is', flag: 'is', native: 'Íslenska', english: 'Icelandic', alt: 'Iceland' },
    { code: 'it', flag: 'it', native: 'Italiano', english: 'Italian', alt: 'Italy' },
    { code: 'ja', flag: 'jp', native: '日本語', english: 'Japanese', alt: 'Japan' },
    { code: 'ko', flag: 'kr', native: '한국어', english: 'Korean', alt: 'South Korea' },
    { code: 'lo', flag: 'la', native: 'ລາວ', english: 'Lao', alt: 'Laos' },
    { code: 'si', flag: 'lk', native: 'සිංහල', english: 'Sinhala', alt: 'Sri Lanka' },
    { code: 'lt', flag: 'lt', native: 'Lietuvių', english: 'Lithuanian', alt: 'Lithuania' },
    { code: 'lv', flag: 'lv', native: 'Latviešu', english: 'Latvian', alt: 'Latvia' },
    { code: 'mk', flag: 'mk', native: 'Македонски', english: 'Macedonian', alt: 'N. Macedonia' },
    { code: 'my', flag: 'mm', native: 'မြန်မာ', english: 'Burmese', alt: 'Myanmar' },
    { code: 'mn', flag: 'mn', native: 'Монгол', english: 'Mongolian', alt: 'Mongolia' },
    { code: 'pa', flag: 'in', native: 'ਪੰਜਾਬੀ', english: 'Punjabi', alt: 'India' },
    { code: 'ms', flag: 'my', native: 'Bahasa Melayu', english: 'Malay', alt: 'Malaysia' },
    { code: 'nl', flag: 'nl', native: 'Nederlands', english: 'Dutch', alt: 'Netherlands' },
    { code: 'no', flag: 'no', native: 'Norsk', english: 'Norwegian', alt: 'Norway' },
    { code: 'ne', flag: 'np', native: 'नेपाली', english: 'Nepali', alt: 'Nepal' },
    { code: 'fil', flag: 'ph', native: 'Filipino', english: 'Filipino', alt: 'Philippines' },
    { code: 'ur', flag: 'pk', native: 'اردو', english: 'Urdu', alt: 'Pakistan' },
    { code: 'pl', flag: 'pl', native: 'Polski', english: 'Polish', alt: 'Poland' },
    { code: 'ro', flag: 'ro', native: 'Română', english: 'Romanian', alt: 'Romania' },
    { code: 'sr', flag: 'rs', native: 'Српски', english: 'Serbian', alt: 'Serbia' },
    { code: 'ru', flag: 'ru', native: 'Русский', english: 'Russian', alt: 'Russia' },
    { code: 'sv', flag: 'se', native: 'Svenska', english: 'Swedish', alt: 'Sweden' },
    { code: 'sl', flag: 'si', native: 'Slovenščina', english: 'Slovenian', alt: 'Slovenia' },
    { code: 'sk', flag: 'sk', native: 'Slovenčina', english: 'Slovak', alt: 'Slovakia' },
    { code: 'so', flag: 'so', native: 'Soomaali', english: 'Somali', alt: 'Somalia' },
    { code: 'th', flag: 'th', native: 'ภาษาไทย', english: 'Thai', alt: 'Thailand' },
    { code: 'uk', flag: 'ua', native: 'Українська', english: 'Ukrainian', alt: 'Ukraine' },
    { code: 'vi', flag: 'vn', native: 'Tiếng Việt', english: 'Vietnamese', alt: 'Vietnam' },
]

const PRIORITY_LANGS = ['tr', 'en', 'ar-ps', 'az', 'kk', 'ky', 'tk', 'uz', 'sq-xk']

export default function LanguageSelector() {
    const { t, i18n } = useTranslation()
    const [isOpen, setIsOpen] = useState(false)
    const [searchQuery, setSearchQuery] = useState('')
    const dropdownRef = useRef(null)

    // Close when clicking outside
    useEffect(() => {
        function handleClickOutside(event) {
            if (dropdownRef.current && !dropdownRef.current.contains(event.target)) {
                setIsOpen(false)
            }
        }
        if (isOpen) {
            document.addEventListener('mousedown', handleClickOutside)
        }
        return () => {
            document.removeEventListener('mousedown', handleClickOutside)
        }
    }, [isOpen])

    const currentLangCode = i18n.language || 'en'
    const currentLang = useMemo(() => {
        return LANGUAGES.find(l => l.code === currentLangCode) || LANGUAGES.find(l => l.code === 'en')
    }, [currentLangCode])

    const sortedLanguages = useMemo(() => {
        return [...LANGUAGES].sort((a, b) => {
            const indexA = PRIORITY_LANGS.indexOf(a.code)
            const indexB = PRIORITY_LANGS.indexOf(b.code)
            if (indexA !== -1 && indexB !== -1) return indexA - indexB
            if (indexA !== -1) return -1
            if (indexB !== -1) return 1
            return a.native.localeCompare(b.native, 'en')
        })
    }, [])

    const filteredLanguages = useMemo(() => {
        if (!searchQuery.trim()) return sortedLanguages
        const q = searchQuery.toLowerCase()
        return sortedLanguages.filter(
            (lang) =>
                lang.native.toLowerCase().includes(q) ||
                lang.english.toLowerCase().includes(q) ||
                lang.alt.toLowerCase().includes(q) ||
                lang.code.toLowerCase().includes(q)
        )
    }, [searchQuery, sortedLanguages])

    const handleSelect = (code) => {
        localStorage.setItem('temp_language', code)
        localStorage.setItem('language', code)
        i18n.changeLanguage(code)
        setIsOpen(false)
        setSearchQuery('')
    }

    return (
        <div className="global-language-selector" ref={dropdownRef}>
            <button
                type="button"
                className="selector-trigger"
                onClick={() => setIsOpen(!isOpen)}
                aria-haspopup="true"
                aria-expanded={isOpen}
            >
                {currentLang && (
                    <>
                        <img
                            src={`/flags/${currentLang.flag}.png`}
                            alt={currentLang.alt}
                            className="current-flag"
                        />
                        <span className="current-name">{currentLang.native}</span>
                    </>
                )}
                <span className="arrow-down">▾</span>
            </button>

            {isOpen && (
                <div className="selector-dropdown">
                    <div className="dropdown-header">
                        <h4>{t('Language')}</h4>
                        <button
                            type="button"
                            className="close-dropdown-btn"
                            onClick={() => setIsOpen(false)}
                        >
                            ✕
                        </button>
                    </div>
                    <div className="dropdown-search">
                        <input
                            type="text"
                            placeholder={t('Search language or country...')}
                            value={searchQuery}
                            onChange={(e) => setSearchQuery(e.target.value)}
                            autoFocus
                        />
                    </div>
                    <div className="dropdown-list">
                        {filteredLanguages.length === 0 && (
                            <div className="no-results">{t('No results found.')}</div>
                        )}
                        {filteredLanguages.map((lang) => (
                            <div
                                key={lang.code}
                                className={`dropdown-item ${currentLangCode === lang.code ? 'active' : ''}`}
                                onClick={() => handleSelect(lang.code)}
                            >
                                <img src={`/flags/${lang.flag}.png`} alt={lang.alt} className="item-flag" />
                                <span className="item-names">
                                    <span className="native-name">{lang.native}</span>
                                    <span className="english-name">({lang.english})</span>
                                </span>
                                {currentLangCode === lang.code && <span className="checkmark">✓</span>}
                            </div>
                        ))}
                    </div>
                </div>
            )}
        </div>
    )
}
