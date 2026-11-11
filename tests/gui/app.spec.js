// Behaviour of the GUI frontend against a mocked Tauri API (see fixtures.js).
import { axeViolations } from './audit.js';
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

test('a running transfer shows its speed and the time left once they are known', async ({ app }) => {
  // A stopped clock: only runFor moves it, so the speed is exact.
  await app.clock.install({ time: new Date('2026-11-10T12:00:00Z') });
  await app.clock.pauseAt(new Date('2026-11-10T12:00:01Z'));
  const at = (done) => emit(app, 'transfer', transfer({ kind: 'running' }, { done, total: 100_000_000, route: 'direct' }));
  const row = app.locator('#transfers .transfer');
  await at(0);
  await app.clock.runFor(500);
  await at(5_000_000);
  // Not after half a second: the first values jump.
  await expect(row).toContainText('5% · 4.8 MB / 95.4 MB · direct');
  for (let i = 2; i <= 10; i++) {
    await app.clock.runFor(500);
    await at(i * 5_000_000);
  }
  // 10 MB/s steadily; 50 MB of 100 MB left.
  await expect(row).toContainText('50% · 47.7 MB / 95.4 MB · 9.5 MB/s · 0:05 left · direct');

  // No new data: the last speed gives way to how long nothing arrived …
  await app.clock.runFor(2500);
  await expect(row).toContainText('· 9.5 MB/s · 0:05 left');
  await app.clock.runFor(1000);
  await expect(row).toContainText('50% · 47.7 MB / 95.4 MB · no data for 0:03 · direct');
  await app.clock.runFor(2000);
  await expect(row).toContainText('no data for 0:05');
  // … and data after it is measured anew, without the gap.
  await at(55_000_000);
  await expect(row).toContainText('55% · 52.5 MB / 95.4 MB · direct');
  await app.clock.runFor(1000);
  await at(65_000_000);
  await expect(row).toContainText('· 9.5 MB/s · 0:04 left');

  // A pause starts a new measurement after resuming.
  await emit(app, 'transfer', transfer({ kind: 'paused' }, { done: 50_000_000, total: 100_000_000 }));
  await at(50_000_000);
  await expect(row).toContainText('50% · 47.7 MB / 95.4 MB · direct');

  // The speed stays when only the language changes.
  await app.clock.runFor(1000);
  await at(60_000_000);
  await expect(row).toContainText('· 9.5 MB/s · 0:04 left');
  await app.locator('#btn-settings').click();
  await app.locator('#language').selectOption('pl');
  await expect(row).toContainText('· 9.5 MB/s · zostało 0:04 · bezpośrednio');
});

test('a received file opens from its row, on Android in the app for its type', async ({ app, isMobile }) => {
  const path = isMobile ? '/storage/emulated/0/Pictures/Ferry/photo.jpg' : '/home/me/Downloads/photo.jpg';
  await emit(app, 'transfer', transfer({ kind: 'completed', path }, { direction: 'download', done: 2_000_000 }));
  const row = app.locator('#transfers .transfer');
  await row.getByRole('button', { name: 'Open', exact: true }).click();
  expect(await calls(app)).toContainEqual(['open_received', { path, reveal: false }]);
  // Android has no folder view for a single file.
  await expect(row.getByRole('button', { name: 'Show in folder' })).toHaveCount(isMobile ? 0 : 1);
  // Sent files and failed downloads have nothing to open.
  await emit(app, 'transfer', transfer({ kind: 'completed', path: null }, { id: 't2' }));
  await emit(app, 'transfer', transfer({ kind: 'failed', error: 'lost' }, { id: 't3', direction: 'download' }));
  await expect(app.locator('#transfers').getByRole('button', { name: 'Open', exact: true })).toHaveCount(1);
});

