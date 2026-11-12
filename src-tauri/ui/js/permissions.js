// Android permissions. Android's dialog appears by itself on the first start
// (MainActivity), for the local network again when a search starts, and for
// the camera only when a QR code is scanned. Whatever is still missing is
// explained on a card like the welcome card, with "Allow" (Android's dialog
// follows) and "Not now"; when Android no longer shows its dialog, the button
// opens the app settings. Desktop systems need none of this:
// permission_status answers null there.
import { $, el, fail, toast } from './dom.js';
import { helpLink } from './help.js';
import { t } from './i18n.js';
import { cameraPermission, invoke, isMobile } from './tauri.js';

/** The last answer of `permission_status`. */
let status = null;
/** "Not now" for this start of the app. */
let later = false;
/** A scan found the camera refused for good (only the settings help). */
let camera = false;

/** The permissions that are missing: `local_network`, `notifications`, `storage`, `camera`. */
const missing = () => [...['local_network', 'notifications', 'storage'].filter((k) => status?.[k] === 'denied'), ...(camera ? ['camera'] : [])];
/** Android no longer shows its dialog for something missing. */
const settingsOnly = () => camera || status?.can_ask === false;

/** Whether Ferry may not reach the local network (Android 17+ without the permission). */
const localNetworkDenied = () => status?.local_network === 'denied';

function render() {
  const wanted = missing();
  // Hidden while Android's own dialog asks.
  $('permissions').hidden = later || !wanted.length || status?.asking === true;
  if ($('permissions').hidden) return;
  $('permissions-list').replaceChildren(...wanted.map((k) => el('li', {}, t(`perm_${k}`))));
  const settings = settingsOnly();
  $('permissions-settings').hidden = !settings;
  $('btn-permissions-allow').textContent = t(settings ? 'perm_open_settings' : 'perm_allow');
}

async function refresh() {
  try { status = await invoke('permission_status'); } catch (e) { console.warn('Permissions:', e); }
  if (camera) {
    try { camera = (await cameraPermission()) !== 'granted'; } catch (e) { console.warn('Camera permission:', e); }
  }
  render();
  window.dispatchEvent(new Event('ferry-permission-status'));
}

$('btn-permissions-allow').onclick = async () => {
  try {
    await invoke(settingsOnly() ? 'open_app_settings' : 'request_permissions');
  } catch (e) { fail(e); }
};
$('btn-permissions-later').onclick = () => { later = true; render(); };

/** A line under Nearby while the local network is not allowed. */
export function permissionHint() {
  if (!localNetworkDenied()) return null;
  const allow = el('button', { class: 'link', onclick: () => { later = false; render(); $('btn-permissions-allow').click(); } }, t('perm_allow'));
  return el('p', { class: 'hint warn' }, t('perm_local_network_missing'), ' ', allow, ' ', helpLink('trouble'));
}

/** Before a search: Android's dialog for the local network, or the card once Android no longer asks. */
export async function askLocalNetwork() {
  if (!localNetworkDenied()) return;
  if (status.can_ask === false) {
    later = false;
    render();
    return;
  }
  try { await invoke('request_permissions', { only: 'local_network' }); } catch (e) { fail(e); }
}

/**
 * Before a scan: whether the camera may be used, after Android's dialog when
 * it still shows one. Refused for good, the card leads to the app settings.
 */
export async function allowCamera() {
  const state = await cameraPermission();
  if (state === 'granted') return true;
  if (state !== 'denied' && (await cameraPermission(true)) === 'granted') return true;
  if (state !== 'denied') {
    toast(t('perm_camera_refused'), true);
    return false;
  }
  camera = true;
  later = false;
  render();
  $('btn-permissions-allow').focus();
  return false;
}

/** Reads the permissions on Android and follows their changes. */
export async function loadPermissions() {
  if (!isMobile || !invoke) return;
  await refresh();
  // Android's dialog was answered, or the user comes back from the settings.
  window.addEventListener('ferry-permissions', refresh);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
}
