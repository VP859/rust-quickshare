// A password-protected backup of this device (its key and paired devices),
// so a new phone or a reinstalled computer keeps every pairing. The backup
// itself is made and read by the backend (ferry::node::backup); this asks for
// the password, where to save it (desktop) and which file to restore.
import { $, close, fail, open, toast } from './dom.js';
import { t } from './i18n.js';
import { askRestart } from './settings.js';
import { invoke, isMobile, pickFile, pickSavePath } from './tauri.js';

/** Shortest password the backend accepts (`backup::MIN_PASSWORD_LEN`). */
const MIN_PASSWORD = 8;

/** `create`, or `{ path }` of the backup to restore. */
let mode = null;

function showError(text) {
  $('backup-error').hidden = !text;
  $('backup-error').textContent = text ?? '';
}

function openDialog(next) {
  mode = next;
  const creating = next === 'create';
  $('backup-modal-title').textContent = t(creating ? 'backup_create_title' : 'backup_restore_title');
  $('backup-modal-text').textContent = creating ? t(isMobile ? 'backup_create_text_mobile' : 'backup_create_text')
    : t('backup_restore_text', { name: next.path.split(/[\\/]/).pop() });
  $('backup-confirm-field').hidden = !creating;
  $('backup-password').autocomplete = creating ? 'new-password' : 'current-password';
  $('backup-password').value = '';
  $('backup-confirm').value = '';
  $('btn-backup-go').textContent = t(creating ? 'backup_create_go' : 'backup_restore_go');
  $('btn-backup-go').disabled = false;
  showError(null);
  open('modal-backup');
  $('backup-password').focus();
}

$('btn-backup-create').onclick = () => openDialog('create');
$('btn-backup-restore').onclick = async () => {
  try {
    const path = await pickFile();
    if (path) openDialog({ path });
  } catch (e) { fail(e); }
};

/** Makes the backup: on the desktop where the user chooses, on Android in Download/Ferry. */
async function create(password) {
  if (password.length < MIN_PASSWORD) return showError(t('backup_password_short', { min: MIN_PASSWORD }));
  if (password !== $('backup-confirm').value) return showError(t('backup_password_mismatch'));
  let path = null;
  if (!isMobile) {
    const day = new Date().toISOString().slice(0, 10);
    path = await pickSavePath(`ferry-backup-${day}.ferry-backup`);
    if (!path) return;
  }
  const saved = await invoke('create_backup', { password, path });
  close('modal-backup');
  toast(t('backup_saved', { path: saved }));
}

/** Restores the chosen backup; a new key needs a restart of Ferry. */
async function restore(password) {
  const result = await invoke('restore_backup', { path: mode.path, password });
  close('modal-backup');
  toast(t('backup_restored', { n: result.peers_added }));
  if (result.key_changed) askRestart('backup');
}

$('backup-form').onsubmit = async (e) => {
  e.preventDefault();
  const password = $('backup-password').value;
  showError(null);
  const button = $('btn-backup-go');
  button.disabled = true;
  const label = button.textContent;
  // Deriving the key from the password takes a moment on purpose.
  button.textContent = t('backup_working');
  try {
    await (mode === 'create' ? create(password) : restore(password));
  } catch (err) {
    showError(String(err?.message ?? err));
  } finally {
    button.disabled = false;
    button.textContent = label;
  }
};
