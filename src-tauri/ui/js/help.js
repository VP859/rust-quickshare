// Help that does not get in the way: a help window with short answers (the
// ? button in the top bar), and a small welcome card for new users that goes
// away for good once closed, or once the first device is paired or the first
// file has gone through.
import { $, el, fail, open } from './dom.js';
import { docUrl, TOPICS } from './help-topics.js';
import { t } from './i18n.js';
import { state } from './state.js';
import { invoke, isMobile, listen } from './tauri.js';

// ---------- Help window ----------
function renderHelp() {
  $('help-topics').replaceChildren(...TOPICS.map((topic) => {
    // The website opens in the system browser (desktop); on a phone the
    // answers stand on their own.
    const more = isMobile ? '' : el('button', { class: 'link', onclick: () => invoke('open_link', { url: docUrl(topic.doc) }).catch(fail) },
      t('help_more'));
    return el('details', { class: 'help-topic', 'data-topic': topic.id },
      el('summary', {}, t('help_' + topic.id + '_q')),
      el('p', {}, t('help_' + topic.id + '_a')),
      more);
  }));
}

/** Opens the help window, with `topic` unfolded when given. */
function openHelp(topic = null) {
  renderHelp();
  if (topic) {
    const details = $('help-topics').querySelector(`[data-topic="${topic}"]`);
    if (details) details.open = true;
  }
  open('modal-help');
}
$('btn-help').onclick = () => openHelp();

/** A "Why?" link (or `label`) that opens `topic` of the help window. */
export function helpLink(topic, label = 'help_why') {
  return el('button', { class: 'link', onclick: () => openHelp(topic) }, t(label));
}
// The same for the links written in index.html (`data-help="topic"`).
for (const link of document.querySelectorAll('[data-help]')) link.onclick = () => openHelp(link.dataset.help);

// ---------- Welcome card ----------
/** The card may be shown: it was never closed and nothing has happened yet. */
let welcome = false;

/** Shows the card to a new user (no paired devices yet). */
export function renderWelcome() {
  if (welcome && state.peers.length) {
    welcomeDone(); // the first device is paired: it did its job
    return;
  }
  $('welcome').hidden = !welcome;
}

/** Hides the card for good. */
function welcomeDone() {
  welcome = false;
  $('welcome').hidden = true;
  invoke('dismiss_welcome').catch((e) => console.warn('Welcome:', e));
}

/** Asks the backend whether the card was closed before, and listens for the first transfer. */
export async function loadWelcome() {
  try { welcome = await invoke('get_welcome'); } catch { welcome = false; }
  renderWelcome();
  await listen('transfer', ({ payload }) => {
    if (welcome && payload?.status?.kind === 'completed') welcomeDone();
  });
}
$('btn-welcome-close').onclick = welcomeDone;
$('btn-welcome-help').onclick = () => openHelp('send');
