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
    settings: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: true },
  }]);
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
