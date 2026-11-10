// Accessibility of the GUI: every control has a name, fields have labels,
// dialogs are labelled, text has enough contrast (WCAG 2.2 AA) in the light
// and the dark theme, and everything works with the keyboard.
//
// Two independent checks run on what is actually rendered, for the main
// screen and every dialog, with the example data of fixtures.js: axe-core
// (the WCAG 2.0–2.2 A and AA rules) and Ferry's own audit (audit.js), which
// also covers what axe leaves to people, like icon-only buttons in lists.
import { audit, axeViolations } from './audit.js';
import { emit, expect, test } from './fixtures.js';

const PEER = 'b'.repeat(64);

/** Puts example content on the screen: a running and a finished transfer. */
async function fillScreen(app) {
  const base = { peer: PEER, peer_name: null, direction: 'download', total: 2_000_000, route: 'direct' };
  await emit(app, 'transfer', { ...base, id: 't1', file_name: 'photo.jpg', done: 500_000, status: { kind: 'running' } });
  await emit(app, 'transfer', { ...base, id: 't2', file_name: 'notes.txt', done: 2_000_000, status: { kind: 'completed', path: '/home/me/notes.txt' } });
  await emit(app, 'transfer', { ...base, id: 't3', file_name: 'big.iso', done: 0, status: { kind: 'failed', error: 'connection lost' } });
}

/** The dialogs to check, with how to open them. */
const DIALOGS = {
  settings: async (app) => app.locator('#btn-settings').click(),
  history: async (app) => app.locator('#btn-history').click(),
  help: async (app) => {
    await app.locator('#btn-help').click();
    await app.locator('#modal-help summary').first().click();
  },
  recipient: async (app) => {
    await app.evaluate(() => window.__respond('take_shared_files', ['/home/me/a.jpg']));
    await emit(app, 'share-files', null);
  },
  offer: async (app) => emit(app, 'incoming-offer', { id: 'o1', peer: PEER, alias: 'Laptop', file_name: 'setup.exe', size: 1000, count: 1, names: ['setup.exe'], risk: 'program' }),
  'LocalSend offer': async (app) => emit(app, 'incoming-offer', { id: 'o2', peer: 'localsend:192.168.1.20', alias: "Kate's iPhone", file_name: 'a.jpg', size: 1000, count: 3, names: ['a.jpg', 'b.jpg'], localsend: '192.168.1.20', risk: null }),
  'localsend PIN': async (app) => emit(app, 'localsend-pin', { fingerprint: 'AB12', alias: "Kate's iPhone", paths: ['/a.jpg'], wrong: true }),
};

for (const scheme of ['light', 'dark']) {
  test.describe(`${scheme} theme`, () => {
    test.use({ colorScheme: scheme });

    test('the main screen has names, labels and enough contrast', async ({ app }) => {
      await fillScreen(app);
      expect(await app.evaluate(audit, null)).toEqual([]);
      expect(await axeViolations(app)).toEqual([]);
    });

    test.describe('on Android without a permission', () => {
      test.use({ responses: { permission_status: { local_network: 'denied', notifications: 'denied', can_ask: true } } });
      test('the permission card has names, labels and enough contrast', async ({ app, isMobile }) => {
        test.skip(!isMobile, 'Android only');
        await expect(app.locator('#permissions')).toBeVisible();
        expect(await app.evaluate(audit, '#permissions')).toEqual([]);
        expect(await axeViolations(app, '#permissions')).toEqual([]);
      });
    });

    test.describe('for a new user', () => {
      test.use({ responses: { list_peers: [], get_welcome: true } });
      test('the welcome card has names, labels and enough contrast', async ({ app }) => {
        await expect(app.locator('#welcome')).toBeVisible();
        expect(await app.evaluate(audit, '#welcome')).toEqual([]);
        expect(await axeViolations(app, '#welcome')).toEqual([]);
      });
    });

    for (const [name, openDialog] of Object.entries(DIALOGS)) {
      test(`the ${name} dialog has names, labels and enough contrast`, async ({ app }) => {
        await openDialog(app);
        const dialog = app.locator('.modal-overlay.active').last();
        await expect(dialog).toBeVisible();
        const id = await dialog.getAttribute('id');
        expect(await app.evaluate(audit, `#${id} .modal`)).toEqual([]);
        expect(await axeViolations(app, `#${id}`)).toEqual([]);
      });
    }
  });
}

