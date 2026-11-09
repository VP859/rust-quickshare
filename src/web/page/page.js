// The page a browser sees when it opens a Ferry session link: files to
// download and, when allowed, a way to send files to the Ferry device.
// It only talks to the server it came from (relative URLs under the token).
// Everything with content is encrypted with the key after `#` in the link,
// which the browser never sends to the server (seal.js, src/web/seal.rs).
import { Opener, openAll, purpose, RECORD, Sealer, sealAll, sealedLen, keyFromFragment } from './seal.js';

const TEXTS = {
  en: {
    title: 'Files from {device}',
    title_receive_only: 'Send files to {device}',
    subtitle: 'Shared over your local network with Ferry.',
    download_title: 'Download',
    download: 'Download',
    upload_title: 'Send to {device}',
    choose: 'Choose files',
    drop_hint: 'or drop them here',
    waiting: 'Waiting for {device} to accept {n} file(s)…',
    sending: 'Sending…',
    sent: 'Sent',
    all_sent: 'All files were sent.',
    failed: 'Failed',
    declined: '{device} declined the files.',
    expires: 'This link works for {min} more minute(s).',
    expired: 'This link has expired. Start a new session in Ferry.',
    nothing: 'Nothing is shared in this session.',
    unreachable: 'Ferry cannot be reached. The session may have ended.',
    too_many: 'At most {n} files at once.',
    security_note: 'Files and names are encrypted with a key that is only in this link, not sent over the network. Someone who can change this page on the way could still read them: use the link on a network you trust.',
    no_key: 'This link is incomplete: the key after “#” is missing. Open the whole link or scan the QR code again.',
    wrong_key: 'This link does not belong to the session (the key does not fit). Open the whole link or scan the QR code again.',
    downloading: 'Downloading…',
    encrypting: 'Encrypting…',
  },
  pl: {
    title: 'Pliki z urządzenia {device}',
    title_receive_only: 'Wyślij pliki do urządzenia {device}',
    subtitle: 'Udostępnione w sieci lokalnej przez Ferry.',
    download_title: 'Pobieranie',
    download: 'Pobierz',
    upload_title: 'Wyślij do urządzenia {device}',
    choose: 'Wybierz pliki',
    drop_hint: 'lub upuść je tutaj',
    waiting: 'Czekam, aż {device} przyjmie pliki ({n})…',
    sending: 'Wysyłanie…',
    sent: 'Wysłano',
    all_sent: 'Wszystkie pliki zostały wysłane.',
    failed: 'Błąd',
    declined: '{device} odrzuca pliki.',
    expires: 'Ten link działa jeszcze przez {min} min.',
    expired: 'Ten link wygasł. Rozpocznij nową sesję w Ferry.',
    nothing: 'W tej sesji nic nie jest udostępnione.',
    unreachable: 'Nie można połączyć się z Ferry. Sesja mogła się zakończyć.',
    too_many: 'Najwyżej {n} plików naraz.',
    security_note: 'Pliki i nazwy są szyfrowane kluczem, który jest tylko w tym linku i nie jest wysyłany przez sieć. Ktoś, kto może po drodze zmienić tę stronę, nadal mógłby je odczytać: używaj linku w zaufanej sieci.',
    no_key: 'Ten link jest niepełny: brakuje klucza po „#”. Otwórz cały link albo zeskanuj kod QR jeszcze raz.',
    wrong_key: 'Ten link nie pasuje do sesji (klucz się nie zgadza). Otwórz cały link albo zeskanuj kod QR jeszcze raz.',
    downloading: 'Pobieranie…',
    encrypting: 'Szyfrowanie…',
  },
};
const lang = (navigator.languages ?? [navigator.language]).map(l => String(l).slice(0, 2)).find(l => l in TEXTS) ?? 'en';
document.documentElement.lang = lang;
const t = (key, vars = {}) => TEXTS[lang][key].replace(/\{(\w+)\}/g, (_, k) => String(vars[k] ?? ''));

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};
const fmtSize = (b) => b >= 1 << 30 ? (b / (1 << 30)).toFixed(2) + ' GB' : b >= 1 << 20 ? (b / (1 << 20)).toFixed(1) + ' MB'
  : b >= 1024 ? (b / 1024).toFixed(0) + ' KB' : b + ' B';