test('a download from a Ferry device can be cancelled and its partial file deleted', async ({ app }) => {
  await emit(app, 'transfer', transfer({ kind: 'running' }, { direction: 'download' }));
  const row = app.locator('#transfers .transfer');
  await row.getByRole('button', { name: 'Cancel and delete what was received' }).click();
  expect(await calls(app)).toContainEqual(['cancel_transfer', { id: 't1', discard: true }]);
  await expect(app.locator('#toast')).toContainText('partly received file is deleted');

  // Uploads and LocalSend downloads have nothing to delete here.
  await emit(app, 'transfer', transfer({ kind: 'running' }, { id: 't2' }));
  await emit(app, 'transfer', transfer({ kind: 'running' }, { id: 't3', direction: 'download', peer: 'localsend:192.168.1.20', peer_name: 'Phone' }));
  await expect(app.locator('#transfers').getByRole('button', { name: 'Cancel and delete what was received' })).toHaveCount(1);
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
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: true, keep_history: true, paired_only: false, pairing_open: false, announce_platform: false, random_port: false, strip_metadata: false },
  }]);
});

test('"only paired devices" shows the pairing window, which opens pairing for a while', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await expect(app.locator('#pairing-window')).toBeHidden();
  await app.locator('label.switch:has(#privacy-paired)').click();
  await expect(app.locator('#privacy-paired')).toBeChecked();
  expect(await calls(app)).toContainEqual(['set_privacy', {
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: true, paired_only: true, pairing_open: false, announce_platform: false, random_port: false, strip_metadata: false },
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
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: true, paired_only: false, pairing_open: false, announce_platform: true, random_port: false, strip_metadata: false },
  }]);
  await app.locator('label.switch:has(#privacy-port)').click();
  await expect(app.locator('#modal-restart')).toHaveClass(/active/);
});

test('removing the location from photos is a privacy setting', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await expect(app.locator('#privacy-strip')).not.toBeChecked();
  await app.locator('label.switch:has(#privacy-strip)').click();
  expect(await calls(app)).toContainEqual(['set_privacy', {
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: true, paired_only: false, pairing_open: false, announce_platform: false, random_port: false, strip_metadata: true },
  }]);
});

