// DOM helpers shared by the whole GUI: element builders, notifications, and
// dialogs that keep keyboard focus inside while open.
export const $ = (id) => document.getElementById(id);

let toastTimer;
export function toast(msg, isError = false) {
  const el = $('toast');
  el.textContent = msg;
  el.classList.toggle('error', isError);
  el.classList.add('visible');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('visible'), isError ? 6000 : 3000);
}
export const fail = (e) => toast(String(e?.message ?? e), true);
export const fmtSize = (b) => b >= 1 << 30 ? (b / (1 << 30)).toFixed(2) + ' GB' : b >= 1 << 20 ? (b / (1 << 20)).toFixed(1) + ' MB' : b >= 1024 ? (b / 1024).toFixed(0) + ' KB' : b + ' B';
export const baseName = (p) => decodeURIComponent(String(p).split(/[\\/]/).pop() || p);
export const shortId = (id) => id.slice(0, 10) + '…';
export const initials = (name) => (String(name).trim().split(/\s+/).slice(0, 2).map(w => [...w][0]).join('') || '?').toUpperCase();
export function el(tag, props = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v);
  }
  for (const c of children.flat()) e.append(c instanceof Node ? c : document.createTextNode(String(c)));
  return e;
}
export function icon(name, cls = '') {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('class', ('i ' + cls).trim());
  const use = document.createElementNS(ns, 'use');
  use.setAttribute('href', '#i-' + name);
  svg.append(use);
  return svg;
}
export const preview = (text, max = 120) => { const flat = String(text).replace(/\s+/g, ' ').trim(); return flat.length > max ? flat.slice(0, max - 1) + '…' : flat; };
export const isLink = (text) => /^https?:\/\/\S+$/i.test(String(text).trim());
export const dotClass = (state) => 'dot ' + (state === true ? 'ok' : state === false ? 'bad' : 'wait');

// Dialogs keep keyboard focus inside while open and give it back to the
// element that opened them when closed (WAI-ARIA dialog pattern).
// `a[href]`, not `[href]`: the <use href> of every SVG icon is not focusable.
const FOCUSABLE = 'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';
const focusReturn = new Map();
const visibleFocusable = (m) => [...m.querySelectorAll(FOCUSABLE)].filter(e => e.offsetParent !== null);
const topModal = () => [...document.querySelectorAll('.modal-overlay.active')].pop();
export function open(id) {
  const m = $(id);
  if (!m.classList.contains('active')) {
    focusReturn.set(id, document.activeElement);
    m.classList.add('active');
  }
  if (!m.contains(document.activeElement)) {
    const first = m.querySelector('[autofocus]') ?? visibleFocusable(m).find(e => !e.matches('[data-close]')) ?? visibleFocusable(m)[0];
    requestAnimationFrame(() => first?.focus());
  }
}
export function close(id) {
  const m = $(id);
  if (!m.classList.contains('active')) return;
  m.classList.remove('active');
  const back = focusReturn.get(id);
  focusReturn.delete(id);
  if (back?.isConnected && !topModal()) back.focus();
}
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Tab') return;
  const m = topModal();
  if (!m) return;
  const items = visibleFocusable(m);
  if (!items.length) return;
  const first = items[0], last = items[items.length - 1];
  if (!m.contains(document.activeElement)) { e.preventDefault(); first.focus(); }
  else if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
});
for (const m of document.querySelectorAll('.modal-overlay')) {
  m.addEventListener('click', (e) => { if (e.target === m && m.getAttribute('role') !== 'alertdialog') close(m.id); });
}
for (const b of document.querySelectorAll('[data-close]')) {
  b.addEventListener('click', () => close(b.dataset.close));
}
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape') return;
  for (const m of document.querySelectorAll('.modal-overlay.active:not([role=alertdialog])')) close(m.id);
});

export function emptyState(iconName, text, extra = '') {
  return el('div', { class: 'empty' }, icon(iconName, 'lg'), el('div', {}, text), extra);
}
