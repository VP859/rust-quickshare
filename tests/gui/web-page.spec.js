// The page a browser sees in a Ferry browser session (src/web/page), served
// with the server's Content-Security-Policy and a mocked `api/` of the
// session that encrypts and decrypts like the server (with the page's own
// seal.js, whose bytes tests in src/web/seal.rs pin to the Rust side). The
// real server is tested in tests/web.rs.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test as base, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { Opener, openAll, purpose, sealAll } from '../../src/web/page/seal.js';
import { audit } from './audit.js';

const PAGE = join(import.meta.dirname, '..', '..', 'src', 'web', 'page');
const SERVER = readFileSync(join(import.meta.dirname, '..', '..', 'src', 'web', 'server.rs'), 'utf8');
// The policy exactly as the server sends it.
const CSP = SERVER.match(/const CSP: &str = "([^"]+)"/s)[1].replace(/\\\s+/g, '');
const BASE = 'http://192.168.1.10:47835/0123456789abcdef0123456789abcdef/';

/** The session key and the link with it after `#`. */
const KEY = Uint8Array.from({ length: 32 }, (_, i) => (i * 7 + 3) % 256);
const fragment = (key) => '#k=' + Buffer.from(key).toString('base64url');
const LINK = BASE + fragment(KEY);

const REPORT = Buffer.from(Uint8Array.from({ length: 2_500_000 }, (_, i) => i % 251));
const INFO = { device: "Kate's laptop", files: [{ id: 0, name: 'report.pdf', size: REPORT.length }], receive: true, expires_in: 1500, max_files: 3 };

const STATIC = new Set(['page.js', 'page.css', 'seal.js', 'vendor/chacha.js', 'vendor/_arx.js', 'vendor/_poly1305.js', 'vendor/utils.js']);

/**
 * Serves the page and a session `api` on `page`: answers in `api.info` and
 * `api.offer`, and everything the page sent, decrypted, in `api.offers` and
 * `api.uploads` (raw requests in `api.requests`).
 */
async function serve(page, api) {
  await page.route(`${BASE}**`, async (route) => {
    const request = route.request();
    const path = request.url().slice(BASE.length).replace(/#.*$/, '');
    const body = request.postDataBuffer();
    api.requests.push([request.method(), path, body]);
    if (path === '') return route.fulfill({ path: join(PAGE, 'index.html'), headers: { 'Content-Security-Policy': CSP } });
    if (STATIC.has(path)) return route.fulfill({ path: join(PAGE, path), contentType: path.endsWith('.css') ? 'text/css' : 'text/javascript' });
    const sealed = (aad, bytes) => route.fulfill({ contentType: 'application/octet-stream', body: Buffer.from(sealAll(KEY, aad, bytes)) });
    const reply = (status, json) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(json) });
    if (path === 'api/info') {
      if (api.info.status !== 200) return reply(api.info.status, api.info.body);
      return sealed(purpose.info, new TextEncoder().encode(JSON.stringify(api.info.body)));
    }
    if (path === 'api/files/0') return sealed(purpose.download(0), REPORT);
    if (path === 'api/offer') {
      api.offers.push(JSON.parse(new TextDecoder().decode(openAll(KEY, purpose.offer, new Uint8Array(body)))));
      return reply(api.offer.status, api.offer.body);
    }
    const upload = /^api\/upload\/(\w+)\/(\d+)$/.exec(path);
    if (upload) {
      const [, id, index] = upload;
      const size = api.offers.at(-1).files[Number(index)].size;
      const opener = new Opener(KEY, purpose.upload(id, Number(index)), size);
      const parts = opener.push(new Uint8Array(body));
      opener.finish();
      api.uploads.push(Buffer.concat(parts.map((p) => Buffer.from(p))).toString());
      return reply(200, { ok: true });
    }
    return reply(404, { message: 'Not found' });
  });
}

const newApi = () => ({ info: { status: 200, body: INFO }, offer: { status: 200, body: { id: 'off1' } }, requests: [], offers: [], uploads: [] });

const test = base.extend({
  // biome-ignore lint/correctness/noEmptyPattern: Playwright fixtures take their dependencies as an object pattern.
  api: async ({}, use) => use(newApi()),
  session: async ({ page, api }, use) => {
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('console', (m) => { if (m.type() === 'error' && !/status of (403|404|410|429)/.test(m.text())) errors.push(m.text()); });
    await serve(page, api);
    await use(page);
    expect(errors, 'no errors in the console').toEqual([]);
  },
});

test.use({ locale: 'en-US' });

test('shows the device, the shared files and how long the link works', async ({ session }) => {
  await session.goto(LINK);
  await expect(session.locator('#title')).toHaveText("Files from Kate's laptop");
  await expect(session.locator('#files li')).toHaveCount(1);
  await expect(session.locator('#files')).toContainText('report.pdf');
  await expect(session.locator('#files')).toContainText('2.4 MB');
  await expect(session.locator('#upload-title')).toHaveText("Send to Kate's laptop");
  await expect(session.locator('#expires')).toHaveText('This link works for 25 more minute(s).');
  await expect(session.locator('footer')).toContainText('encrypted with a key that is only in this link');
  // The key leaves the address bar (and so the browser's history) …
  expect(session.url()).toBe(BASE);
  // … but a reload of the tab still works.
  await session.reload();
  await expect(session.locator('#title')).toHaveText("Files from Kate's laptop");
});

