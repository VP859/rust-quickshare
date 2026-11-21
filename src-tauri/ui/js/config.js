// The settings file (config.toml): the warning when it could not be read at
// start, and its check and repair (a dialog opened then, or from the settings).
import { $, close, el, fail, open, toast } from './dom.js';
import { t } from './i18n.js';
import { askRestart } from './settings.js';
import { invoke } from './tauri.js';

/** Hides the warning: the settings in use are complete again. */
export function hideConfigProblem() {
  $('config-problem').hidden = true;
}

/** Opens the dialog: `problem` is why the file could not be read at start, or null for a check from the settings. */
function openDialog(problem) {
  $('config-title').textContent = t(problem ? 'config_title' : 'config_check_title');
  $('config-modal-text').textContent = problem ?? t('config_check_modal');
  $('config-report').hidden = true;
  $('config-repair-ask').hidden = true;
  $('btn-config-repair').hidden = true;
  $('btn-config-verify').hidden = false;
  $('btn-config-verify').disabled = false;
  $('btn-config-close').textContent = t(problem ? 'config_go_app' : 'close');
  open('modal-config');
}

/** Shows why the settings could not be read: a dialog at start, and the warning above the app until they are repaired. */
export async function showConfigProblem() {
  let problem = null;
  try { problem = await invoke('config_problem'); } catch {}
  $('config-problem-text').textContent = problem ?? '';
  $('config-problem').hidden = !problem;
  if (problem) openDialog(problem);
}

/** Checks the file and lists what was found; offers a repair when it would change something. */
async function verify() {
  const button = $('btn-config-verify');
  button.disabled = true;
  button.textContent = t('config_checking');
  try {
    const { problems, repairable } = await invoke('check_config');
    const list = $('config-report');
    list.replaceChildren(...(problems.length
      ? problems.map((p) => el('li', { class: p.severity }, p.message))
      : [el('li', {}, t('config_no_problems'))]));
    list.hidden = false;
    const damaged = problems.some((p) => p.severity === 'error');
    const ask = $('config-repair-ask');
    ask.textContent = t(repairable ? 'config_repair_ask' : damaged ? 'config_cannot_repair' : 'config_nothing_to_repair');
    ask.hidden = !problems.length;
    button.hidden = true;
    $('btn-config-repair').hidden = !repairable;
    if (repairable) $('btn-config-close').textContent = t('config_not_now');
    ($('btn-config-repair').hidden ? $('btn-config-close') : $('btn-config-repair')).focus();
  } catch (e) {
    fail(e);
  } finally {
    button.disabled = false;
    button.textContent = t('config_verify');
  }
}

/** Repairs the file; the repaired settings are fully used after a restart. */
async function repair() {
  const button = $('btn-config-repair');
  button.disabled = true;
  try {
    await invoke('repair_config');
    close('modal-config');
    hideConfigProblem();
    toast(t('config_repaired'));
    askRestart('repair');
  } catch (e) {
    fail(e);
  } finally {
    button.disabled = false;
  }
}

$('btn-config-close').onclick = () => close('modal-config');
$('btn-config-verify').onclick = verify;
$('btn-config-repair').onclick = repair;
$('btn-config-problem-check').onclick = () => { openDialog($('config-problem-text').textContent); verify(); };
$('btn-config-check').onclick = () => { openDialog(null); verify(); };