test('a text goes through the recipient list and appears among the transfers', async ({ app }) => {
  await app.locator('#btn-send-text').click();
  await app.locator('#text-input').fill('https://example.org/x');
  await app.locator('#btn-text-next').click();
  const recipients = app.locator('#modal-recipient');
  await expect(recipients).toHaveClass(/active/);
  await expect(app.locator('#btn-share-ticket')).toBeHidden();
  await expect(app.locator('#localsend-section')).toBeHidden();
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
  // Other addresses wait behind "Does not work?".
  await expect(app.locator('#web-other-list')).toBeHidden();
  await app.locator('#web-other summary').click();
  await expect(app.locator('#web-other-list')).toContainText('10.5.0.2');
  await expect(app.locator('#web-expires')).toContainText('30 more minute');
  await expect(app.locator('#web-access')).toContainText('Not opened yet');
  // Help for a phone that cannot open the link, folded away.
  await app.locator('#web-phone-help summary').click();
  await expect(app.locator('#web-phone-help')).toContainText('Local network');
  // The phone opened it; then another device was turned away.
  await app.evaluate(() => window.__respond('web_share_status', { running: true, owner: '192.168.1.20', refused: [] }));
  await expect(app.locator('#web-access')).toContainText('Opened by 192.168.1.20', { timeout: 5000 });
  await app.evaluate(() => window.__respond('web_share_status', { running: true, owner: '192.168.1.20', refused: ['192.168.1.30'] }));
  await expect(app.locator('#web-access')).toContainText('Turned away: 192.168.1.30', { timeout: 5000 });
  await expect(dialog).toContainText('encrypted with the key in the link');
  await expect(dialog).toContainText('network you trust');

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

test('the relay-only mode asks for a relay first and keeps the saved one', async ({ app }) => {
  await app.locator('#btn-settings').click();
  await app.locator('#network-mode').selectOption('relay-only');
  await expect(app.locator('#relay-field')).toBeVisible();
  await expect(app.locator('#network-desc')).toContainText('never directly');
  expect((await calls(app)).some(([cmd]) => cmd === 'set_network_mode')).toBe(false);
  await app.locator('#relay-urls').fill('https://relay.example.org');
  await app.locator('#btn-relay-save').click();
  expect(await calls(app)).toContainEqual(['set_network_mode', { mode: 'relay-only', relays: ['https://relay.example.org'] }]);
  await app.locator('#btn-restart-later').click();
  await expect(app.locator('#mode-pill')).toContainText('Own relay only');

  // With a relay saved, switching between the relay modes saves at once.
  await app.locator('#network-mode').selectOption('relay');
  expect(await calls(app)).toContainEqual(['set_network_mode', { mode: 'relay', relays: null }]);
  await app.locator('#btn-restart-later').click();
  await app.locator('#network-mode').selectOption('relay-only');
  expect((await calls(app)).filter(([cmd, args]) => cmd === 'set_network_mode' && args.mode === 'relay-only')).toHaveLength(2);
});

test('the entered relays can be checked before they are saved', async ({ app }) => {
  await app.evaluate(() => window.__respond('check_relays', [
    { url: 'https://relay.example.org', ms: 23, error: null },
    { url: 'https://relay.exmaple.org', ms: null, error: 'Cannot connect to relay.exmaple.org on port 443 (unknown name, server off or port blocked)' },
  ]));
  await app.locator('#btn-settings').click();
  await app.locator('#network-mode').selectOption('relay-only');
  await app.locator('#btn-relay-check').click();
  await expect(app.locator('#toast')).toContainText('Enter the address of a relay');
  expect((await calls(app)).some(([cmd]) => cmd === 'check_relays')).toBe(false);

  await app.locator('#relay-urls').fill('https://relay.example.org\nhttps://relay.exmaple.org');
  await app.locator('#btn-relay-check').click();
  expect(await calls(app)).toContainEqual(['check_relays', { relays: ['https://relay.example.org', 'https://relay.exmaple.org'] }]);
  const results = app.locator('#relay-results > div');
  await expect(results).toHaveCount(2);
  await expect(results.nth(0)).toContainText('https://relay.example.org – works (23 ms)');
  await expect(results.nth(0).locator('.dot.ok')).toHaveCount(1);
  await expect(results.nth(1)).toContainText('does not work: Cannot connect to relay.exmaple.org');
  await expect(results.nth(1).locator('.dot.bad')).toHaveCount(1);
  await expect(app.locator('#btn-relay-check')).toBeEnabled();
  await expect(app.locator('#btn-relay-check')).toHaveText('Check');
  // Checking saves nothing.
  expect((await calls(app)).some(([cmd]) => cmd === 'set_network_mode')).toBe(false);
  expect(await axeViolations(app, '#relay-field')).toEqual([]);
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
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: false, paired_only: false, pairing_open: false, announce_platform: false, random_port: false, strip_metadata: false },
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

test('a file from the history goes out again: to the same Ferry device or to someone chosen now', async ({ app }) => {
  await app.evaluate((peer) => window.__respond('get_history', [
    { time: '2026-11-04T11:00:00Z', direction: 'upload', peer, file_name: 'slides.pdf', size: 10, status: 'failed', detail: 'connection lost', source: '/home/me/slides.pdf', local_file: '/home/me/slides.pdf', ferry_peer: true },
    { time: '2026-11-03T09:00:00Z', direction: 'upload', peer: 'browser:192.168.1.20', peer_name: 'Safari', file_name: 'report.pdf', size: 100, status: 'completed', local_file: '/home/me/report.pdf', ferry_peer: false },
    { time: '2026-11-02T09:00:00Z', direction: 'download', peer, file_name: 'gone.jpg', size: 1, status: 'completed', detail: '/x/gone.jpg', local_file: null, ferry_peer: true },
  ]), PEER);
  await app.locator('#btn-history').click();
  const rows = app.locator('#history-list .row');
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(1).getByRole('button', { name: /Send again/ })).toHaveCount(0);
  await expect(rows.nth(2).getByRole('button')).toHaveCount(0);
  await rows.nth(0).getByRole('button', { name: 'Send again to Laptop' }).click();
  expect(await calls(app)).toContainEqual(['send_files', { paths: ['/home/me/slides.pdf'], target: PEER }]);
  await expect(app.locator('#modal-history')).not.toHaveClass(/active/);

  await app.locator('#btn-history').click();
  await rows.nth(1).getByRole('button', { name: 'Send…' }).click();
  await expect(app.locator('#modal-recipient')).toHaveClass(/active/);
  await expect(app.locator('#recipient-files')).toHaveText('report.pdf');
});

test('the clipboard from the tray menu is ready to be sent, or the user hears that it is empty', async ({ app }) => {
  await emit(app, 'send-clipboard', 'https://example.org/from-the-clipboard');
  await expect(app.locator('#text-input')).toHaveValue('https://example.org/from-the-clipboard');
  await app.locator('#modal-text-send [data-close]').first().click();
  await emit(app, 'send-clipboard', null);
  await expect(app.locator('#toast')).toContainText('clipboard is empty');
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

test('files can go to a LocalSend app, with a warning that its name is not verified', async ({ app }) => {
  await app.evaluate(() => window.__respond('take_shared_files', ['/home/me/a.jpg']));
  await emit(app, 'share-files', null);
  const dialog = app.locator('#modal-recipient');
  await expect(dialog).toHaveClass(/active/);
  await expect(app.locator('#localsend-section')).toBeVisible();
  await expect(app.locator('#localsend-section .compat-warning')).toContainText('not verified');
  const row = app.locator('#localsend-recipients').getByRole('button', { name: /Kate's iPhone/ });
  await expect(row).toContainText('192.168.1.20');
  await app.locator('#btn-localsend-scan').click();
  expect(await calls(app)).toContainEqual(['scan_localsend', {}]);
  await expect(app.locator('#localsend-recipients')).toContainText('None found yet');
});

test('a LocalSend app that was heard is a recipient', async ({ app }) => {
  await app.evaluate(() => window.__respond('take_shared_files', ['/home/me/a.jpg']));
  await emit(app, 'share-files', null);
  await app.locator('#localsend-recipients').getByRole('button', { name: /Kate's iPhone/ }).click();
  expect(await calls(app)).toContainEqual(['send_to_localsend', { paths: ['/home/me/a.jpg'], fingerprint: 'AB12', pin: null }]);
  await expect(app.locator('#modal-recipient')).not.toHaveClass(/active/);
});

test('LocalSend apps are listed among nearby devices as not verified, and the search finds them too', async ({ app }) => {
  const block = app.locator('#nearby-localsend-block');
  const list = app.locator('#nearby-localsend');
  await expect(block).toBeVisible();
  await expect(list).toContainText("Kate's iPhone");
  await expect(list).toContainText('LocalSend · not verified');
  await expect(block.locator('.hint')).toContainText('names are not verified');
  await expect(app.locator('#localsend-count')).toHaveText('1');
  await app.locator('#btn-nearby-refresh').click();
  expect(await calls(app)).toContainEqual(['refresh_nearby', {}]);
  expect(await calls(app)).toContainEqual(['scan_localsend', {}]);
  // A device answering later appears without another search.
  await emit(app, 'localsend-nearby', [{ alias: 'Mac mini', model: 'Mac', device_type: 'desktop', fingerprint: 'CD34', address: '192.168.1.30:53317', https: true }]);
  await expect(list).toContainText('Mac mini');
});

test('a LocalSend app that asks for a PIN gets it, and a wrong one is said so', async ({ app }) => {
  await emit(app, 'localsend-pin', { fingerprint: 'AB12', alias: "Kate's iPhone (iPhone)", paths: ['/home/me/a.jpg'], wrong: true });
  const dialog = app.locator('#modal-localsend-pin');
  await expect(dialog).toHaveClass(/active/);
  await expect(app.locator('#localsend-pin-title')).toContainText("Kate's iPhone");
  await expect(app.locator('#localsend-pin-wrong')).toBeVisible();
  await app.locator('#localsend-pin').fill('4711');
  await app.locator('#btn-pin-send').click();
  await expect(dialog).not.toHaveClass(/active/);
  expect(await calls(app)).toContainEqual(['send_to_localsend', { paths: ['/home/me/a.jpg'], fingerprint: 'AB12', pin: '4711' }]);
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

test('the help window answers briefly and links to the documentation', async ({ app, isMobile }) => {
  await app.locator('#btn-help').click();
  const dialog = app.locator('#modal-help');
  await expect(dialog).toHaveClass(/active/);
  const topics = dialog.locator('.help-topic');
  await expect(topics).toHaveCount(7);
  await expect(dialog.locator('.help-topic p').first()).toBeHidden();
  await dialog.getByText('How do I pair a device, and why compare the code?').click();
  await expect(dialog.locator('[data-topic="pair"] p')).toContainText('same six-digit code');
  const more = dialog.locator('[data-topic="pair"]').getByRole('button', { name: 'More in the documentation' });
  if (isMobile) {
    await expect(more).toHaveCount(0);
  } else {
    await more.click();
    expect(await calls(app)).toContainEqual(['open_link', { url: 'https://vp859.github.io/rust-quickshare/index.html#local-network-and-pairing' }]);
  }
});

test.describe('a new user', () => {
  test.use({ responses: { list_peers: [], get_welcome: true } });

  test('sees a short welcome that leads to the help and goes away for good when closed', async ({ app }) => {
    const card = app.locator('#welcome');
    await expect(card).toBeVisible();
    await expect(card.locator('li')).toHaveCount(3);
    await card.getByRole('button', { name: 'How does it work?' }).click();
    await expect(app.locator('#modal-help')).toHaveClass(/active/);
    await expect(app.locator('[data-topic="send"]')).toHaveAttribute('open', '');
    await app.locator('#modal-help [data-close]').first().click();
    await card.getByRole('button', { name: 'Close the welcome' }).click();
    await expect(card).toBeHidden();
    expect(await calls(app)).toContainEqual(['dismiss_welcome', {}]);
  });

  test('no longer needs it after the first file went through', async ({ app }) => {
    await expect(app.locator('#welcome')).toBeVisible();
    await emit(app, 'transfer', { id: 'w1', peer: PEER, peer_name: null, file_name: 'a.jpg', direction: 'upload', total: 1, done: 1, status: { kind: 'completed', path: null }, route: null });
    await expect(app.locator('#welcome')).toBeHidden();
    expect(await calls(app)).toContainEqual(['dismiss_welcome', {}]);
  });

  test('no longer needs it after pairing the first device', async ({ app }) => {
    await expect(app.locator('#welcome')).toBeVisible();
    await app.evaluate((peer) => window.__respond('list_peers', [{ node_id: peer, alias: 'Laptop', auto_accept: false, verified: false, added_at: '2026-11-01T00:00:00Z', has_address: true }]), PEER);
    await emit(app, 'peers-changed', null);
    await expect(app.locator('#welcome')).toBeHidden();
    expect(await calls(app)).toContainEqual(['dismiss_welcome', {}]);
  });
});

test.describe('someone who already uses Ferry', () => {
  test.use({ responses: { get_welcome: true } });

  test('does not see the welcome (a device is paired)', async ({ app }) => {
    await expect(app.locator('#peers')).toContainText('Laptop');
    await expect(app.locator('#welcome')).toBeHidden();
    expect(await calls(app)).toContainEqual(['dismiss_welcome', {}]);
  });
});

test('"Why?" links lead to the matching help topic', async ({ app }) => {
  // Nearby is empty in the example data: the empty list asks.
  await emit(app, 'nearby', []);
  await app.locator('#nearby').getByRole('button', { name: 'A device does not show up?' }).click();
  await expect(app.locator('#modal-help')).toHaveClass(/active/);
  await expect(app.locator('[data-topic="trouble"]')).toHaveAttribute('open', '');
  await app.locator('#modal-help [data-close]').first().click();
  // While pairing, the reason to compare the code.
  await emit(app, 'pairing-code', { node_id: 'c'.repeat(64), name: 'Kitchen PC', code: '123 456' });
  await app.locator('#modal-pairing').getByRole('button', { name: 'Why compare the code?' }).click();
  await expect(app.locator('[data-topic="pair"]')).toHaveAttribute('open', '');
});

test.describe('on Android 17 without the local network permission', () => {
  test.use({ responses: { permission_status: { local_network: 'denied', notifications: 'granted', storage: 'not_needed', can_ask: true } } });

  test('Ferry says why before Android asks, and the card goes when it is allowed', async ({ app, isMobile }) => {
    test.skip(!isMobile, 'Android only');
    const card = app.locator('#permissions');
    await expect(card).toBeVisible();
    await expect(card.locator('li')).toHaveCount(1);
    await expect(card).toContainText('Local network');
    await card.getByRole('button', { name: 'Allow' }).click();
    expect(await calls(app)).toContainEqual(['request_permissions', {}]);
    await app.evaluate(() => {
      window.__respond('permission_status', { local_network: 'granted', notifications: 'granted', can_ask: true });
      window.dispatchEvent(new Event('ferry-permissions'));
    });
    await expect(card).toBeHidden();
  });

  test('"Not now" leaves a hint under Nearby, and a refused dialog leads to the settings', async ({ app, isMobile }) => {
    test.skip(!isMobile, 'Android only');
    await emit(app, 'nearby', []);
    await app.locator('#permissions').getByRole('button', { name: 'Not now' }).click();
    await expect(app.locator('#permissions')).toBeHidden();
    await expect(app.locator('#nearby')).toContainText('may not use the local network');
    await app.evaluate(() => {
      window.__respond('permission_status', { local_network: 'denied', notifications: 'granted', can_ask: false });
      window.dispatchEvent(new Event('ferry-permissions'));
    });
    await app.locator('#nearby').getByRole('button', { name: 'Allow' }).click();
    await expect(app.locator('#permissions')).toBeVisible();
    await expect(app.locator('#permissions')).toContainText('Android no longer asks');
    expect(await calls(app)).toContainEqual(['open_app_settings', {}]);
  });
});

test.describe('on Android 17 without the local network permission, searching', () => {
  test.use({ responses: { permission_status: { local_network: 'denied', notifications: 'granted', storage: 'not_needed', can_ask: true } } });

  test('a search asks Android for the local network only', async ({ app, isMobile }) => {
    test.skip(!isMobile, 'Android only');
    await app.locator('#permissions').getByRole('button', { name: 'Not now' }).click();
    await app.locator('#btn-nearby-refresh').click();
    await expect.poll(() => calls(app)).toContainEqual(['request_permissions', { only: 'local_network' }]);
    expect(await calls(app)).toContainEqual(['refresh_nearby', {}]);
  });

  test('once Android no longer asks, a search brings the card back', async ({ app, isMobile }) => {
    test.skip(!isMobile, 'Android only');
    await app.locator('#permissions').getByRole('button', { name: 'Not now' }).click();
    await app.evaluate(() => {
      window.__respond('permission_status', { local_network: 'denied', notifications: 'granted', can_ask: false });
      window.dispatchEvent(new Event('ferry-permissions'));
    });
    await app.locator('#btn-nearby-refresh').click();
    await expect(app.locator('#permissions')).toBeVisible();
    await expect(app.locator('#permissions')).toContainText('Android no longer asks');
    expect((await calls(app)).some(([cmd, args]) => cmd === 'request_permissions' && args.only)).toBe(false);
  });
});

test.describe('on Android while its permission dialog is open', () => {
  test.use({ responses: { permission_status: { local_network: 'denied', notifications: 'denied', storage: 'not_needed', can_ask: true, asking: true } } });

  test('the card waits for the answer', async ({ app, isMobile }) => {
    test.skip(!isMobile, 'Android only');
    await expect(app.locator('#permissions')).toBeHidden();
    await app.evaluate(() => {
      window.__respond('permission_status', { local_network: 'granted', notifications: 'denied', can_ask: true, asking: false });
      window.dispatchEvent(new Event('ferry-permissions'));
    });
    await expect(app.locator('#permissions')).toBeVisible();
    await expect(app.locator('#permissions li')).toHaveCount(1);
    await expect(app.locator('#permissions')).toContainText('Notifications');
  });
});

test.describe('scanning a QR code on Android', () => {
  const SCAN = { 'plugin:barcode-scanner|scan': { content: 'ferry:abc' }, inspect_ticket: { has_share: true }, receive_from_ticket: null };
  const cmds = async (app) => (await calls(app)).map(([cmd]) => cmd).filter((cmd) => cmd.startsWith('plugin:barcode-scanner'));

  test.describe('the first time', () => {
    test.use({ responses: { ...SCAN, 'plugin:barcode-scanner|check_permissions': { camera: 'prompt' }, 'plugin:barcode-scanner|request_permissions': { camera: 'granted' } } });
    test('asks for the camera only then, and scans once it is allowed', async ({ app, isMobile }) => {
      test.skip(!isMobile, 'Android only');
      expect(await cmds(app)).toEqual([]);
      await app.locator('#btn-scan').click();
      await expect.poll(() => cmds(app)).toEqual(['plugin:barcode-scanner|check_permissions', 'plugin:barcode-scanner|request_permissions', 'plugin:barcode-scanner|scan']);
      await expect.poll(() => calls(app)).toContainEqual(['receive_from_ticket', { ticket: 'ferry:abc' }]);
    });
  });

  test.describe('when the camera is refused', () => {
    test.use({ responses: { ...SCAN, 'plugin:barcode-scanner|check_permissions': { camera: 'prompt' }, 'plugin:barcode-scanner|request_permissions': { camera: 'denied' } } });
    test('says how to go on without it and does not scan', async ({ app, isMobile }) => {
      test.skip(!isMobile, 'Android only');
      await app.locator('#btn-scan').click();
      await expect(app.locator('#toast')).toContainText('Without the camera');
      expect(await cmds(app)).not.toContain('plugin:barcode-scanner|scan');
      await expect(app.locator('#permissions')).toBeHidden();
    });
  });

  test.describe('when Android no longer asks for the camera', () => {
    test.use({ responses: { ...SCAN, 'plugin:barcode-scanner|check_permissions': { camera: 'denied' } } });
    test('the card leads to the app settings and goes once it is allowed', async ({ app, isMobile }) => {
      test.skip(!isMobile, 'Android only');
      await app.locator('#btn-scan').click();
      const card = app.locator('#permissions');
      await expect(card).toBeVisible();
      await expect(card).toContainText('Camera');
      expect(await cmds(app)).toEqual(['plugin:barcode-scanner|check_permissions']);
      await card.getByRole('button', { name: 'Open the app settings' }).click();
      expect(await calls(app)).toContainEqual(['open_app_settings', {}]);
      await app.evaluate(() => {
        window.__respond('plugin:barcode-scanner|check_permissions', { camera: 'granted' });
        window.dispatchEvent(new Event('ferry-permissions'));
      });
      await expect(card).toBeHidden();
    });
  });
});

test('the desktop never shows the permission card', async ({ app, isMobile }) => {
  test.skip(isMobile, 'desktop only');
  await expect(app.locator('#permissions')).toBeHidden();
  expect((await calls(app)).some(([cmd]) => cmd === 'permission_status')).toBe(false);
});

test('the top bar says only whether the node is online; the Node ID is in the tooltip', async ({ app, isMobile }) => {
  const pill = app.locator('#node-pill');
  await expect(app.locator('#node-status')).toHaveText('Online');
  await expect(pill).toHaveAttribute('title', /Node ID a{64}/);
  const width = (await pill.boundingBox()).width;
  // On a phone only the dot is visible; the word stays for screen readers.
  if (isMobile) expect(width).toBeLessThan(20);
  else expect(width).toBeGreaterThan(40);
});