function status(text, error = false) {
  $('status').textContent = text;
  $('status').classList.toggle('error', error);
}

/** The message of an error answer (`{"message": …}` or plain text). */
async function reason(res) {
  const body = await res.text();
  try { return JSON.parse(body).message ?? body; } catch { return body; }
}

let info = null;
let expiresAt = null;

/**
 * The session key: from the link (`#k=…`), kept for this tab in
 * sessionStorage so a reload works, and taken out of the address bar so it
 * does not stay in the browser's history.
 */
function sessionKey() {
  let key = keyFromFragment(location.hash);
  const stored = `ferry-key:${location.pathname}`;
  try {
    if (key) sessionStorage.setItem(stored, location.hash);
    else key = keyFromFragment(sessionStorage.getItem(stored));
  } catch { /* storage may be off; the link still works once */ }
  if (location.hash) history.replaceState(null, '', location.pathname + location.search);
  return key;
}
const key = sessionKey();

/** Saves `parts` (Uint8Arrays) as a file named `name`. */
function save(parts, name) {
  const url = URL.createObjectURL(new Blob(parts, { type: 'application/octet-stream' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/** Downloads shared file `f`, decrypts it and saves it; progress in `button`. */
async function download(f, button) {
  button.disabled = true;
  const label = button.textContent;
  status(t('downloading'));
  try {
    const res = await fetch(`api/files/${f.id}`, { cache: 'no-store' });
    if (!res.ok) throw new Error(await reason(res));
    const opener = new Opener(key, purpose.download(f.id), f.size);
    const parts = [];
    let done = 0;
    const reader = res.body.getReader();
    while (true) {
      const { value, done: end } = await reader.read();
      if (end) break;
      for (const part of opener.push(value)) {
        parts.push(part);
        done += part.length;
      }
      button.textContent = `${f.size ? Math.floor((done / f.size) * 100) : 100}%`;
    }
    opener.finish();
    save(parts, f.name);
    status('');
  } catch (e) {
    status(`${f.name}: ${e.message || t('failed')}`, true);
  } finally {
    button.disabled = false;
    button.textContent = label;
  }
}

function renderExpiry() {
  if (!expiresAt) { $('expires').textContent = ''; return; }
  const left = expiresAt - Date.now();
  $('expires').textContent = left > 0 ? t('expires', { min: Math.max(1, Math.ceil(left / 60000)) }) : t('expired');
}

async function load() {
  for (const node of document.querySelectorAll('[data-t]')) node.textContent = t(node.dataset.t);
  if (!key) { status(t('no_key'), true); return; }
  let res;
  try { res = await fetch('api/info', { cache: 'no-store' }); } catch { status(t('unreachable'), true); return; }
  if (!res.ok) { status(await reason(res), true); return; }
  try {
    info = JSON.parse(new TextDecoder().decode(openAll(key, purpose.info, new Uint8Array(await res.arrayBuffer()))));
  } catch { status(t('wrong_key'), true); return; }
  const device = info.device;
  $('title').textContent = t(info.files.length ? 'title' : 'title_receive_only', { device });
  $('subtitle').textContent = t('subtitle');
  document.title = `Ferry – ${device}`;
  if (info.files.length) {
    $('download').hidden = false;
    $('files').replaceChildren(...info.files.map((f) => {
      const li = el('li');
      const button = el('button', 'button', t('download'));
      button.type = 'button';
      button.setAttribute('aria-label', `${t('download')}: ${f.name}`);
      button.addEventListener('click', () => download(f, button));
      li.append(el('span', 'name', f.name), button, el('span', 'size', fmtSize(f.size)));
      return li;
    }));
  }
  if (info.receive) {
    $('upload').hidden = false;
    $('upload-title').textContent = t('upload_title', { device });
  }
  if (!info.files.length && !info.receive) status(t('nothing'));
  if (info.expires_in !== null) {
    expiresAt = Date.now() + info.expires_in * 1000;
    renderExpiry();
    setInterval(renderExpiry, 15000);
  }
}

/** `file` encrypted as the stream `aad`, as a Blob (read in pieces, not at once). */
async function encryptFile(file, aad) {
  const sealer = new Sealer(key, aad);
  const parts = [sealer.prefix];
  const count = Math.max(1, Math.ceil(file.size / RECORD));
  const STEP = 16; // records read from the file at once
  for (let first = 0; first < count; first += STEP) {
    const bytes = new Uint8Array(await file.slice(first * RECORD, (first + STEP) * RECORD).arrayBuffer());
    for (let i = first; i < Math.min(first + STEP, count); i++) {
      const at = (i - first) * RECORD;
      parts.push(sealer.seal(bytes.subarray(at, at + RECORD), i + 1 === count));
    }
  }
  const blob = new Blob(parts, { type: 'application/octet-stream' });
  if (blob.size !== sealedLen(file.size)) throw new Error('the file changed while it was read');
  return blob;
}

/** Uploads one file with progress; resolves to an error message or null. */
function put(url, file, onProgress) {
  return new Promise((resolve) => {
    const xhr = new XMLHttpRequest();
    xhr.open('PUT', url);
    xhr.upload.onprogress = (e) => { if (e.lengthComputable) onProgress(e.loaded / e.total); };
    xhr.onload = () => {
      if (xhr.status === 200) return resolve(null);
      try { resolve(JSON.parse(xhr.responseText).message); } catch { resolve(xhr.statusText || String(xhr.status)); }
    };
    xhr.onerror = () => resolve(t('unreachable'));
    xhr.send(file);
  });
}

async function send(files) {
  if (!files.length || !info) return;
  if (files.length > info.max_files) { status(t('too_many', { n: info.max_files }), true); return; }
  const drop = $('drop');
  drop.classList.add('busy');
  const rows = files.map((file) => {
    const li = el('li');
    const bar = el('div', 'progress');
    bar.append(el('div'));
    const state = el('span', 'state', '');
    li.append(el('span', 'name', file.name), state, bar);
    return { li, bar: bar.firstChild, state };
  });
  $('uploads').prepend(...rows.map(r => r.li));
  status(t('waiting', { device: info.device, n: files.length }));
  try {
    const offer = new TextEncoder().encode(JSON.stringify({ files: files.map(f => ({ name: f.name, size: f.size })) }));
    const res = await fetch('api/offer', {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: sealAll(key, purpose.offer, offer),
    });
    if (!res.ok) {
      const why = res.status === 403 ? t('declined', { device: info.device }) : await reason(res);
      for (const r of rows) { r.li.classList.add('failed'); r.state.textContent = t('failed'); }
      status(why, true);
      return;
    }
    const { id } = await res.json();
    status(t('sending'));
    let failed = 0;
    for (const [i, file] of files.entries()) {
      const r = rows[i];
      r.state.textContent = t('encrypting');
      let body;
      try { body = await encryptFile(file, purpose.upload(id, i)); } catch (e) {
        r.li.classList.add('failed');
        r.state.textContent = t('failed');
        r.li.title = e.message;
        failed += 1;
        continue;
      }
      r.state.textContent = '0%';
      const error = await put(`api/upload/${id}/${i}`, body, (p) => {
        r.bar.style.width = Math.round(p * 100) + '%';
        r.state.textContent = Math.round(p * 100) + '%';
      });
      r.li.classList.add(error ? 'failed' : 'done');
      r.bar.style.width = '100%';
      r.state.textContent = error ? t('failed') : t('sent');
      if (error) { failed += 1; r.li.title = error; }
    }
    status(failed ? t('failed') : t('all_sent'), failed > 0);
  } catch {
    status(t('unreachable'), true);
  } finally {
    drop.classList.remove('busy');
    $('picker').value = '';
  }
}

$('picker').addEventListener('change', (e) => send([...e.target.files]));
const drop = $('drop');
drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
drop.addEventListener('dragleave', () => drop.classList.remove('over'));
drop.addEventListener('drop', (e) => {
  e.preventDefault();
  drop.classList.remove('over');
  send([...(e.dataTransfer?.files ?? [])]);
});

load();
