import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
// Use Vite's glob import to load all translation JSON files dynamically
const modules = import.meta.glob('./locales/**/*.json', { eager: true });
const resources = {};

for (const path in modules) {
    const parts = path.split('/');
    if (parts.length >= 3) {
        const lang = parts[2];
        resources[lang] = {
            translation: modules[path].default || modules[path]
        };
    }
}

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
