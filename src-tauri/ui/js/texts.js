// Texts and links: composing and sending one, and texts received from other
// devices (copied to the clipboard after confirmation).
import { $, close, el, fail, icon, isLink, open, preview, shortId, toast } from './dom.js';
import { t } from './i18n.js';
import { askPin } from './localsend.js';
import { chooseRecipient } from './send.js';
import { copyText, invoke, isMobile, listen, T } from './tauri.js';
import { addRow } from './transfers.js';

/** Largest text the backend accepts (`MAX_TEXT_LEN`). */
const MAX_TEXT_BYTES = 32 * 1024;

let textTarget = null;
export function composeText(nodeId = null, label = null, prefill = '') {
  textTarget = nodeId ? { nodeId, label } : null;
  $('text-input').value = prefill;
  $('btn-text-next').textContent = textTarget ? t('send_text') : t('choose_recipient_btn');
  open('modal-text-send');
  $('text-input').focus();
}
$('btn-send-text').onclick = () => composeText();
$('btn-text-cancel').onclick = () => close('modal-text-send');
$('btn-text-paste').onclick = async () => {
  try {
    const text = T?.clipboardManager?.readText ? await T.clipboardManager.readText() : await invoke('plugin:clipboard-manager|read_text');
    if (text) $('text-input').value = text; else toast(t('clipboard_empty'), true);
  } catch { toast(t('clipboard_empty'), true); }
};
$('btn-text-next').onclick = () => {
  const text = $('text-input').value;
  if (!text.trim()) return toast(t('text_empty'), true);
  if (new TextEncoder().encode(text).length > MAX_TEXT_BYTES) return toast(t('text_too_long'), true);
  close('modal-text-send');
  if (textTarget) sendText(text, textTarget.nodeId, textTarget.label);
  else chooseRecipient([], text);
};

/** A text row in the transfers list. */
function addTextRow(entry) {
  const row = { el: el('div', { class: 'row transfer' }), text: entry };
  row.render = () => renderTextRow(row);
  return addRow(row);
}
function renderTextRow(row) {
  const x = row.text;
  row.el.className = 'row transfer' + (x.state === 'done' ? ' done' : x.state === 'failed' ? ' failed' : '');
  const detail = x.state === 'sending' ? t('text_sending', { name: x.label })
    : x.state === 'failed' ? x.error
    : x.dir === 'upload' ? (x.done ?? t('text_delivered', { name: x.label }))
    : x.copied ? t('text_copied') : t('text_not_copied');
  const trailing = el('div', { class: 'trailing' },
    el('button', { class: 'ghost icon', title: t('copy'), 'aria-label': t('copy'), onclick: () => copyText(x.text) }, icon('copy', 'sm')));
  if (!isMobile && isLink(x.text)) {
    trailing.prepend(el('button', { onclick: () => invoke('open_link', { url: x.text.trim() }).catch(fail) }, t('open_link')));
  }
  row.el.replaceChildren(
    el('div', { class: 'dir' }, icon('text', 'sm')),
    el('div', { class: 'body' },
      el('div', { class: 'meta' }, el('span', { class: 'title' }, preview(x.text, 80)),
        el('span', { class: 'muted small' }, (x.dir === 'upload' ? '↑ ' : '↓ ') + x.label)),
      el('div', { class: 'detail' }, detail)),
    trailing);
}

export async function sendText(text, nodeId, label) {
  const row = addTextRow({ dir: 'upload', text, label, state: 'sending' });
  try {
    await invoke('send_text', { text, target: nodeId });
    row.text.state = 'done';
  } catch (e) {
    row.text.state = 'failed';
    row.text.error = String(e?.message ?? e);
    fail(e);
  }
  row.render();
}

/**
 * Sends `text` to the LocalSend app `device`, which shows it as a message;
 * asks for a PIN when the app needs one (the same row follows the retry).
 */
export async function sendTextToLocalSend(text, device, pin = null, row = null) {
  row ??= addTextRow({ dir: 'upload', text, label: device.alias, state: 'sending' });
  const end = (state, extra) => { Object.assign(row.text, { state }, extra); row.render(); };
  end('sending');
  try {
    const answer = await invoke('send_text_to_localsend', { text, fingerprint: device.fingerprint, pin });
    if (answer.status === 'sent') end('done', { done: t('text_shown', { name: device.alias }) });
    else if (answer.status === 'rejected') end('failed', { error: t('text_declined', { name: device.alias }) });
    else {
      askPin({
        alias: device.alias,
        wrong: answer.wrong,
        retry: (next) => sendTextToLocalSend(text, device, next, row),
        cancel: () => end('failed', { error: t('text_not_sent') }),
      });
    }
  } catch (e) {
    end('failed', { error: String(e?.message ?? e) });
    fail(e);
  }
}

// ---------- Texts from other devices ----------
export const textRequests = [];
export function showNextTextRequest() {
  const r = textRequests[0];
  if (!r) return close('modal-text-in');
  $('text-in-from').textContent = t('text_from', { name: r.alias ?? t('unknown_device', { id: shortId(r.peer) }) });
  $('text-in-body').textContent = r.text;
  $('text-in-queue').textContent = textRequests.length > 1 ? t('queued', { n: textRequests.length - 1 }) : '';
  open('modal-text-in');
}
async function answerTextRequest(accept) {
  const r = textRequests.shift();
  if (r) { try { await invoke('respond_to_offer', { transferId: r.id, accept }); } catch (e) { fail(e); } }
  showNextTextRequest();
}
$('btn-text-accept').onclick = () => answerTextRequest(true);
$('btn-text-reject').onclick = () => answerTextRequest(false);

function onTextReceived(x) {
  const label = x.alias ?? shortId(x.peer);
  addTextRow({ dir: 'download', text: x.text, label, state: 'done', copied: x.copied });
  toast(x.copied ? t('text_received_toast', { name: label }) : t('text_not_copied'), !x.copied);
}

/** Subscribes to incoming texts. */
export async function listenTexts() {
  await listen('incoming-text', ({ payload }) => {
    textRequests.push(payload);
    if (textRequests.length === 1) showNextTextRequest();
    else $('text-in-queue').textContent = t('queued', { n: textRequests.length - 1 });
  });
  await listen('text-received', ({ payload }) => onTextReceived(payload));
}
