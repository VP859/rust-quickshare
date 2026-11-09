// Devices on the local network, pairing (both sides) and the default device
// name on Android.
import { $, close, el, emptyState, fail, icon, open, toast } from './dom.js';
import { t } from './i18n.js';
import { refreshPeers, renderPeers } from './peers.js';
import { helpLink } from './help.js';
import { isScanning, localSendRow, onLocalSendChange, scanLocalSend, sendToLocalSend } from './localsend.js';
import { permissionHint } from './permissions.js';
import { sendFiles } from './send.js';
import { isTrusted, platformIcon, platformLabel, state } from './state.js';
import { invoke, isMobile, listen, pickFiles } from './tauri.js';
import { composeText } from './texts.js';

export function setNearby(list) {
  state.nearby = list || [];
  for (const d of state.nearby) if (isTrusted(d.node_id)) state.online.set(d.node_id, true);
  renderNearby();
  renderPeers();
}

export function renderNearby() {
  const box = $('nearby');
  const { nearby, peers } = state;
  $('nearby-count').textContent = nearby.length;
  box.replaceChildren();
  updateDiscoveryHint();
  if (!nearby.length) {
    box.append(emptyState('wifi', t('searching_nearby'), helpLink('trouble', 'help_nearby_missing')));
    const hint = permissionHint();
    if (hint) box.append(hint);
    return;
  }
  for (const d of nearby) {
    const peer = peers.find(p => p.node_id === d.node_id);
    const send = el('button', { 'aria-label': t('send_to', { name: peer?.alias ?? d.name }), onclick: (e) => { e.stopPropagation(); sendToDevice(d.node_id, peer?.alias ?? d.name); } },
      icon('send', 'sm'), el('span', { class: 'label-text' }, t('send_file')));
    const text = el('button', { class: 'ghost icon', title: t('send_text'), 'aria-label': t('send_text'),
      onclick: (e) => { e.stopPropagation(); composeText(d.node_id, peer?.alias ?? d.name); } }, icon('text', 'sm'));
    const trailing = peer
      ? el('div', { class: 'trailing' }, text, send)
      : el('div', { class: 'trailing' }, text, send,
          el('button', { class: 'primary', onclick: (e) => { e.stopPropagation(); pairWith(d); } }, t('pair')));
    box.append(el('div', { class: 'row device', 'data-node': d.node_id },
      el('div', { class: 'avatar' }, icon(platformIcon(d.platform)), el('span', { class: 'dot ok' })),
      el('div', { class: 'body' },
        el('div', { class: 'title' }, d.name),
        el('div', { class: 'sub' }, platformLabel(d.platform), peer ? [d.platform ? ' · ' : '', el('span', { class: 'tag' }, t('paired_as', { name: peer.alias }))] : '')),
      trailing));
  }
}

/** LocalSend apps below the Ferry devices, with a warning that their names are not verified. */
function renderLocalSendNearby() {
  const block = $('nearby-localsend-block');
  const box = $('nearby-localsend');
  box.replaceChildren();
  block.hidden = !state.localsend.length && !isScanning();
  $('localsend-count').textContent = state.localsend.length;
  if (block.hidden) return;
  if (!state.localsend.length) box.append(emptyState('wifi', t('localsend_searching')));
  for (const d of state.localsend) {
    const send = el('button', { 'aria-label': t('send_to', { name: d.alias }), onclick: async () => { try { await sendToLocalSend(await pickFiles(), d); } catch (e) { fail(e); } } },
      icon('send', 'sm'), el('span', { class: 'label-text' }, t('send_file')));
    box.append(localSendRow(d, null, send));
  }
}
onLocalSendChange(renderLocalSendNearby);

export async function updateDiscoveryHint() {
  let hint = null;
  try { hint = await invoke('discovery_health'); } catch {}
  $('nearby-hint').replaceChildren(...(hint ? [hint, ' ', helpLink('trouble')] : []));
  $('nearby-hint').hidden = !hint;
}

async function sendToDevice(nodeId, label) {
  try { await sendFiles(await pickFiles(), nodeId, label); } catch (e) { fail(e); }
}

