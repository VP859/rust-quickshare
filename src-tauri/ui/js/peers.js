// Trusted devices: the list, the device menu, the safety number and the
// add/edit dialog.
import { $, close, dotClass, el, emptyState, fail, icon, initials, open, toast } from './dom.js';
import { t } from './i18n.js';
import { sendToPeer } from './send.js';
import { state } from './state.js';
import { copyText, invoke, scanQr } from './tauri.js';
import { composeText } from './texts.js';

let openMenu = null;
function closeMenu() {
  openMenu?.closest('.list')?.classList.remove('menu-open');
  openMenu?.remove();
  openMenu = null;
}
document.addEventListener('click', closeMenu);

export async function refreshPeers() {
  try { state.peers = await invoke('list_peers'); } catch (e) { fail(e); state.peers = []; }
  renderPeers();
}

export function renderPeers() {
  const box = $('peers');
  const { peers, online } = state;
  $('peers-count').textContent = peers.length;
  closeMenu();
  box.replaceChildren();
  if (!peers.length) {
    box.append(emptyState('shield', t('no_peers')));
    return;
  }
  for (const p of peers) {
    const reachable = online.get(p.node_id);
    const status = reachable === true ? t('peer_online') : reachable === false ? t('peer_offline') : t('peer_checking');
    const label = t('send_to', { name: p.alias });
    const sendBtn = el('button', { class: 'ghost icon', 'aria-label': label, title: label,
      onclick: (e) => { e.stopPropagation(); sendToPeer(p); } }, icon('send', 'sm'));
    const menuBtn = el('button', { class: 'ghost icon', 'aria-label': t('device_options'), title: t('device_options'),
      onclick: (e) => { e.stopPropagation(); showMenu(row, p); } }, icon('more'));
    // Clicking anywhere on the row sends, as a shortcut for the mouse; the
    // keyboard and screen readers use the send button, the row itself is no
    // control (a control holding other controls cannot be announced well).
    const row = el('div', { class: 'row clickable device', 'data-node': p.node_id, onclick: () => sendToPeer(p) },
      el('div', { class: 'avatar' }, initials(p.alias), el('span', { class: dotClass(reachable) })),
      el('div', { class: 'body' },
        el('div', { class: 'title' }, p.alias),
        el('div', { class: 'sub', title: p.node_id }, status,
          p.auto_accept ? [' · ', el('span', { class: 'tag' }, 'Auto-Accept')] : '',
          p.verified ? [' · ', el('span', { class: 'tag verified', title: t('safety_verified') }, `✓ ${t('verified_tag')}`)] : '')),
      el('div', { class: 'trailing' }, sendBtn, menuBtn));
    box.append(row);
  }
}

export async function checkOnline() {
  await Promise.all(state.peers.map(async (p) => {
    try { state.online.set(p.node_id, await invoke('peer_online', { nodeId: p.node_id })); }
    catch { state.online.set(p.node_id, false); }
    renderPeers();
  }));
}

// The safety number shared with a trusted device: equal on both devices, so
// comparing it (reading it out, side by side) shows nobody is in between.
let safetyPeer = null;
async function showSafetyNumber(p) {
  const info = await invoke('safety_number', { nodeId: p.node_id });
  safetyPeer = p.node_id;
  $('safety-title').textContent = t('safety_title', { name: p.alias });
  $('safety-number').replaceChildren(...info.number.split(' ').map((group) => el('span', {}, group)));
  $('safety-verified').checked = info.verified;
  open('modal-safety');
}
$('safety-verified').onchange = async (e) => {
  try {
    await invoke('set_verified', { nodeId: safetyPeer, verified: e.target.checked });
    await refreshPeers();
  } catch (err) { fail(err); e.target.checked = !e.target.checked; }
};
$('btn-safety-copy').onclick = () => copyText($('safety-number').textContent.replace(/(\d{5})(?=\d)/g, '$1 '));

function showMenu(row, p) {
  closeMenu();
  const item = (iconName, label, fn, cls = '') => el('button', { class: cls, role: 'menuitem', onclick: async (e) => { e.stopPropagation(); closeMenu(); try { await fn(); } catch (err) { fail(err); } } }, icon(iconName, 'sm'), label);
  openMenu = el('div', { class: 'menu', role: 'menu', onclick: (e) => e.stopPropagation() },
    item('send', t('send_file_menu'), () => sendToPeer(p)),
    item('text', t('send_text_menu'), () => composeText(p.node_id, p.alias)),
    item('sliders', t('edit_peer'), () => openPeerModal(p)),
    item('shield', p.auto_accept ? t('auto_accept_off') : t('auto_accept_on'), async () => {
      await invoke('update_peer', { nodeId: p.node_id, alias: null, autoAccept: !p.auto_accept });
      await refreshPeers();
      toast(t(p.auto_accept ? 'auto_accept_disabled' : 'auto_accept_enabled', { name: p.alias }));
    }),
    item('shield', t('safety_number'), () => showSafetyNumber(p)),
    item('code', t('copy_node_id'), () => copyText(p.node_id)),
    el('hr'),
    item('x', t('remove_device'), async () => {
      if (!confirm(t('confirm_remove', { name: p.alias }))) return;
      await invoke('remove_peer', { nodeId: p.node_id });
      state.online.delete(p.node_id);
      await refreshPeers();
      toast(t('device_removed'));
    }, 'danger'));
  row.closest('.list').classList.add('menu-open');
  row.append(openMenu);
}

let editing = null;
export function openPeerModal(peer = null, prefill = '') {
  editing = peer;
  $('peer-title').textContent = peer ? t('edit_title', { name: peer.alias }) : t('add_device');
  $('peer-input-field').style.display = peer ? 'none' : '';
  $('peer-pair-hint').style.display = peer ? 'none' : '';
  $('peer-input').value = prefill;
  $('peer-alias').value = peer?.alias ?? '';
  $('peer-auto').checked = peer?.auto_accept ?? false;
  open('modal-peer');
  (peer ? $('peer-alias') : $('peer-input')).focus();
}
$('btn-add-peer').onclick = () => openPeerModal();
$('btn-peer-cancel').onclick = () => close('modal-peer');
$('btn-peer-scan').onclick = async () => { try { const v = await scanQr(); if (v) $('peer-input').value = v; } catch (e) { fail(e); } };
$('btn-peer-save').onclick = async () => {
  const alias = $('peer-alias').value.trim();
  const autoAccept = $('peer-auto').checked;
  try {
    if (editing) {
      await invoke('update_peer', { nodeId: editing.node_id, alias, autoAccept });
      toast(t('changes_saved'));
    } else {
      const input = $('peer-input').value.trim();
      if (!input) return toast(t('paste_ticket'), true);
      const id = await invoke('add_peer', { input, alias, autoAccept });
      state.online.delete(id);
      toast(t('device_added'));
    }
    close('modal-peer');
    await refreshPeers();
    checkOnline();
  } catch (e) { fail(e); }
};
