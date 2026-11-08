// Behaviour of the GUI frontend against a mocked Tauri API (see fixtures.js).
import { calls, emit, expect, respond, test } from './fixtures.js';

const PEER = 'b'.repeat(64);

/** A transfer event as sent by the Rust side. */
const transfer = (status, extra = {}) => ({
  id: 't1',
  peer: PEER,
  file_name: 'photo.jpg',
  direction: 'upload',
  total: 2_000_000,
  done: 500_000,
  status,
  route: 'relay',
  ...extra,
});

test('starts, lists trusted devices and loads under the strict CSP', async ({ app }) => {
  await expect(app.locator('#peers')).toContainText('Laptop');
  const names = (await calls(app)).map(([cmd]) => cmd);
  expect(names).toEqual(expect.arrayContaining(['get_language', 'list_peers', 'get_node_info']));
});

test('shows progress, route and a cancel button for a running transfer', async ({ app }) => {
  await emit(app, 'transfer', transfer({ kind: 'running' }));
  const row = app.locator('#transfers .transfer');
  await expect(row).toContainText('25%');
  await expect(row).toContainText('via relay');

  await row.getByRole('button', { name: 'Cancel transfer' }).click();
  expect(await calls(app)).toContainEqual(['cancel_transfer', { id: 't1' }]);

  await emit(app, 'transfer', transfer({ kind: 'cancelled' }, { done: 0 }));
  await expect(row).toContainText('Cancelled');
  await expect(row.getByRole('button', { name: 'Cancel transfer' })).toHaveCount(0);
});

test('a completed download names the route it took', async ({ app }) => {
  await emit(app, 'transfer', transfer({ kind: 'completed', path: '/home/me/Downloads/photo.jpg' }, {
    direction: 'download',
    done: 2_000_000,
    route: 'direct',
  }));
  await expect(app.locator('#transfers .transfer')).toContainText('direct');
});

test('the pairing code arrives with an event and opens the dialog', async ({ app }) => {
  await emit(app, 'pairing-code', { node_id: PEER, name: 'Laptop', code: '123 456' });
  await expect(app.locator('#modal-pairing')).toHaveClass(/active/);
  await expect(app.locator('#pairing-code')).toHaveText('123 456');

  await emit(app, 'pair-result', { node_id: PEER, alias: 'Laptop', ok: false, message: 'Declined' });
  await expect(app.locator('#pairing-status')).toHaveText('Declined');
});

test('switching the language translates the page', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await app.locator('#language').selectOption('pl');
  expect(await calls(app)).toContainEqual(['set_language', { code: 'pl' }]);
  await expect(app.locator('#settings-title')).toHaveText('Ustawienia');
});

test('changing the network mode asks for a restart and shows the pending mode', async ({ app }, info) => {
  await app.locator('#btn-settings').click();
  await app.locator('#network-mode').selectOption('internet');
  await expect(app.locator('#modal-restart')).toHaveClass(/active/);
  await expect(app.locator('#mode-pill')).toContainText('after restart');

  // Only the desktop can restart itself; Android explains how to do it.
  const ok = app.locator('#btn-restart-ok');
  if (info.project.name === 'desktop') {
    await expect(ok).toBeVisible();
  } else {
    await expect(ok).toBeHidden();
  }

  await app.locator('#btn-restart-later').click();
  await expect(app.locator('#modal-restart')).toBeHidden();
});

test('the close action is saved on the desktop', async ({ app }, info) => {
  test.skip(info.project.name !== 'desktop', 'the window close setting exists only on the desktop');
  await app.locator('#btn-settings').click();
  await app.locator('#close-action').selectOption('quit');
  expect(await calls(app)).toContainEqual(['set_quit_on_close', { quit: true }]);
});

test('dialogs keep the keyboard focus and give it back when closed', async ({ app }, info) => {
  test.skip(info.project.name !== 'desktop', 'keyboard navigation is tested on the desktop');
  const settings = app.locator('#btn-settings');
  await settings.click();
  const modal = app.locator('#modal-settings');
  await expect(modal).toHaveClass(/active/);
  for (let i = 0; i < 40; i++) {
    await app.keyboard.press('Tab');
    const inside = await modal.evaluate((m) => m.contains(document.activeElement));
    const focused = await app.evaluate(() => document.activeElement?.outerHTML.slice(0, 120));
    expect(inside, `Tab ${i + 1} moved the focus to ${focused}`).toBe(true);
  }
  await app.keyboard.press('Escape');
  await expect(modal).not.toHaveClass(/active/);
  await expect(settings).toBeFocused();
});

