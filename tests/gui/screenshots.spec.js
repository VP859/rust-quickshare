// Screenshots and the animation in the README (docs/images/), made from the
// real frontend with example data. Skipped unless FERRY_SCREENSHOTS names the
// output folder:
//
//   FERRY_SCREENSHOTS=docs/images npx playwright test tests/gui/screenshots.spec.js
//
// The animation is assembled from the frames by scripts/make-demo-gif.sh.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from './fixtures.js';

const OUT = process.env.FERRY_SCREENSHOTS;
const VERSION = readFileSync(join(import.meta.dirname, '..', '..', 'Cargo.toml'), 'utf8').match(/^version = "([^"]+)"/m)[1];
const LAPTOP = '3c7f'.repeat(16);
const PHONE = '9a41'.repeat(16);
const KITCHEN = 'e25b'.repeat(16);

test.skip(!OUT, 'set FERRY_SCREENSHOTS to make the screenshots');

test.use({
  responses: {
    get_node_info: {
      node_id: '7f3e2a9c41d0b85e6c3f1a2b9d4e7c0815f2a6b3c9d1e4f7a0b2c5d8e1f4a7b0',
      ticket: 'ferry:example', qr_svg: '<svg xmlns="http://www.w3.org/2000/svg"/>', download_dir: '/home/alex/Downloads',
    },
    list_peers: [
      { node_id: LAPTOP, alias: 'Work laptop', auto_accept: true, verified: true, added_at: '2026-10-01T00:00:00Z', has_address: true },
      { node_id: PHONE, alias: "Alex's phone", auto_accept: false, verified: true, added_at: '2026-10-02T00:00:00Z', has_address: true },
    ],
    get_device_name: { name: 'Desktop', is_default: false },
    app_info: { name: 'Ferry', version: VERSION },
  },
});

const nearby = [
  { node_id: PHONE, name: "Alex's phone", platform: 'android', addr: null },
  { node_id: KITCHEN, name: 'Kitchen PC', platform: 'linux', addr: null },
];

const event = (id, peer, file_name, direction, total, done, status, route = 'direct') =>
  ({ id, peer, peer_name: null, file_name, direction, total, done, status, route });

/** Shows the main window with nearby devices and a few transfers. */
async function populate(app) {
  await app.evaluate((list) => window.__emit('nearby', list), nearby);
  await app.evaluate((events) => { for (const e of events) window.__emit('transfer', e); }, [
    event('t1', PHONE, 'IMG_2041.jpg', 'download', 4_200_000, 4_200_000, { kind: 'completed', path: '/home/alex/Pictures/Ferry/IMG_2041.jpg' }),
    event('t2', LAPTOP, 'Holiday 2026', 'upload', 900_000_000, 900_000_000, { kind: 'completed', path: null }),
    event('t3', LAPTOP, 'presentation.pdf', 'upload', 52_000_000, 31_000_000, { kind: 'running' }),
  ]);
  await expect(app.locator('#peers .device')).toHaveCount(2);
}

for (const [scheme, project] of [['light', 'desktop'], ['dark', 'desktop'], ['light', 'android']]) {
  test(`main window (${project}, ${scheme})`, async ({ app }, info) => {
    test.skip(info.project.name !== project);
    await app.emulateMedia({ colorScheme: scheme });
    await populate(app);
    // Colours change with a short transition: wait for it to end.
    await app.waitForTimeout(400);
    await app.screenshot({ path: `${OUT}/gui-${project}-${scheme}.png`, fullPage: project === 'desktop' });
  });
}

test('incoming offer and settings', async ({ app }, info) => {
  test.skip(info.project.name !== 'desktop');
  await populate(app);
  await app.evaluate(() => window.__emit('incoming-offer', {
    id: 'o1', peer: '9a41'.repeat(16), alias: "Alex's phone", file_name: '', size: 18_400_000, count: 3,
    names: ['IMG_2042.jpg', 'IMG_2043.jpg', 'VID_2044.mp4'], risk: null, localsend: null, browser: null,
  }));
  await expect(app.locator('#modal-offer')).toHaveClass(/active/);
  await app.locator('#modal-offer .modal').screenshot({ path: `${OUT}/gui-offer.png` });
});

test('animation frames', async ({ app }, info) => {
  test.skip(info.project.name !== 'desktop');
  let frame = 0;
  const shot = async (repeat = 1) => {
    for (let i = 0; i < repeat; i++) {
      await app.screenshot({ path: `${OUT}/frames/${String(frame++).padStart(3, '0')}.png` });
    }
  };
  await app.evaluate((list) => window.__emit('nearby', list), nearby);
  await expect(app.locator('#peers .device')).toHaveCount(2);
  await shot(4);
  // Choose files: the recipient list opens.
  await app.evaluate(() => window.__respond('plugin:dialog|open', ['/home/alex/Videos/trip.mp4', '/home/alex/Pictures/map.png']));
  await app.locator('#btn-select-file').click();
  await expect(app.locator('#modal-recipient')).toHaveClass(/active/);
  await shot(4);
  await app.locator('#modal-recipient').getByRole('button', { name: /Work laptop/ }).click();
  await app.mouse.move(0, 0);
  // The transfer runs and completes: show the transfers list.
  await app.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
  for (const [done, map] of [[0, 0], [60, 0], [150, 0], [240, 0], [310, 0], [310, 1]]) {
    const mb = 1_000_000;
    await app.evaluate((events) => { for (const e of events) window.__emit('transfer', e); }, [
      event('v', LAPTOP, 'trip.mp4', 'upload', 310 * mb, done * mb, done === 310 ? { kind: 'completed', path: null } : { kind: 'running' }),
      ...(done === 310 ? [event('m', LAPTOP, 'map.png', 'upload', 2 * mb, map * 2 * mb, map ? { kind: 'completed', path: null } : { kind: 'running' })] : []),
    ]);
    await shot(2);
  }
  await shot(6);
});
