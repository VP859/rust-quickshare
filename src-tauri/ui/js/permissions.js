// Android permissions, asked for only after Ferry said why: a card like the
// welcome card, with "Allow" (Android's own dialog follows) and "Not now".
// When Android no longer shows its dialog, the button opens the app settings.
// Desktop systems need none of this: permission_status answers null there.
import { $, el, fail } from './dom.js';
import { helpLink } from './help.js';
import { t } from './i18n.js';
import { invoke, isMobile } from './tauri.js';

/** The last answer of `permission_status`. */
let status = null;
/** "Not now" for this start of the app. */
let later = false;

/** The permissions that are missing: `local_network`, `notifications`, `storage`. */
const missing = () => ['local_network', 'notifications', 'storage'].filter((k) => status?.[k] === 'denied');

/** Whether Ferry may not reach the local network (Android 17+ without the permission). */
export const localNetworkDenied = () => status?.local_network === 'denied';

function render() {
  const wanted = missing();
  $('permissions').hidden = later || !wanted.length;
  if ($('permissions').hidden) return;
  $('permissions-list').replaceChildren(...wanted.map((k) => el('li', {}, t(`perm_${k}`))));
  const settings = status.can_ask === false;
  $('permissions-settings').hidden = !settings;
  $('btn-permissions-allow').textContent = t(settings ? 'perm_open_settings' : 'perm_allow');
}

async function refresh() {
  try { status = await invoke('permission_status'); } catch (e) { console.warn('Permissions:', e); }
  render();
  window.dispatchEvent(new Event('ferry-permission-status'));
}

$('btn-permissions-allow').onclick = async () => {
  try {
    await invoke(status?.can_ask === false ? 'open_app_settings' : 'request_permissions');
  } catch (e) { fail(e); }
};
$('btn-permissions-later').onclick = () => { later = true; render(); };

/** A line under Nearby while the local network is not allowed. */
export function permissionHint() {
  if (!localNetworkDenied()) return null;
  const allow = el('button', { class: 'link', onclick: () => { later = false; render(); $('btn-permissions-allow').click(); } }, t('perm_allow'));
  return el('p', { class: 'hint warn' }, t('perm_local_network_missing'), ' ', allow, ' ', helpLink('trouble'));
}

/** Reads the permissions on Android and follows their changes. */
export async function loadPermissions() {
  if (!isMobile || !invoke) return;
  await refresh();
  // Android's dialog was answered, or the user comes back from the settings.
  window.addEventListener('ferry-permissions', refresh);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
}