test('Android shows what limits it in the background', async ({ app }, info) => {
  const android = info.project.name === 'android';
  if (android) {
    await respond(app, 'background_status', {
      battery_optimized: true,
      power_save: false,
      background_restricted: false,
      data_saver: true,
    });
  }
  await app.locator('#btn-settings').click();
  const section = app.locator('#background-section');
  if (!android) {
    await expect(section).toBeHidden();
    return;
  }
  await expect(section).toBeVisible();
  await expect(section.locator('li.warn')).toHaveCount(2);
  await expect(section).toContainText('Battery optimization is on');
  await section.getByRole('button', { name: 'Open app settings' }).click();
  expect(await calls(app)).toContainEqual(['open_app_settings', {}]);
});

test('an offer for a program carries a warning, a photo does not', async ({ app }) => {
  const offer = (file_name, risk) => ({
    id: `o-${file_name}`, peer: PEER, alias: 'Laptop', file_name, size: 1000, count: 1, names: [], risk,
  });
  await emit(app, 'incoming-offer', offer('setup.exe', 'program'));
  await expect(app.locator('#modal-offer')).toHaveClass(/active/);
  await expect(app.locator('#offer-risk')).toBeVisible();
  await expect(app.locator('#offer-risk')).toContainText('program or installer');

  await app.locator('#btn-offer-reject').click();
  await emit(app, 'incoming-offer', offer('holiday.jpg', null));
  await expect(app.locator('#offer-file')).toHaveText('holiday.jpg');
  await expect(app.locator('#offer-risk')).toBeHidden();
});

test('the safety number of a trusted device can be compared and confirmed', async ({ app }) => {
  await app.locator('#peers .device').first().getByRole('button', { name: 'Device options' }).click();
  await app.getByRole('menuitem', { name: /Safety number/ }).click();
  const dialog = app.locator('#modal-safety');
  await expect(dialog).toHaveClass(/active/);
  await expect(app.locator('#safety-title')).toContainText('Laptop');
  await expect(app.locator('#safety-number span')).toHaveCount(12);
  await expect(app.locator('#safety-number')).toContainText('54414');

  await dialog.locator('label.switch').click();
  await expect(app.locator('#safety-verified')).toBeChecked();
  expect(await calls(app)).toContainEqual(['set_verified', { nodeId: PEER, verified: true }]);
});

test('an offer from a LocalSend app says the sender is not verified', async ({ app }) => {
  await emit(app, 'incoming-offer', {
    id: 'localsend-1', peer: 'localsend:192.168.1.20', alias: "Kate's iPhone (iPhone)", file_name: 'IMG_0001.HEIC',
    size: 2_000_000, count: 1, names: [], risk: null, localsend: '192.168.1.20',
  });
  await expect(app.locator('#modal-offer')).toHaveClass(/active/);
  await expect(app.locator('#offer-localsend')).toBeVisible();
  await expect(app.locator('#offer-localsend')).toContainText('192.168.1.20');
  await expect(app.locator('#offer-localsend')).toContainText('not verified');
  await expect(app.locator('#offer-meta')).toContainText("Kate's iPhone");

  await app.locator('#btn-offer-reject').click();
  await emit(app, 'incoming-offer', {
    id: 'o-2', peer: PEER, alias: 'Laptop', file_name: 'a.jpg', size: 1, count: 1, names: [], risk: null, localsend: null,
  });
  await expect(app.locator('#offer-localsend')).toBeHidden();
});

test('the LocalSend compatibility mode is switched on in the privacy settings', async ({ app }) => {
  await app.locator('#btn-settings').click();
  const toggle = app.locator('#privacy-localsend');
  await expect(toggle).not.toBeChecked();
  await app.locator('label.switch:has(#privacy-localsend)').click();
  await expect(toggle).toBeChecked();
  expect(await calls(app)).toContainEqual(['set_privacy', {
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: true, keep_history: true, paired_only: false, pairing_open: false, announce_platform: false, random_port: false },
  }]);
});

