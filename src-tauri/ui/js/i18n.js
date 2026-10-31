// Translations of the GUI: t() looks a key up in the active language, falling
// back to English. The texts live in locales/<code>.js.
import en from './locales/en.js';
import pl from './locales/pl.js';

const I18N = { en, pl };
export let lang = 'en';

/** Switches the language used by t(); call applyStaticText() to update the page. */
export function setLang(code) {
  lang = code;
}
export function t(key, vars = {}) {
  const v = I18N[lang]?.[key] ?? I18N.en[key] ?? key;
  return typeof v === 'function' ? v(vars) : v.replace(/\{(\w+)\}/g, (m, k) => k in vars ? String(vars[k]) : m);
}
export function applyStaticText() {
  document.documentElement.lang = lang;
  document.querySelectorAll('[data-i18n]').forEach(e => { e.textContent = t(e.dataset.i18n); });
  document.querySelectorAll('[data-i18n-ph]').forEach(e => { e.placeholder = t(e.dataset.i18nPh); });
  document.querySelectorAll('[data-i18n-aria]').forEach(e => { e.setAttribute('aria-label', t(e.dataset.i18nAria)); });
  document.querySelectorAll('[data-i18n-title]').forEach(e => { e.title = t(e.dataset.i18nTitle); });
}
