// The history of finished transfers: searchable, with tabs for files and
// texts, and it can be cleared. Texts are listed without their content.
import { $, el, emptyState, fail, fmtSize, icon, open, shortId, toast } from './dom.js';
import { lang, t } from './i18n.js';
import { state } from './state.js';
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
  return el('div', { class: 'row' },
    el('div', { class: 'dir' }, icon(isText ? 'text' : e.direction === 'upload' ? 'up' : 'down', 'sm')),
    el('div', { class: 'body' },
      el('div', { class: 'title', title: e.file_name }, isText ? t(e.direction === 'upload' ? 'hist_text_sent' : 'hist_text_received') : e.file_name),
      el('div', { class: 'sub', title: [device, e.detail].filter(Boolean).join(' · ') }, `${fmtSize(e.size)} · `, status, ` · ${device}`, e.detail ? ` · ${e.detail}` : '')),
    el('span', { class: 'when' }, when));
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
  for (const tab of $('history-tabs').querySelectorAll('[role=tab]')) tab.setAttribute('aria-selected', String(tab.dataset.kind === kind));
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