test('"only paired devices" shows the pairing window, which opens pairing for a while', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await expect(app.locator('#pairing-window')).toBeHidden();
  await app.locator('label.switch:has(#privacy-paired)').click();
  await expect(app.locator('#privacy-paired')).toBeChecked();
  expect(await calls(app)).toContainEqual(['set_privacy', {
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: true, paired_only: true, pairing_open: false, announce_platform: false, random_port: false },
  }]);
  await expect(app.locator('#pairing-window')).toBeVisible();
  await expect(app.locator('#pairing-window-label')).toHaveText('Pairing is closed');
  await app.locator('#btn-pairing-window').click();
  expect(await calls(app)).toContainEqual(['set_pairing_open', { open: true }]);
});

test('showing the operating system and a random port are privacy settings; the port asks for a restart', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await app.locator('label.switch:has(#privacy-platform)').click();
  expect(await calls(app)).toContainEqual(['set_privacy', {
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: true, paired_only: false, pairing_open: false, announce_platform: true, random_port: false },
  }]);
  await app.locator('label.switch:has(#privacy-port)').click();
  await expect(app.locator('#modal-restart')).toHaveClass(/active/);
});

test('a text goes through the recipient list and appears among the transfers', async ({ app }) => {
  await app.locator('#btn-send-text').click();
  await app.locator('#text-input').fill('https://example.org/x');
  await app.locator('#btn-text-next').click();
  const recipients = app.locator('#modal-recipient');
  await expect(recipients).toHaveClass(/active/);
  await expect(app.locator('#btn-share-ticket')).toBeHidden();
  await recipients.getByRole('button', { name: /Laptop/ }).click();
  await expect(recipients).not.toHaveClass(/active/);
  expect(await calls(app)).toContainEqual(['send_text', { text: 'https://example.org/x', target: PEER }]);
  const row = app.locator('#transfers .row.transfer').first();
  await expect(row).toContainText('https://example.org/x');
  await expect(row).toContainText('Laptop');
  await expect(app.locator('#btn-clear')).toBeVisible();
});

test('receiving from a browser shows the link, its lifetime and a stop button', async ({ app }) => {
  await app.locator('#btn-web-receive').click();
  const dialog = app.locator('#modal-web');
  await expect(dialog).toHaveClass(/active/);
  expect(await calls(app)).toContainEqual(['start_web_share', { paths: [] }]);
  await expect(app.locator('#web-title')).toHaveText('Receive from a browser');
  await expect(app.locator('#web-url')).toHaveText('http://192.168.1.10:47835/0123456789abcdef0123456789abcdef/');
  await expect(app.locator('#web-other')).toContainText('10.5.0.2');
  await expect(app.locator('#web-expires')).toContainText('30 more minute');
  await expect(dialog).toContainText('not encrypted');

  await app.locator('#btn-web-stop').click();
  await expect(dialog).not.toHaveClass(/active/);
  expect(await calls(app)).toContainEqual(['stop_web_share', {}]);
});

test('chosen files can be shared with a browser instead of a device', async ({ app }) => {
  await app.evaluate(() => window.__respond('plugin:dialog|open', ['/home/me/a.jpg', '/home/me/b.pdf']));
  await app.evaluate(() => window.__respond('start_web_share', { url: 'http://192.168.1.10:47835/ab/', urls: ['http://192.168.1.10:47835/ab/'], qr_svg: '<svg xmlns="http://www.w3.org/2000/svg"/>', expires_in: 1800, files: 2 }));
  await app.locator('#btn-select-file').click();
  await expect(app.locator('#modal-recipient')).toHaveClass(/active/);
  await app.locator('#btn-share-web').click();
  expect(await calls(app)).toContainEqual(['start_web_share', { paths: ['/home/me/a.jpg', '/home/me/b.pdf'] }]);
  await expect(app.locator('#web-title')).toHaveText('Share 2 file(s) with a browser');
  await expect(app.locator('#web-other')).toBeHidden();
});

test('files from a browser are marked in the offer', async ({ app }) => {
  await emit(app, 'incoming-offer', {
    id: 'browser-1', peer: 'browser:192.168.1.20', alias: 'Web browser (Safari, iPhone) at 192.168.1.20',
    file_name: 'IMG_1.HEIC', size: 1000, count: 1, names: [], risk: null, localsend: null, browser: 'Web browser (Safari, iPhone) at 192.168.1.20',
  });
  await expect(app.locator('#offer-browser')).toBeVisible();
  await expect(app.locator('#offer-browser')).toContainText('web browser');
  await expect(app.locator('#offer-localsend')).toBeHidden();
  await expect(app.locator('#offer-meta')).toContainText('Safari, iPhone');
});