test('the page language follows the interface language', async ({ app }) => {
  await expect(app.locator('html')).toHaveAttribute('lang', 'en');
});

test('a dialog keeps the keyboard inside, closes with Escape and gives the focus back', async ({ app, isMobile }) => {
  test.skip(isMobile, 'keyboard navigation is a desktop matter');
  const opener = app.locator('#btn-history');
  await opener.focus();
  await app.keyboard.press('Enter');
  const dialog = app.locator('#modal-history');
  await expect(dialog).toHaveClass(/active/);
  for (let i = 0; i < 15; i++) {
    await app.keyboard.press('Tab');
    expect(await app.evaluate(() => !!document.activeElement.closest('#modal-history')), `Tab ${i + 1} stays in the dialog`).toBe(true);
  }
  await app.keyboard.press('Shift+Tab');
  expect(await app.evaluate(() => !!document.activeElement.closest('#modal-history'))).toBe(true);
  await app.keyboard.press('Escape');
  await expect(dialog).not.toHaveClass(/active/);
  await expect(opener).toBeFocused();
});

test('the main actions are reachable with Tab and show where the focus is', async ({ app, isMobile }) => {
  test.skip(isMobile, 'keyboard navigation is a desktop matter');
  const reached = new Set();
  for (let i = 0; i < 60; i++) {
    await app.keyboard.press('Tab');
    const info = await app.evaluate(() => {
      const e = document.activeElement;
      const s = getComputedStyle(e);
      return { id: e.id || `${e.tagName}.${e.className} "${(e.textContent || '').trim().slice(0, 30)}"`, outline: s.outlineStyle !== 'none' && parseFloat(s.outlineWidth) > 0, shadow: s.boxShadow !== 'none' };
    });
    if (info.id.startsWith('BODY')) continue; // past the last control, before the first
    reached.add(info.id);
    expect(info.outline || info.shadow, `focus is visible on #${info.id}`).toBe(true);
  }
  for (const id of ['btn-select-file', 'btn-send-text', 'btn-settings', 'btn-history', 'btn-nearby-refresh']) {
    expect(reached, `#${id} is reachable with Tab`).toContain(id);
  }
});

test('toasts are announced to screen readers', async ({ app }) => {
  const toast = app.locator('#toast');
  await expect(toast).toHaveAttribute('role', 'status');
  await expect(toast).toHaveAttribute('aria-live', 'polite');
});

test('the history tabs follow the tab pattern: one Tab stop, arrow keys between tabs', async ({ app, isMobile }) => {
  test.skip(isMobile, 'keyboard navigation is a desktop matter');
  await app.locator('#btn-history').click();
  const tab = (name) => app.locator('#history-tabs').getByRole('tab', { name });
  await tab('All').focus();
  await app.keyboard.press('ArrowRight');
  await expect(tab('Files')).toBeFocused();
  await expect(tab('Files')).toHaveAttribute('aria-selected', 'true');
  await expect(tab('All')).toHaveAttribute('tabindex', '-1');
  await app.keyboard.press('ArrowLeft');
  await app.keyboard.press('ArrowLeft');
  await expect(tab('Texts')).toBeFocused();
  await app.keyboard.press('Home');
  await expect(tab('All')).toBeFocused();
});

test('animations stop when the system asks for less motion', async ({ app }) => {
  await app.emulateMedia({ reducedMotion: 'reduce' });
  const duration = await app.evaluate(() => getComputedStyle(document.querySelector('.modal')).transitionDuration);
  expect(duration.split(',').every(d => parseFloat(d) === 0)).toBe(true);
});
