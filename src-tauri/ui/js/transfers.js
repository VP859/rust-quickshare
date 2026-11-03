// The transfers list (files from `transfer` events, texts from texts.js) and
// the dialog for incoming offers.
import { $, close, el, emptyState, fail, fmtSize, icon, open, shortId } from './dom.js';
import { t } from './i18n.js';
import { state } from './state.js';
import { invoke, isMobile, listen } from './tauri.js';

/** Row ID → `{ el, ev }` for files, or `{ el, text, render }` for texts. */
const transfers = new Map();
const FINAL = ['completed', 'failed', 'rejected', 'cancelled'];

/** Puts a row on top of the list; `row.render()` draws it (also after a language change). */
let textSeq = 0;
export function addRow(row) {
  transfers.set('text-' + (++textSeq), row);
  const list = $('transfers');
  list.querySelector('.empty')?.remove();
  list.prepend(row.el);
  row.render();
  $('btn-clear').style.visibility = '';
  return row;
}

function renderTransfer(ev) {
  let row = transfers.get(ev.id);
  if (!row) {
    row = { el: el('div', { class: 'row transfer' }) };
    transfers.set(ev.id, row);
    const list = $('transfers');
    list.querySelector('.empty')?.remove();
    list.prepend(row.el);
  }
  const previous = row.ev?.status.kind;
  row.ev = ev;
  const kind = ev.status.kind;
  if (kind !== previous && FINAL.includes(kind)) {
    $('sr-status').textContent = `${ev.file_name}: ${t('st_' + kind)}`;
  }
  const pct = ev.total ? Math.min(100, Math.round(ev.done / ev.total * 100)) : (kind === 'completed' ? 100 : 0);
  const peer = state.peers.find(p => p.node_id === ev.peer)?.alias ?? ev.peer_name ?? shortId(ev.peer);
  // How the data travels; a relay explains a slow transfer in internet mode.
  const route = ev.route ? ' · ' + t('route_' + ev.route) : '';
  const detail = kind === 'running' ? (ev.total ? `${pct}% · ${fmtSize(ev.done)} / ${fmtSize(ev.total)}${route}` : fmtSize(ev.done) + route)
    : kind === 'retrying' ? t('st_retrying', { attempt: ev.status.attempt, of: ev.status.of })
    : kind === 'failed' ? t('tr_failed', { error: ev.status.error }) : kind === 'rejected' ? t('tr_rejected', { reason: ev.status.reason })
    : kind === 'completed' ? (ev.status.path ? t('tr_saved', { path: ev.status.path }) : t('st_completed')) + route
    : t('st_' + kind);
  row.el.className = 'row transfer' + (kind === 'completed' ? ' done' : kind === 'failed' || kind === 'rejected' ? ' failed' : kind === 'cancelled' ? ' cancelled' : '');
  const bar = el('div', { class: 'progress' }, el('div'));
  bar.firstChild.style.width = pct + '%';
  const parts = [
    el('div', { class: 'dir' }, icon(ev.direction === 'upload' ? 'up' : 'down', 'sm')),
    el('div', { class: 'body' },
      el('div', { class: 'meta' }, el('span', { class: 'title' }, ev.file_name), el('span', { class: 'muted small' }, peer)),
      bar,
      el('div', { class: 'detail' }, detail))];
  if (kind === 'preparing' || kind === 'running' || kind === 'retrying') {
    parts.push(el('div', { class: 'trailing' },
      el('button', { class: 'ghost icon', title: t('cancel_transfer'), 'aria-label': t('cancel_transfer'),
        onclick: () => invoke('cancel_transfer', { id: ev.id }).catch(fail) }, icon('x', 'sm'))));
  }
  if (kind === 'completed' && ev.direction === 'download' && ev.status.path && !isMobile) {
    const openFile = (reveal) => invoke('open_received', { path: ev.status.path, reveal }).catch(fail);
    parts.push(el('div', { class: 'trailing' },
      el('button', { onclick: () => openFile(false) }, t('open')),
      el('button', { class: 'ghost icon', title: t('show_in_folder'), 'aria-label': t('show_in_folder'), onclick: () => openFile(true) }, icon('folder', 'sm'))));
  }
  row.el.replaceChildren(...parts);
  $('btn-clear').style.visibility = '';
}

export function renderNoTransfers() {
  if (!transfers.size) $('transfers').replaceChildren(emptyState('inbox', t('no_transfers')));
  $('btn-clear').style.visibility = transfers.size ? '' : 'hidden';
}
$('btn-clear').onclick = () => {
  for (const [id, row] of transfers) {
    const finished = row.text ? row.text.state !== 'sending' : FINAL.includes(row.ev.status.kind);
    if (finished) { row.el.remove(); transfers.delete(id); }
  }
  renderNoTransfers();
};

// ---------- Incoming offers ----------
export const offers = [];
export function showNextOffer() {
  const o = offers[0];
  if (!o) return close('modal-offer');
  const batch = (o.count ?? 1) > 1;
  $('offer-title').textContent = t(batch ? 'incoming_files' : 'incoming_file');
  $('offer-file').textContent = batch ? t('n_files', { n: o.count }) : o.file_name;
  const more = o.count - o.names.length;
  $('offer-names').textContent = batch ? o.names.join('\n') + (more > 0 ? '\n' + t('and_more', { n: more }) : '') : '';
  $('offer-names').hidden = !batch;
  $('offer-meta').textContent = t('from', { size: fmtSize(o.size), name: o.alias ?? t('unknown_device', { id: shortId(o.peer) }) });
  // LocalSend compatibility mode: the sender is not verified.
  $('offer-localsend').hidden = !o.localsend;
  $('offer-localsend').textContent = o.localsend ? t('localsend_offer', { address: o.localsend }) : '';
  // Programs, scripts and documents with macros: say so before the user decides.
  $('offer-risk').hidden = !o.risk;
  $('offer-risk').textContent = o.risk ? t(`risk_${o.risk}`) : '';
  $('offer-queue').textContent = offers.length > 1 ? t('queued', { n: offers.length - 1 }) : '';
  open('modal-offer');
}
async function answerOffer(accept) {
  const o = offers.shift();
  if (o) { try { await invoke('respond_to_offer', { transferId: o.id, accept }); } catch (e) { fail(e); } }
  showNextOffer();
}
$('btn-offer-accept').onclick = () => answerOffer(true);
$('btn-offer-reject').onclick = () => answerOffer(false);

/** Draws every row and the open offer again (after a language change). */
export function rerenderTransfers() {
  for (const row of transfers.values()) row.render ? row.render() : renderTransfer(row.ev);
  renderNoTransfers();
  if (offers.length) showNextOffer();
}

/** Subscribes to transfer events and incoming offers. */
export async function listenTransfers() {
  await listen('transfer', ({ payload }) => renderTransfer(payload));
  await listen('transfer-error', ({ payload }) => fail(payload.message));
  await listen('incoming-offer', ({ payload }) => {
    offers.push(payload);
    if (offers.length === 1) showNextOffer();
    else $('offer-queue').textContent = t('queued', { n: offers.length - 1 });
  });
}
