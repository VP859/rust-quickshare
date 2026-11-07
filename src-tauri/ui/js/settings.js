// The settings dialog: device name, download folder, network mode, privacy,
// language, autostart and close action, Android background limits, about.
import { $, close, el, fail, open, toast } from './dom.js';
import { lang, setLang, t } from './i18n.js';
import { updateDiscoveryHint } from './nearby.js';
import { loadNodeInfo } from './node.js';
import { state } from './state.js';
import { invoke, isMobile, T } from './tauri.js';

// ---------- Network mode ----------
// Always visible in the top bar: whether anything may go through external
// servers (the mode the node actually runs in) and whether the device is hidden.
function renderModePill() {
  const { network, privacy } = state;
  const pill = $('mode-pill');
  const parts = [t(`mode_${network.saved}`)];
  if (network.saved !== network.active) parts.push(t('mode_pending'));
  if (privacy.hidden) parts.push(t('mode_hidden'));
  pill.textContent = parts.join(' · ');
  pill.title = t('mode_hint');
  pill.classList.toggle('internet', network.saved !== 'local');
  pill.hidden = false;
}
/** Shows the mode (`mode`, or the saved one) and, for `relay`, the relay field. */
export function renderNetwork(mode = state.network.saved) {
  const { network } = state;
  renderModePill();
  $('network-mode').value = mode;
  $('network-desc').textContent = t(`network_${mode}_desc`);
  $('network-pending').hidden = network.saved === network.active;
  $('relay-field').hidden = mode !== 'relay';
  if (document.activeElement !== $('relay-urls')) $('relay-urls').value = (network.relays ?? []).join('\n');
}
export async function loadNetwork() {
  try { state.network = await invoke('get_network_mode'); } catch (e) { console.warn('Network mode:', e); }
  renderNetwork();
}
async function saveNetwork(mode, relays = null) {
  try {
    await invoke('set_network_mode', { mode, relays });
    state.network.saved = mode;
    if (relays) state.network.relays = relays;
  } catch (err) { fail(err); renderNetwork(); return; }
  renderNetwork();
  if (state.network.saved !== state.network.active) open('modal-restart');
}
$('network-mode').onchange = (e) => {
  // The relay mode needs a relay first: it is saved with the address.
  if (e.target.value === 'relay' && !(state.network.relays ?? []).length) {
    renderNetwork('relay');
    $('relay-urls').focus();
    return;
  }
  saveNetwork(e.target.value);
};
$('btn-relay-save').onclick = () => {
  const relays = $('relay-urls').value.split(/[\s,]+/).filter(Boolean);
  if (!relays.length) { toast(t('relay_missing'), true); return; }
  saveNetwork('relay', relays);
};
$('btn-restart-later').onclick = () => close('modal-restart');
$('btn-restart-ok').onclick = () => invoke('restart_app').catch(fail);
$('btn-restart').onclick = () => invoke('restart_app').catch(fail);

// ---------- Privacy ----------
function showPrivacy() {
  const { privacy } = state;
  $('privacy-hidden').checked = privacy.hidden;
  $('privacy-texts').checked = privacy.auto_accept_texts;
  $('privacy-exe').checked = privacy.auto_accept_executables;
  $('privacy-localsend').checked = privacy.localsend;
  $('privacy-history').checked = privacy.keep_history;
  renderModePill();
}
export async function loadPrivacy() {
  try { state.privacy = await invoke('get_privacy'); } catch (e) { console.warn('Privacy:', e); }
  showPrivacy();
}
async function savePrivacy(change) {
  const next = { ...state.privacy, ...change };
  try {
    await invoke('set_privacy', { settings: next });
    state.privacy = next;
    updateDiscoveryHint();
  } catch (e) { fail(e); }
  showPrivacy();
}
$('privacy-hidden').onchange = (e) => savePrivacy({ hidden: e.target.checked });
$('privacy-texts').onchange = (e) => savePrivacy({ auto_accept_texts: e.target.checked });
$('privacy-exe').onchange = (e) => savePrivacy({ auto_accept_executables: e.target.checked });
$('privacy-localsend').onchange = (e) => savePrivacy({ localsend: e.target.checked });
$('privacy-history').onchange = (e) => savePrivacy({ keep_history: e.target.checked });

// ---------- Device key in the system's secret store ----------
async function loadKeyStorage() {
  try {
    const info = await invoke('get_key_storage');
    $('keyring-toggle').hidden = !info.supported;
    $('privacy-keyring').checked = info.system;
  } catch (e) { console.warn('Key storage:', e); }
}
$('privacy-keyring').onchange = async (e) => {
  const system = e.target.checked;
  e.target.disabled = true;
  try {
    await invoke('set_key_storage', { system });
    toast(t(system ? 'keyring_moved_system' : 'keyring_moved_file'));
  } catch (err) { e.target.checked = !system; fail(err); }
  e.target.disabled = false;
};

