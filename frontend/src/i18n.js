import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import translationEN from './locales/en/translation.json';
import translationTR from './locales/tr/translation.json';

const resources = {
    en: {
        translation: translationEN
    },
    tr: {
        translation: translationTR
    }
};

function getInitialLanguage() {
    const saved = localStorage.getItem('temp_language') || localStorage.getItem('language');
    if (saved) return saved;

    try {
        const sysLang = navigator.language || navigator.languages?.[0];
        if (sysLang) {
            const normalized = sysLang.toLowerCase();
            if (normalized === 'tr' || normalized.startsWith('tr-')) {
                return 'tr';
            }
        }
    } catch (e) {
        console.error('Failed to detect system language', e);
    }
    return 'en';
}

const initialLang = getInitialLanguage();
// Ensure the initial language is saved so other components can reference it correctly
if (!localStorage.getItem('temp_language') && !localStorage.getItem('language')) {
    localStorage.setItem('temp_language', initialLang);
}

i18n
    .use(initReactI18next)
    .init({
        resources,
        lng: initialLang,
        fallbackLng: "en",
        interpolation: {
            escapeValue: false
        }
    });

export default i18n;
