// The Ferry GUI: startup, backend events and the Android "Share" menu. The
// parts of the interface live in their own modules:
//   node.js       this device's status and TICKET
//   peers.js      trusted devices, device menu, safety number
//   nearby.js     devices on the local network, pairing, default device name
//   send.js       choosing a recipient, sharing by TICKET, drag and drop
//   localsend.js  LocalSend apps (iPhone, Mac): list, search, sending, PIN
//   help.js       the help window and the welcome card for new users
//   permissions.js Android permissions, asked for after saying why
//   texts.js      sending and receiving texts and links
//   transfers.js  the transfers list and incoming offers
//   settings.js   the settings dialog
//   web.js        sessions with a web browser (link and QR code)
//   history.js    the history of finished transfers
//   state.js      state shared by them
import { fail, toast } from './dom.js';
import './history.js';
import { applyStaticText, t } from './i18n.js';
import { ensureDeviceName, listenNearbyAndPairing, pairRequests, renderNearby, setNearby, showNextPairRequest, updateDiscoveryHint } from './nearby.js';
import { loadWelcome } from './help.js';
import { listenLocalSend } from './localsend.js';
import { loadPermissions } from './permissions.js';
import { loadNodeInfo, renderNodeStatus } from './node.js';
import { checkOnline, refreshPeers, renderPeers } from './peers.js';
import { chooseRecipient, sendFiles, setupDragDrop } from './send.js';
import { loadAppInfo, loadLanguage, loadNetwork, loadPrivacy, onLanguageChange, renderNetwork } from './settings.js';
import { state } from './state.js';
import { invoke, isMobile, listen } from './tauri.js';
import { composeText, listenTexts, showNextTextRequest, textRequests } from './texts.js';
import { listenTransfers, offers, renderNoTransfers, rerenderTransfers, showNextOffer } from './transfers.js';

if (isMobile) document.body.classList.add('mobile');

/** Translates everything on the page after a language change. */
function rerender() {
  applyStaticText();
  renderNodeStatus();
  renderPeers();
  renderNearby();
  rerenderTransfers();
  if (pairRequests.length) showNextPairRequest();
  if (textRequests.length) showNextTextRequest();
  renderNetwork();
}
onLanguageChange(rerender);

// ---------- Android integration ----------
/** The paired device chosen in the Android share menu (Direct Share), if any and still paired. */
async function shareTarget() {
  const target = await invoke('take_share_target');
  if (!target) return null;
  const find = () => state.peers.find((p) => p.node_id === target);
  // Started by the share menu: the list may not be loaded yet.
  if (!find()) await refreshPeers();
  return find() ?? null;
}
async function checkSharedFiles() {
  try {
    const files = await invoke('take_shared_files');
    const text = files.length ? null : await invoke('take_shared_text');
    if (!files.length && !text) return;
    const peer = await shareTarget();
    // Files go to the chosen device at once; a text opens ready to send to it.
    if (files.length) return peer ? sendFiles(files, peer.node_id, peer.alias) : chooseRecipient(files);
    composeText(peer?.node_id ?? null, peer?.alias ?? null, text);
  } catch (e) { fail(e); }
}
window.addEventListener('ferry-share', checkSharedFiles);
window.addEventListener('ferry-lan-permission', async () => {
  try { await invoke('refresh_nearby'); toast(t('lan_granted')); }
  catch (e) { fail(e); }
});

// ---------- Startup ----------
document.addEventListener('DOMContentLoaded', async () => {
  applyStaticText();
  renderNodeStatus();
  renderNoTransfers();
  renderPeers();
  if (!invoke) { toast(t('no_tauri'), true); return; }
  await loadLanguage();
  applyStaticText();
  renderNodeStatus();
  renderNoTransfers();
  loadAppInfo();
  await listenTransfers();
  await listen('node-error', ({ payload }) => fail(payload));
  await listenTexts();
  // A request answered or expired in the backend leaves its queue.
  await listen('request-cancelled', ({ payload }) => {
    for (const [queue, show, expired] of [[offers, showNextOffer, null], [pairRequests, showNextPairRequest, 'pair_expired'], [textRequests, showNextTextRequest, 'text_expired']]) {
      const i = queue.findIndex(r => r.id === payload);
      if (i < 0) continue;
      queue.splice(i, 1);
      if (i === 0) show();
      if (expired) toast(t(expired), true);
    }
  });
  await listenNearbyAndPairing();
  await listenLocalSend();
  await setupDragDrop();
  await refreshPeers();
  await loadWelcome();
  await loadPermissions();
  await ensureDeviceName();
  try { setNearby(await invoke('list_nearby')); } catch {}
  await loadNodeInfo();
  await loadNetwork();
  await loadPrivacy();
  checkOnline();
  setInterval(checkOnline, 60000);
  setInterval(updateDiscoveryHint, 5000);
  // Files from the Android "Share" menu or from "Send with Ferry" in a file manager.
  await listen('share-files', checkSharedFiles);
  // "Send the clipboard" in the tray menu: the text, ready for a recipient.
  await listen('send-clipboard', ({ payload }) => {
    if (payload) composeText(null, null, payload);
    else toast(t('clipboard_empty'), true);
  });
  checkSharedFiles();
});