test('the own-relay mode is saved together with the relay address', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await app.locator('#network-mode').selectOption('relay');
  // Nothing is saved before there is a relay.
  await expect(app.locator('#relay-field')).toBeVisible();
  expect((await calls(app)).some(([cmd]) => cmd === 'set_network_mode')).toBe(false);
  await expect(app.locator('#network-desc')).toContainText('no n0 server');

  await app.locator('#btn-relay-save').click();
  await expect(app.locator('#toast')).toContainText('Enter the address of a relay');

  await app.locator('#relay-urls').fill('https://relay.example.org\nhttp://192.168.1.5:3340');
  await app.locator('#btn-relay-save').click();
  expect(await calls(app)).toContainEqual(['set_network_mode', { mode: 'relay', relays: ['https://relay.example.org', 'http://192.168.1.5:3340'] }]);
  await expect(app.locator('#modal-restart')).toHaveClass(/active/);
  await app.locator('#btn-restart-later').click();
  await expect(app.locator('#mode-pill')).toContainText('Local + own relay');

  // Back to local: the relay field hides and the relays stay saved.
  await app.locator('#network-mode').selectOption('local');
  await expect(app.locator('#relay-field')).toBeHidden();
  expect(await calls(app)).toContainEqual(['set_network_mode', { mode: 'local', relays: null }]);
});

test('a running transfer can be paused and resumed', async ({ app }) => {
  const ev = (status) => ({ id: 't1', peer: PEER, peer_name: null, file_name: 'big.iso', direction: 'download', total: 1000, done: 400, status, route: 'direct' });
  await emit(app, 'transfer', ev({ kind: 'running' }));
  const row = app.locator('#transfers .row.transfer').first();
  await row.getByRole('button', { name: 'Pause' }).click();
  expect(await calls(app)).toContainEqual(['pause_transfer', { id: 't1', paused: true }]);

  await emit(app, 'transfer', ev({ kind: 'paused' }));
  await expect(row).toContainText('Paused at 400 B of 1000 B');
  await row.getByRole('button', { name: 'Resume' }).click();
  expect(await calls(app)).toContainEqual(['pause_transfer', { id: 't1', paused: false }]);
  await expect(row.getByRole('button', { name: 'Cancel transfer' })).toBeVisible();
});

test('files can be sent to several devices at once', async ({ app }) => {
  await emit(app, 'nearby', [{ node_id: 'c'.repeat(64), name: 'Kitchen PC', platform: 'linux', addr: null }]);
  await app.evaluate(() => window.__respond('plugin:dialog|open', ['/home/me/a.jpg']));
  await app.locator('#btn-select-file').click();
  const dialog = app.locator('#modal-recipient');
  await expect(app.locator('#btn-send-selected')).toBeHidden();
  await dialog.getByRole('checkbox', { name: 'Select Laptop' }).check();
  await dialog.getByRole('checkbox', { name: 'Select Kitchen PC' }).check();
  await expect(app.locator('#btn-send-selected')).toHaveText('Send to the selected devices (2)');
  await app.locator('#btn-send-selected').click();
  await expect(dialog).not.toHaveClass(/active/);
  const sends = (await calls(app)).filter(([cmd]) => cmd === 'send_files').map(([, a]) => a.target);
  expect(sends.sort()).toEqual([PEER, 'c'.repeat(64)].sort());
});

test('the history can be searched and cleared', async ({ app }) => {
  await app.locator('#btn-history').click();
  const dialog = app.locator('#modal-history');
  await expect(dialog).toHaveClass(/active/);
  await expect(app.locator('#history-list .row')).toHaveCount(3);
  await expect(app.locator('#history-list')).toContainText('holiday.jpg');
  await expect(app.locator('#history-list')).toContainText('Laptop', { message: 'the alias of a trusted device' });
  await expect(app.locator('#history-list')).toContainText('declined');

  await app.locator('#history-search').fill('report');
  await expect.poll(async () => (await calls(app)).some(([cmd, a]) => cmd === 'get_history' && a.query === 'report' && a.kind === 'all')).toBe(true);

  app.once('dialog', (d) => d.accept());
  await app.locator('#btn-history-clear').click();
  expect(await calls(app)).toContainEqual(['clear_history', {}]);
});

