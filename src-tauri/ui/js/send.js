// Sending files: choosing a recipient, sharing by TICKET, receiving from a
// TICKET or QR code, and dropping files onto the window.
import { $, baseName, close, dotClass, el, emptyState, fail, icon, initials, open, preview, shortId, toast } from './dom.js';
import { t } from './i18n.js';
import { showTicket } from './node.js';
import { openPeerModal } from './peers.js';
import { isTrusted, platformIcon, platformLabel, state } from './state.js';
import { invoke, pickFiles, pickFolder, scanQr, T } from './tauri.js';
import { sendText } from './texts.js';
import { startWebShare } from './web.js';

export async function sendFiles(paths, target, label) {
  if (!paths.length) return;
  try {
    await invoke('send_files', { paths, target });
    const what = paths.length > 1 ? t('n_files', { n: paths.length }) : baseName(paths[0]);
    toast(t('sending', { what, name: label }));
  } catch (e) { fail(e); }
}

export async function sendToPeer(p) {
  try { await sendFiles(await pickFiles(), p.node_id, p.alias); } catch (e) { fail(e); }
}

// ---------- Choosing a recipient ----------
let pendingFiles = [];
let pendingText = null;
let pendingIsFolder = false;
function recipientRow(avatar, title, sub, onclick) {
  return el('button', { class: 'row', onclick },
    el('div', { class: 'avatar' }, avatar),
    el('div', { class: 'body' }, el('div', { class: 'title' }, title), el('div', { class: 'sub' }, sub)));
}
function deliver(nodeId, label) {
  close('modal-recipient');
  if (pendingText !== null) sendText(pendingText, nodeId, label);
  else sendFiles(pendingFiles, nodeId, label);
}
/** Asks where to send `paths` (or `text`) among trusted and nearby devices. */
export function chooseRecipient(paths, text = null, isFolder = false) {
  if (text === null && !paths.length) return;
  pendingIsFolder = isFolder;
  pendingFiles = paths;
  pendingText = text;
  $('recipient-files').textContent = text !== null ? t('text_to', { preview: preview(text) })
    : pendingIsFolder ? t('folder_label', { name: baseName(paths[0]) })
    : paths.length === 1 ? baseName(paths[0]) : t('n_files_list', { n: paths.length, list: paths.map(baseName).join(', ') });
  const box = $('recipients');
  box.replaceChildren();
  const { peers, online } = state;
  const unpaired = state.nearby.filter(d => !isTrusted(d.node_id));
  if (!peers.length && !unpaired.length) box.append(emptyState('inbox', t('no_devices')));
  for (const p of peers) {
    box.append(recipientRow([initials(p.alias), el('span', { class: dotClass(online.get(p.node_id)) })], p.alias, el('span', { class: 'mono' }, shortId(p.node_id)),
      () => deliver(p.node_id, p.alias)));
  }
  if (unpaired.length) box.append(el('div', { class: 'label' }, t('nearby_unpaired')));
  for (const d of unpaired) {
    box.append(recipientRow([icon(platformIcon(d.platform)), el('span', { class: 'dot ok' })], d.name, platformLabel(d.platform),
      () => deliver(d.node_id, d.name)));
  }
  // A TICKET shares exactly one file, so it is not offered for texts and folders.
  $('btn-share-ticket').style.display = text !== null || pendingIsFolder ? 'none' : '';
  $('btn-share-ticket').disabled = paths.length !== 1;
  $('btn-share-ticket').title = paths.length !== 1 ? t('ticket_one_file') : '';
  // A browser session shares files (any number), not texts or folders.
  $('btn-share-web').style.display = text !== null || pendingIsFolder ? 'none' : '';
  open('modal-recipient');
}
$('btn-share-web').onclick = () => {
  close('modal-recipient');
  startWebShare(pendingFiles);
};
$('btn-share-ticket').onclick = async () => {
  close('modal-recipient');
  try {
    const s = await invoke('share_file', { path: pendingFiles[0] });
    showTicket(t('shared_title', { name: s.file_name }), t('shared_desc'), s.ticket, s.qr_svg);
  } catch (e) { fail(e); }
};
$('btn-select-file').onclick = async () => { try { chooseRecipient(await pickFiles()); } catch (e) { fail(e); } };
$('btn-select-folder').onclick = async () => { try { chooseRecipient(await pickFolder(), null, true); } catch (e) { fail(e); } };

// ---------- Receiving from a TICKET / QR code ----------
async function handleScannedTicket(text) {
  if (!text) return;
  const info = await invoke('inspect_ticket', { ticket: text });
  if (info.has_share) {
    await invoke('receive_from_ticket', { ticket: text });
    toast(t('downloading'));
  } else {
    openPeerModal(null, text);
  }
}
$('btn-scan').onclick = async () => { try { await handleScannedTicket(await scanQr()); } catch (e) { fail(e); } };
$('btn-receive-ticket').onclick = () => { $('receive-input').value = ''; open('modal-receive'); $('receive-input').focus(); };
$('btn-receive-cancel').onclick = () => close('modal-receive');
$('btn-receive-go').onclick = async () => {
  try { await handleScannedTicket($('receive-input').value.trim()); close('modal-receive'); } catch (e) { fail(e); }
};

// ---------- Native drag and drop (Tauri) ----------
function targetAt(pos) {
  if (!pos) return null;
  const ratio = window.devicePixelRatio || 1;
  const node = document.elementFromPoint(pos.x / ratio, pos.y / ratio);
  return node?.closest('.device') || node?.closest('#dropzone') || null;
}
let hovered = null;
function setHover(target) {
  if (hovered === target) return;
  hovered?.classList.remove('drag-over');
  hovered = target;
  hovered?.classList.add('drag-over');
}
/** Files dropped on a device are sent to it, anywhere else they ask for a recipient. */
export async function setupDragDrop() {
  const webview = T?.webview?.getCurrentWebview?.();
  if (!webview?.onDragDropEvent) return;
  await webview.onDragDropEvent(({ payload }) => {
    if (payload.type === 'enter' || payload.type === 'over') {
      setHover(targetAt(payload.position) || $('dropzone'));
    } else if (payload.type === 'drop') {
      const target = targetAt(payload.position);
      setHover(null);
      const paths = payload.paths || [];
      const id = target?.dataset?.node;
      const peer = id && state.peers.find(p => p.node_id === id);
      const device = id && state.nearby.find(d => d.node_id === id);
      if (peer) sendFiles(paths, peer.node_id, peer.alias);
      else if (device) sendFiles(paths, device.node_id, device.name);
      else chooseRecipient(paths);
    } else {
      setHover(null);
    }
  });
}
