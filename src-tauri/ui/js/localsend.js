// LocalSend apps (iPhone, Mac, …): the list of those heard on the network,
// searching for them, sending files to them and asking for their PIN (also
// for texts, see texts.js).
// Their names are not verified, so every place that lists them says so.
import { $, baseName, close, el, fail, icon, open, toast } from './dom.js';
import { t } from './i18n.js';
import { state } from './state.js';
import { invoke, listen } from './tauri.js';

const renderers = [];
/** Calls `render` whenever the list of LocalSend apps changes. */
export function onLocalSendChange(render) { renderers.push(render); }
function changed() { for (const render of renderers) render(); }

/** The LocalSend apps heard so far, without announcing this device. */
export async function loadLocalSend() {
  try { state.localsend = await invoke('list_localsend'); } catch (e) { console.warn('LocalSend:', e); }
  changed();
}

let scanning = false;
/** Announces this device to LocalSend apps and lists those that answer. */
export async function scanLocalSend() {
  if (scanning) return;
  scanning = true;
  changed();
  try { state.localsend = await invoke('scan_localsend'); } catch (e) { fail(e); }
  scanning = false;
  changed();
}
export const isScanning = () => scanning;

/** A LocalSend app as a list row; `onclick` runs when it is chosen. */
export function localSendRow(d, onclick, ...trailing) {
  const body = el('div', { class: 'body' }, el('div', { class: 'title' }, d.alias),
    el('div', { class: 'sub' }, el('span', { class: 'tag unverified' }, t('localsend_tag')), ' · ',
      d.model || 'LocalSend', ' · ', d.address.replace(/:\d+$/, '')));
  const avatar = el('div', { class: 'avatar' }, icon(d.device_type === 'mobile' ? 'phone' : 'monitor'));
  return onclick
    ? el('div', { class: 'recipient' }, el('button', { class: 'row', onclick }, avatar, body))
    : el('div', { class: 'row device localsend' }, avatar, body, el('div', { class: 'trailing' }, ...trailing));
}

/** Sends `paths` to the LocalSend app `device`; a PIN is asked for when it needs one. */
export async function sendToLocalSend(paths, device, pin = null) {
  if (!paths.length) return;
  try {
    await invoke('send_to_localsend', { paths, fingerprint: device.fingerprint, pin });
    const what = paths.length > 1 ? t('n_files', { n: paths.length }) : baseName(paths[0]);
    toast(t('sending', { what, name: device.alias }));
  } catch (e) { fail(e); }
}

// ---------- PIN ----------
/** The open PIN question: `retry(pin)` sends again, `cancel()` gives up. */
let pinRequest = null;
/** Asks for the PIN of the LocalSend app `alias` (`wrong`: the last one was refused). */
export function askPin({ alias, wrong, retry, cancel = () => {} }) {
  pinRequest = { retry, cancel };
  $('localsend-pin-title').textContent = t('localsend_pin_title', { name: alias });
  $('localsend-pin-wrong').hidden = !wrong;
  $('localsend-pin').value = '';
  open('modal-localsend-pin');
  $('localsend-pin').focus();
}
$('btn-pin-cancel').onclick = () => {
  pinRequest?.cancel();
  pinRequest = null;
  close('modal-localsend-pin');
};
$('btn-pin-send').onclick = () => {
  const pin = $('localsend-pin').value.trim();
  if (!pin || !pinRequest) return;
  const { retry } = pinRequest;
  pinRequest = null;
  close('modal-localsend-pin');
  retry(pin);
};
$('localsend-pin').onkeydown = (e) => { if (e.key === 'Enter') $('btn-pin-send').click(); };

/** Subscribes to the LocalSend events of the backend. */
export async function listenLocalSend() {
  await listen('localsend-nearby', ({ payload }) => { state.localsend = payload || []; changed(); });
  await listen('localsend-pin', ({ payload }) => {
    const { paths, fingerprint, alias, wrong } = payload;
    askPin({ alias, wrong, retry: (pin) => sendToLocalSend(paths, { fingerprint, alias }, pin) });
  });
  await loadLocalSend();
}