// ---------- Speed limit ----------
/** `2M`, `500K` or empty for no limit (bytes per second as typed by `parse_speed`). */
function speedText(bytes) {
  if (!bytes) return '';
  const mib = 1024 * 1024;
  return bytes % mib === 0 ? `${bytes / mib}M` : `${Math.round(bytes / 1024)}K`;
}
async function loadSpeedLimit() {
  try { $('speed-limit').value = speedText(await invoke('get_speed_limit')); } catch (e) { console.warn('Speed limit:', e); }
}
$('btn-speed-save').onclick = async () => {
  try {
    const limit = await invoke('set_speed_limit', { value: $('speed-limit').value.trim() || 'off' });
    $('speed-limit').value = speedText(limit);
    toast(limit ? t('speed_saved', { speed: speedText(limit) }) : t('speed_off'));
  } catch (e) { fail(e); }
};

// ---------- Language ----------
export async function loadLanguage() {
  try {
    const info = await invoke('get_language');
    setLang(info.code);
    $('language').replaceChildren(...info.available.map(l => el('option', { value: l.code }, l.name)));
    $('language').value = lang;
  } catch (e) { console.warn('Language:', e); }
}
/** Saves the language chosen in the settings and calls `rerender` to translate the page. */
export function onLanguageChange(rerender) {
  $('language').onchange = async (e) => {
    try {
      setLang(await invoke('set_language', { code: e.target.value }));
      rerender();
    } catch (err) { e.target.value = lang; fail(err); }
  };
}

// ---------- The dialog ----------
export async function loadAppInfo() {
  try {
    const info = await invoke('app_info');
    $('footer-version').textContent = `${info.name} ${info.version}`;
    $('about-version').textContent = `${info.name} ${info.version}`;
  } catch {}
}
const openRepository = () => invoke('open_repository').catch(fail);
$('link-repo').onclick = openRepository;
$('btn-repo').onclick = openRepository;
$('btn-settings').onclick = async () => {
  if (!state.nodeInfo) await loadNodeInfo();
  $('settings-node').textContent = state.nodeInfo?.node_id ?? '—';
  try { $('device-name').value = (await invoke('get_device_name')).name; } catch {}
  $('settings-dir').textContent = state.nodeInfo?.download_dir ?? '—';
  $('language').value = lang;
  await loadNetwork();
  await loadPrivacy();
  await loadBackgroundStatus();
  await loadSpeedLimit();
  await loadKeyStorage();
  if (!isMobile) { try { $('autostart').checked = await invoke('plugin:autostart|is_enabled'); } catch {} }
  if (!isMobile) { try { $('close-action').value = (await invoke('get_quit_on_close')) ? 'quit' : 'tray'; } catch {} }
  open('modal-settings');
};
// Android only: what may pause Ferry in the background, with a way to the
// system screen where it is changed. The section stays hidden on desktop.
async function loadBackgroundStatus() {
  let status = null;
  try { status = await invoke('background_status'); } catch (e) { console.warn('Background status:', e); }
  $('background-section').hidden = !status;
  if (!status) return;
  const row = (problem, key) => el('li', { class: problem ? 'warn' : 'ok' }, t(problem ? `bg_${key}` : `bg_${key}_ok`));
  $('background-status').replaceChildren(
    row(status.battery_optimized, 'battery'),
    row(status.background_restricted, 'restricted'),
    row(status.power_save, 'power_save'),
    row(status.data_saver, 'data_saver'),
  );
}
$('btn-app-settings').onclick = () => invoke('open_app_settings').catch(fail);
$('mode-pill').onclick = () => $('btn-settings').click();
$('close-action').onchange = async (e) => {
  try { await invoke('set_quit_on_close', { quit: e.target.value === 'quit' }); }
  catch (err) { fail(err); e.target.value = e.target.value === 'quit' ? 'tray' : 'quit'; }
};
$('btn-device-name-save').onclick = async () => {
  try { $('device-name').value = await invoke('set_device_name', { name: $('device-name').value }); toast(t('name_saved')); }
  catch (e) { fail(e); }
};
$('autostart').onchange = async (e) => {
  try { await invoke(e.target.checked ? 'plugin:autostart|enable' : 'plugin:autostart|disable'); toast(t('saved')); }
  catch (err) { e.target.checked = !e.target.checked; fail(err); }
};
$('btn-change-dir').onclick = async () => {
  try {
    const dir = T?.dialog?.open ? await T.dialog.open({ directory: true }) : await invoke('plugin:dialog|open', { options: { directory: true } });
    if (!dir) return;
    await invoke('set_download_dir', { path: String(dir) });
    await loadNodeInfo();
    $('settings-dir').textContent = state.nodeInfo?.download_dir ?? String(dir);
    toast(t('dir_changed'));
  } catch (e) { fail(e); }
};