test('a shared file is downloaded encrypted and saved decrypted under its name', async ({ session }) => {
  await session.goto(LINK);
  const saving = session.waitForEvent('download');
  await session.locator('#files').getByRole('button', { name: 'Download: report.pdf' }).click();
  const download = await saving;
  expect(download.suggestedFilename()).toBe('report.pdf');
  expect(readFileSync(await download.path()).equals(REPORT)).toBe(true);
});

test('files are offered first and uploaded one by one after acceptance, all encrypted', async ({ session, api }) => {
  await session.goto(LINK);
  await session.locator('#picker').setInputFiles([
    { name: 'a.jpg', mimeType: 'image/jpeg', buffer: Buffer.from('aaaa') },
    { name: 'b.txt', mimeType: 'text/plain', buffer: Buffer.from('bb') },
  ]);
  await expect(session.locator('#status')).toHaveText('All files were sent.');
  await expect(session.locator('#uploads li.done')).toHaveCount(2);
  expect(api.offers).toEqual([{ files: [{ name: 'a.jpg', size: 4 }, { name: 'b.txt', size: 2 }] }]);
  expect(api.uploads).toEqual(['aaaa', 'bb']);
  const sent = api.requests.filter(([m]) => m === 'POST' || m === 'PUT');
  expect(sent.map(([, path]) => path)).toEqual(['api/offer', 'api/upload/off1/0', 'api/upload/off1/1']);
  // Nothing readable on the wire: neither the names nor the content.
  for (const [, , body] of sent) {
    for (const secret of ['a.jpg', 'b.txt', 'aaaa']) expect(body.includes(secret)).toBe(false);
  }
});

test('a declined offer uploads nothing', async ({ session, api }) => {
  api.offer = { status: 403, body: { message: 'The files were declined.' } };
  await session.goto(LINK);
  await session.locator('#picker').setInputFiles([{ name: 'a.jpg', mimeType: 'image/jpeg', buffer: Buffer.from('a') }]);
  await expect(session.locator('#status')).toHaveText("Kate's laptop declined the files.");
  await expect(session.locator('#status')).toHaveClass(/error/);
  await expect(session.locator('#uploads li.failed')).toHaveCount(1);
  expect(api.requests.some(([m]) => m === 'PUT')).toBe(false);
});

test('too many files are refused before asking', async ({ session, api }) => {
  await session.goto(LINK);
  const files = [1, 2, 3, 4].map((i) => ({ name: `${i}.txt`, mimeType: 'text/plain', buffer: Buffer.from('x') }));
  await session.locator('#picker').setInputFiles(files);
  await expect(session.locator('#status')).toHaveText('At most 3 files at once.');
  expect(api.requests.some(([, path]) => path === 'api/offer')).toBe(false);
});

test('a link used by another device says so', async ({ session, api }) => {
  api.info = { status: 403, body: { message: 'This link is already used by another device. Start a new session in Ferry.' } };
  await session.goto(LINK);
  await expect(session.locator('#status')).toContainText('already used by another device');
  await expect(session.locator('#upload')).toBeHidden();
  await expect(session.locator('#download')).toBeHidden();
});

test('a link without its key, or with another key, says so and shows nothing', async ({ session, api }) => {
  await session.goto(BASE);
  await expect(session.locator('#status')).toContainText('the key after “#” is missing');
  expect(api.requests.some(([, path]) => path.startsWith('api/'))).toBe(false);
  // Only the fragment changes: the browser does not load the page again by itself.
  await session.goto(BASE + fragment(new Uint8Array(32).fill(9)));
  await session.reload();
  await expect(session.locator('#status')).toContainText('does not belong to the session');
  await expect(session.locator('#download')).toBeHidden();
});

test('a receive-only session has no download list, in Polish for Polish browsers', async ({ browser }) => {
  const page = await browser.newPage({ locale: 'pl-PL' });
  const api = newApi();
  api.info.body = { ...INFO, files: [], expires_in: null };
  await serve(page, api);
  await page.goto(LINK);
  await expect(page.locator('#title')).toHaveText("Wyślij pliki do urządzenia Kate's laptop");
  await expect(page.locator('#download')).toBeHidden();
  await expect(page.locator('#expires')).toHaveText('');
  await page.close();
});

for (const scheme of ['light', 'dark']) {
  test(`the page has names, labels and enough contrast (${scheme} theme)`, async ({ session }) => {
    await session.emulateMedia({ colorScheme: scheme });
    await session.goto(LINK);
    await expect(session.locator('#files li')).toHaveCount(1);
    expect(await session.evaluate(audit, null)).toEqual([]);
    const { violations } = await new AxeBuilder({ page: session }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
    expect(violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
  });
}

test('the page and the Rust side encrypt the same way', async () => {
  // The vector of src/web/seal.rs (test_vector_shared_with_the_page).
  const key = Uint8Array.from({ length: 32 }, (_, i) => i);
  const sealed = sealAll(key, purpose.info, new TextEncoder().encode('{"device":"Kate\'s laptop"}'), Uint8Array.from([1, 2, 3, 4, 5, 6, 7, 8]));
  expect(Buffer.from(sealed).toString('hex')).toBe('0102030405060708da7530383449e3150ef4fe615522431dede1f5dd248679cf1e0399bebb9ce65897d6bf024c428baa4a07');
});
