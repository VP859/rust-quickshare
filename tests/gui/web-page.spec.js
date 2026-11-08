// The page a browser sees in a Ferry browser session (src/web/page), served
// with the server's Content-Security-Policy and a mocked `api/` of the
// session. The real server is tested in tests/web.rs.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test as base, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { audit } from './audit.js';

const PAGE = join(import.meta.dirname, '..', '..', 'src', 'web', 'page');
const SERVER = readFileSync(join(import.meta.dirname, '..', '..', 'src', 'web', 'server.rs'), 'utf8');
// The policy exactly as the server sends it.
const CSP = SERVER.match(/const CSP: &str = "([^"]+)"/s)[1].replace(/\\\s+/g, '');
const BASE = 'http://192.168.1.10:47835/0123456789abcdef0123456789abcdef/';

const INFO = { device: "Kate's laptop", files: [{ id: 0, name: 'report.pdf', size: 2_500_000 }], receive: true, expires_in: 1500, max_files: 3 };

/** The page with a mocked `api/` (answers in `api`, which records the requests). */
const test = base.extend({
  // biome-ignore lint/correctness/noEmptyPattern: Playwright fixtures take their dependencies as an object pattern.
  api: async ({}, use) => use({ info: { status: 200, body: INFO }, offer: { status: 200, body: { id: 'off1' } }, requests: [] }),
  session: async ({ page, api }, use) => {
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('console', (m) => { if (m.type() === 'error' && !/status of (403|404|410|429)/.test(m.text())) errors.push(m.text()); });
    await page.route(`${BASE}**`, async (route) => {
      const request = route.request();
      const path = request.url().slice(BASE.length);
      api.requests.push([request.method(), path, request.postData()]);
      if (path === '') return route.fulfill({ path: join(PAGE, 'index.html'), headers: { 'Content-Security-Policy': CSP } });
      if (path === 'page.js' || path === 'page.css') return route.fulfill({ path: join(PAGE, path) });
      const reply = path === 'api/info' ? api.info : path === 'api/offer' ? api.offer : path.startsWith('api/upload/') ? { status: 200, body: { ok: true } } : { status: 404, body: { message: 'Not found' } };
      return route.fulfill({ status: reply.status, contentType: 'application/json', body: JSON.stringify(reply.body) });
    });
    await use(page);
    expect(errors, 'no errors in the console').toEqual([]);
  },
});

test.use({ locale: 'en-US' });

test('shows the device, the shared files and how long the link works', async ({ session }) => {
  await session.goto(BASE);
  await expect(session.locator('#title')).toHaveText("Files from Kate's laptop");
  await expect(session.locator('#files li')).toHaveCount(1);
  await expect(session.locator('#files')).toContainText('report.pdf');
  await expect(session.locator('#files')).toContainText('2.4 MB');
  await expect(session.locator('#files a')).toHaveAttribute('href', 'api/files/0');
  await expect(session.locator('#upload-title')).toHaveText("Send to Kate's laptop");
  await expect(session.locator('#expires')).toHaveText('This link works for 25 more minute(s).');
  await expect(session.locator('footer')).toContainText('not encrypted');
});

test('files are offered first and uploaded one by one after acceptance', async ({ session, api }) => {
  await session.goto(BASE);
  await session.locator('#picker').setInputFiles([
    { name: 'a.jpg', mimeType: 'image/jpeg', buffer: Buffer.from('aaaa') },
    { name: 'b.txt', mimeType: 'text/plain', buffer: Buffer.from('bb') },
  ]);
  await expect(session.locator('#status')).toHaveText('All files were sent.');
  await expect(session.locator('#uploads li.done')).toHaveCount(2);
  const offer = api.requests.find(([, path]) => path === 'api/offer');
  expect(JSON.parse(offer[2])).toEqual({ files: [{ name: 'a.jpg', size: 4 }, { name: 'b.txt', size: 2 }] });
  const uploads = api.requests.filter(([m, path]) => m === 'PUT' && path.startsWith('api/upload/')).map(([, p]) => p);
  expect(uploads).toEqual(['api/upload/off1/0', 'api/upload/off1/1']);
});

test('a declined offer uploads nothing', async ({ session, api }) => {
  api.offer = { status: 403, body: { message: 'The files were declined.' } };
  await session.goto(BASE);
  await session.locator('#picker').setInputFiles([{ name: 'a.jpg', mimeType: 'image/jpeg', buffer: Buffer.from('a') }]);
  await expect(session.locator('#status')).toHaveText("Kate's laptop declined the files.");
  await expect(session.locator('#status')).toHaveClass(/error/);
  await expect(session.locator('#uploads li.failed')).toHaveCount(1);
  expect(api.requests.some(([m]) => m === 'PUT')).toBe(false);
});

test('too many files are refused before asking', async ({ session, api }) => {
  await session.goto(BASE);
  const files = [1, 2, 3, 4].map((i) => ({ name: `${i}.txt`, mimeType: 'text/plain', buffer: Buffer.from('x') }));
  await session.locator('#picker').setInputFiles(files);
  await expect(session.locator('#status')).toHaveText('At most 3 files at once.');
  expect(api.requests.some(([, path]) => path === 'api/offer')).toBe(false);
});

test('a link used by another device says so', async ({ session, api }) => {
  api.info = { status: 403, body: { message: 'This link is already used by another device. Start a new session in Ferry.' } };
  await session.goto(BASE);
  await expect(session.locator('#status')).toContainText('already used by another device');
  await expect(session.locator('#upload')).toBeHidden();
  await expect(session.locator('#download')).toBeHidden();
});

test('a receive-only session has no download list, in Polish for Polish browsers', async ({ browser }) => {
  const page = await browser.newPage({ locale: 'pl-PL' });
  await page.route(`${BASE}**`, (route) => {
    const path = route.request().url().slice(BASE.length);
    if (path === '') return route.fulfill({ path: join(PAGE, 'index.html'), headers: { 'Content-Security-Policy': CSP } });
    if (path === 'page.js' || path === 'page.css') return route.fulfill({ path: join(PAGE, path) });
    return route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...INFO, files: [], expires_in: null }) });
  });
  await page.goto(BASE);
  await expect(page.locator('#title')).toHaveText("Wyślij pliki do urządzenia Kate's laptop");
  await expect(page.locator('#download')).toBeHidden();
  await expect(page.locator('#expires')).toHaveText('');
  await page.close();
});

for (const scheme of ['light', 'dark']) {
  test(`the page has names, labels and enough contrast (${scheme} theme)`, async ({ session }) => {
    await session.emulateMedia({ colorScheme: scheme });
    await session.goto(BASE);
    await expect(session.locator('#files li')).toHaveCount(1);
    expect(await session.evaluate(audit, null)).toEqual([]);
    const { violations } = await new AxeBuilder({ page: session }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
    expect(violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
  });
}
