// The page a browser sees when it opens a Ferry session link: files to
// download and, when allowed, a way to send files to the Ferry device.
// It only talks to the server it came from (relative URLs under the token).

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
    security_note: 'Use this link only on a network you trust: it is not encrypted.',
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
    security_note: 'Używaj tego linku tylko w zaufanej sieci: połączenie nie jest szyfrowane.',
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

function renderExpiry() {
  if (!expiresAt) { $('expires').textContent = ''; return; }
  const left = expiresAt - Date.now();
  $('expires').textContent = left > 0 ? t('expires', { min: Math.max(1, Math.ceil(left / 60000)) }) : t('expired');
}

async function load() {
  for (const node of document.querySelectorAll('[data-t]')) node.textContent = t(node.dataset.t);
  let res;
  try { res = await fetch('api/info', { cache: 'no-store' }); } catch { status(t('unreachable'), true); return; }
  if (!res.ok) { status(await reason(res), true); return; }
  info = await res.json();
  const device = info.device;
  $('title').textContent = t(info.files.length ? 'title' : 'title_receive_only', { device });
  $('subtitle').textContent = t('subtitle');
  document.title = `Ferry – ${device}`;
  if (info.files.length) {
    $('download').hidden = false;
    $('files').replaceChildren(...info.files.map((f) => {
      const li = el('li');
      const link = el('a', 'button', t('download'));
      link.href = `api/files/${f.id}`;
      link.setAttribute('download', f.name);
      li.append(el('span', 'name', f.name), link, el('span', 'size', fmtSize(f.size)));
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
    const res = await fetch('api/offer', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ files: files.map(f => ({ name: f.name, size: f.size })) }),
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
      r.state.textContent = '0%';
      const error = await put(`api/upload/${id}/${i}`, file, (p) => {
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