test('the speed limit and the history setting are saved', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await expect(app.locator('#speed-limit')).toHaveValue('');
  await app.locator('#speed-limit').fill('2M');
  await app.locator('#btn-speed-save').click();
  expect(await calls(app)).toContainEqual(['set_speed_limit', { value: '2M' }]);
  await expect(app.locator('#toast')).toContainText('Speed limit: 2M/s');

  await expect(app.locator('#privacy-history')).toBeChecked();
  await app.locator('label.switch:has(#privacy-history)').click();
  expect(await calls(app)).toContainEqual(['set_privacy', {
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: false, paired_only: false, pairing_open: false, announce_platform: false, random_port: false },
  }]);
});

test('the empty transfers list leads quietly to the history', async ({ app }) => {
  const link = app.locator('#transfers .empty .link');
  await expect(link).toHaveText('See earlier transfers in the history');
  await link.click();
  await expect(app.locator('#modal-history')).toHaveClass(/active/);
  await expect(app.locator('#history-list .row')).toHaveCount(3);
  await app.locator('[data-close="modal-history"]').first().click();

  // A transfer replaces the empty message (and with it the link).
  await emit(app, 'transfer', { id: 't9', peer: PEER, peer_name: null, file_name: 'x.bin', direction: 'download', total: 10, done: 10, status: { kind: 'completed', path: null }, route: null });
  await expect(link).toHaveCount(0);
});

test('the history has tabs for files and texts, and texts show no content', async ({ app }) => {
  await app.locator('#btn-history').click();
  const tab = (name) => app.locator('#history-tabs').getByRole('tab', { name });
  await expect(tab('All')).toHaveAttribute('aria-selected', 'true');

  await tab('Texts').click();
  expect(await calls(app)).toContainEqual(['get_history', { query: '', kind: 'text' }]);
  await expect(tab('Texts')).toHaveAttribute('aria-selected', 'true');
  await expect(tab('All')).toHaveAttribute('aria-selected', 'false');

  await tab('Files').click();
  expect(await calls(app)).toContainEqual(['get_history', { query: '', kind: 'file' }]);

  // The mock answers with everything: check how a text row looks.
  await tab('All').click();
  const text = app.locator('#history-list .row', { hasText: 'Text sent' });
  await expect(text).toHaveCount(1);
  await expect(text).toContainText('27 B');
  await expect(text).toContainText('Laptop');
});

test('files from "Send with Ferry" in a file manager open the recipient list', async ({ app }) => {
  // Ferry is already running; the file manager starts it again with the files.
  await app.evaluate(() => window.__respond('take_shared_files', ['/home/me/Photos/a.jpg', '/home/me/Photos/b.jpg']));
  await emit(app, 'share-files', null);
  const dialog = app.locator('#modal-recipient');
  await expect(dialog).toHaveClass(/active/);
  await expect(app.locator('#recipient-files')).toContainText('a.jpg, b.jpg');
  await dialog.getByRole('button', { name: /Laptop/ }).click();
  expect(await calls(app)).toContainEqual(['send_files', { paths: ['/home/me/Photos/a.jpg', '/home/me/Photos/b.jpg'], target: PEER }]);
});

test('the device key can be moved into the system keyring', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await expect(app.locator('#keyring-toggle')).toBeVisible();
  await expect(app.locator('#privacy-keyring')).not.toBeChecked();
  await app.locator('label.switch:has(#privacy-keyring)').click();
  expect(await calls(app)).toContainEqual(['set_key_storage', { system: true }]);
  await expect(app.locator('#toast')).toContainText('now in the system keyring');
  await expect(app.locator('#privacy-keyring')).toBeChecked();
});

test('the keyring option is hidden where there is no keyring', async ({ app }) => {
  await app.evaluate(() => window.__respond('get_key_storage', { system: false, supported: false }));
  await app.locator('#btn-settings').click();
  await expect(app.locator('#modal-settings')).toHaveClass(/active/);
  await expect(app.locator('#keyring-toggle')).toBeHidden();
});
