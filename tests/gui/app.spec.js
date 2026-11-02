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
