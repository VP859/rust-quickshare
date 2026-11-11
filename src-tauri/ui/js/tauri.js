// Bridge to the native side (Tauri v2 with `withGlobalTauri`): commands,
// events and plugins, with fallbacks for plugins without a JS global.
import { toast } from './dom.js';
import { t } from './i18n.js';

export const T = window.__TAURI__;
export const invoke = T?.core?.invoke;
export const listen = T?.event?.listen;
export const isMobile = /Android/i.test(navigator.userAgent);

export async function copyText(text) {
  try {
    if (T?.clipboardManager?.writeText) await T.clipboardManager.writeText(text);
    else await invoke('plugin:clipboard-manager|write_text', { text });
  } catch { await navigator.clipboard.writeText(text); }
  toast(t('copied'));
}
// Folders are sent with their structure; the system picker returns a path.
export async function pickFolder() {
  const opts = { multiple: false, directory: true };
  const res = T?.dialog?.open ? await T.dialog.open(opts) : await invoke('plugin:dialog|open', { options: opts });
  return res ? [typeof res === 'string' ? res : res.path ?? String(res)] : [];
}
export async function pickFiles() {
  const opts = { multiple: true, directory: false };
  const res = T?.dialog?.open ? await T.dialog.open(opts) : await invoke('plugin:dialog|open', { options: opts });
  if (!res) return [];
  return (Array.isArray(res) ? res : [res]).map(f => typeof f === 'string' ? f : f.path ?? f.uri ?? String(f));
}
/** One file chosen in the system picker (a path, or a `content://` URI on Android); null when cancelled. */
export async function pickFile() {
  const opts = { multiple: false, directory: false };
  const res = T?.dialog?.open ? await T.dialog.open(opts) : await invoke('plugin:dialog|open', { options: opts });
  if (!res) return null;
  return typeof res === 'string' ? res : res.path ?? res.uri ?? String(res);
}
/** Where to save a new file (desktop "Save as" dialog); null when cancelled. */
export async function pickSavePath(defaultPath) {
  const opts = { defaultPath };
  const res = T?.dialog?.save ? await T.dialog.save(opts) : await invoke('plugin:dialog|save', { options: opts });
  return res ? String(res) : null;
}
/**
 * The QR scanner's camera permission (`granted`, `denied`, `prompt`, …); with
 * `request`, Android's dialog appears first when it still can.
 */
export async function cameraPermission(request = false) {
  const api = T?.barcodeScanner;
  if (api?.checkPermissions) return request ? api.requestPermissions() : api.checkPermissions();
  return (await invoke(`plugin:barcode-scanner|${request ? 'request' : 'check'}_permissions`))?.camera;
}
export async function scanQr() {
  const opts = { windowed: false, formats: ['QR_CODE'] };
  const res = T?.barcodeScanner?.scan ? await T.barcodeScanner.scan(opts) : await invoke('plugin:barcode-scanner|scan', opts);
  return res?.content?.trim();
}
