// The Ferry GUI: startup, backend events and the Android "Share" menu. The
// parts of the interface live in their own modules:
//   node.js       this device's status and TICKET
//   peers.js      trusted devices, device menu, safety number
//   nearby.js     devices on the local network, pairing, default device name
//   send.js       choosing a recipient, sharing by TICKET, drag and drop
//   texts.js      sending and receiving texts and links
//   transfers.js  the transfers list and incoming offers
//   settings.js   the settings dialog
//   state.js      state shared by them
import { fail, toast } from './dom.js';
import { applyStaticText, t } from './i18n.js';
import { ensureDeviceName, listenNearbyAndPairing, pairRequests, renderNearby, setNearby, showNextPairRequest, updateDiscoveryHint } from './nearby.js';
import { loadNodeInfo, renderNodeStatus } from './node.js';
import { checkOnline, refreshPeers, renderPeers } from './peers.js';
import { chooseRecipient, setupDragDrop } from './send.js';
import { loadAppInfo, loadLanguage, loadNetwork, loadPrivacy, onLanguageChange, renderNetwork } from './settings.js';
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
async function checkSharedFiles() {
  try {
    const files = await invoke('take_shared_files');
    if (files.length) return chooseRecipient(files);
    const text = await invoke('take_shared_text');
    if (text) composeText(null, null, text);
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
  await setupDragDrop();
  await refreshPeers();
  await ensureDeviceName();
  try { setNearby(await invoke('list_nearby')); } catch {}
  await loadNodeInfo();
  await loadNetwork();
  await loadPrivacy();
  checkOnline();
  setInterval(checkOnline, 60000);
  setInterval(updateDiscoveryHint, 5000);
  if (isMobile) checkSharedFiles();
});
