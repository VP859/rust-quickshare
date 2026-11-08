// The history of finished transfers: searchable, with tabs for files and
// texts, and it can be cleared. Texts are listed without their content.
import { $, close, el, emptyState, fail, fmtSize, icon, open, shortId, toast } from './dom.js';
import { lang, t } from './i18n.js';
import { state } from './state.js';
import { chooseRecipient, sendFiles } from './send.js';
import { invoke } from './tauri.js';

let timer = null;
/** The tab shown: `all`, `file` or `text`. */
let kind = 'all';

/** One finished transfer. */
function entryRow(e) {
  const device = e.peer_name ?? state.peers.find(p => p.node_id === e.peer)?.alias ?? shortId(e.peer);
  const when = new Date(e.time).toLocaleString(lang, { dateStyle: 'short', timeStyle: 'short' });
  const status = el('span', { class: e.status === 'completed' ? '' : 'failed' }, t(`hist_${e.status}`));
  const isText = e.kind === 'text';
  // A file still on this device can go out again: to the same Ferry device
  // (what was sent to it), or to a recipient chosen now.
  const actions = [];
  if (e.local_file && e.direction === 'upload' && e.ferry_peer) {
    actions.push(el('button', { class: 'ghost icon', title: t('send_again_to', { name: device }), 'aria-label': t('send_again_to', { name: device }),
      onclick: () => { close('modal-history'); sendFiles([e.local_file], e.peer, device); } }, icon('refresh', 'sm')));
  }
  if (e.local_file) {
    actions.push(el('button', { class: 'ghost icon', title: t('send_to_other'), 'aria-label': t('send_to_other'),
      onclick: () => { close('modal-history'); chooseRecipient([e.local_file]); } }, icon('send', 'sm')));
  }
  return el('div', { class: 'row' },
    el('div', { class: 'dir' }, icon(isText ? 'text' : e.direction === 'upload' ? 'up' : 'down', 'sm')),
    el('div', { class: 'body' },
      el('div', { class: 'title', title: e.file_name }, isText ? t(e.direction === 'upload' ? 'hist_text_sent' : 'hist_text_received') : e.file_name),
      el('div', { class: 'sub', title: [device, e.detail].filter(Boolean).join(' · ') }, `${fmtSize(e.size)} · `, status, ` · ${device}`, e.detail ? ` · ${e.detail}` : '')),
    el('span', { class: 'when' }, when),
    actions.length ? el('div', { class: 'trailing' }, ...actions) : '');
}

async function render() {
  try {
    const entries = await invoke('get_history', { query: $('history-search').value, kind });
    $('history-list').replaceChildren(...(entries.length ? entries.map(entryRow) : [emptyState('inbox', t('history_empty'))]));
  } catch (e) { fail(e); }
}

/** Switches to the tab `next` (`all`, `file`, `text`). */
function selectTab(next) {
  kind = next;
  for (const tab of $('history-tabs').querySelectorAll('[role=tab]')) {
    const selected = tab.dataset.kind === kind;
    tab.setAttribute('aria-selected', String(selected));
    // Tab moves past the tab list; the arrow keys move between the tabs.
    tab.tabIndex = selected ? 0 : -1;
  }
  return render();
}

/** Opens the history window (also from the empty transfers list). */
export async function openHistory() {
  $('history-search').value = '';
  $('history-off').hidden = state.privacy.keep_history !== false;
  await selectTab('all');
  open('modal-history');
  $('history-search').focus();
}
$('btn-history').onclick = openHistory;
for (const tab of $('history-tabs').querySelectorAll('[role=tab]')) tab.onclick = () => selectTab(tab.dataset.kind);
$('history-tabs').onkeydown = (e) => {
  const tabs = [...$('history-tabs').querySelectorAll('[role=tab]')];
  const i = tabs.findIndex(tab => tab.dataset.kind === kind);
  const next = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: tabs.length - 1 }[e.key];
  if (next === undefined) return;
  e.preventDefault();
  const tab = tabs[(next + tabs.length) % tabs.length];
  selectTab(tab.dataset.kind);
  tab.focus();
};
$('history-search').oninput = () => {
  clearTimeout(timer);
  timer = setTimeout(render, 200);
};
$('btn-history-clear').onclick = async () => {
  if (!confirm(t('history_confirm_clear'))) return;
  try {
    await invoke('clear_history');
    toast(t('history_cleared'));
    await render();
  } catch (e) { fail(e); }
};