// ---------- Pairing started here ----------
let pairingWith = null;
async function pairWith(d) {
  try {
    pairingWith = d.node_id;
    $('pairing-title').textContent = t('pairing_with', { name: d.name });
    // The code depends on random values exchanged with the other device, so
    // it arrives with the `pairing-code` event once the connection is up.
    $('pairing-code').textContent = '––– –––';
    $('pairing-status').textContent = t('pairing_connecting');
    open('modal-pairing');
    const alias = await invoke('pair_device', { nodeId: d.node_id, name: d.name });
    await refreshPeers();
    renderNearby();
    if (pairingWith === d.node_id) {
      $('pairing-status').textContent = t('paired_status', { name: alias });
      setTimeout(() => close('modal-pairing'), 1500);
    }
    toast(t('paired_with', { name: alias }));
  } catch (e) {
    if (pairingWith === d.node_id) $('pairing-status').textContent = t('not_paired', { error: String(e?.message ?? e) });
    else fail(e);
  }
}
$('btn-pairing-close').onclick = () => { pairingWith = null; close('modal-pairing'); };
// Searches for Ferry devices and LocalSend apps (which hear this device's name).
$('btn-nearby-refresh').onclick = async () => {
  try { await invoke('refresh_nearby'); toast(t('searching')); } catch (e) { fail(e); }
  scanLocalSend();
};

// ---------- Pairing requests from other devices ----------
export const pairRequests = [];
export function showNextPairRequest() {
  const r = pairRequests[0];
  if (!r) return close('modal-pair-request');
  $('pair-req-name').textContent = r.name;
  $('pair-req-code').textContent = r.code;
  $('pair-req-queue').textContent = pairRequests.length > 1 ? t('queued', { n: pairRequests.length - 1 }) : '';
  open('modal-pair-request');
}
async function answerPairRequest(accept) {
  const r = pairRequests.shift();
  if (r) {
    try {
      await invoke('respond_to_offer', { transferId: r.id, accept });
      if (accept) toast(t('paired_with', { name: r.name }));
    } catch (e) { fail(e); }
  }
  showNextPairRequest();
}
$('btn-pair-accept').onclick = () => answerPairRequest(true);
$('btn-pair-reject').onclick = () => answerPairRequest(false);

/** Subscribes to the discovery and pairing events of the backend. */
export async function listenNearbyAndPairing() {
  window.addEventListener('ferry-permission-status', renderNearby);
  await listen('nearby', ({ payload }) => setNearby(payload));
  await listen('peers-changed', async () => { await refreshPeers(); renderNearby(); });
  // Also emitted by the background pairing after adding a device by TICKET,
  // which has no dialog open yet.
  await listen('pairing-code', ({ payload }) => {
    if (pairingWith !== payload.node_id) {
      pairingWith = payload.node_id;
      $('pairing-title').textContent = t('pairing_with', { name: payload.name });
      open('modal-pairing');
    }
    $('pairing-code').textContent = payload.code;
    $('pairing-status').textContent = t('waiting_confirmation');
  });
  await listen('pair-result', async ({ payload }) => {
    if (pairingWith === payload.node_id) {
      $('pairing-status').textContent = payload.message;
      if (payload.ok) setTimeout(() => close('modal-pairing'), 1500);
    }
    toast(payload.message, !payload.ok);
    await refreshPeers();
    renderNearby();
  });
  await listen('pair-request', ({ payload }) => {
    pairRequests.push(payload);
    if (pairRequests.length === 1) showNextPairRequest();
    else $('pair-req-queue').textContent = t('queued', { n: pairRequests.length - 1 });
  });
}

// ---------- Device name ----------
/** The Android host name is "localhost", so the phone model is used as the default device name. */
async function detectDeviceModel() {
  try {
    const v = await navigator.userAgentData?.getHighEntropyValues?.(['model']);
    if (v?.model) return v.model;
  } catch {}
  const m = navigator.userAgent.match(/Android [^;]+; ([^;)]+)[;)]/);
  const model = m?.[1]?.trim();
  return model && model !== 'K' && model !== 'wv' ? model : null;
}
export async function ensureDeviceName() {
  try {
    const dn = await invoke('get_device_name');
    if (dn.is_default && isMobile) {
      const model = await detectDeviceModel();
      if (model) await invoke('set_device_name', { name: model });
    }
  } catch (e) { console.warn('Device name:', e); }
}
