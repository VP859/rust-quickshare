// Serves src-tauri/ui with the app's Content-Security-Policy and replaces the
// Tauri API with a mock: commands are recorded in `window.__calls` and answered
// from `responses`, events are sent to the page with `window.__emit`.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test as base, expect } from '@playwright/test';

const UI = join(import.meta.dirname, '..', '..', 'src-tauri', 'ui');
const CSP = JSON.parse(readFileSync(join(UI, '..', 'tauri.conf.json'), 'utf8')).app.security.csp;
export const ORIGIN = 'http://ferry.test';

/** Default answers of the mocked commands. */
const RESPONSES = {
  get_node_info: { node_id: 'a'.repeat(64), ticket: 'ferry:abc', qr_svg: '<svg xmlns="http://www.w3.org/2000/svg"/>', download_dir: '/home/me/Downloads' },
  list_peers: [{ node_id: 'b'.repeat(64), alias: 'Laptop', auto_accept: false, added_at: '2026-10-01T00:00:00Z', has_address: true }],
  get_language: { code: 'en', available: [{ code: 'en', name: 'English' }, { code: 'pl', name: 'Polski' }] },
  list_nearby: [],
  get_device_name: { name: 'Test PC', is_default: false },
  discovery_health: null,
  peer_online: true,
  safety_number: { number: '54414 79221 16121 20603 92559 97490 46181 21597 26358 47392 45394 97560', verified: false },
  set_verified: null,
  app_info: { name: 'Ferry', version: '0.0.0-test' },
  get_network_mode: { active: 'local', saved: 'local', relays: [] },
  get_privacy: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: true, paired_only: false, pairing_open: false, announce_platform: false, random_port: false, strip_metadata: false },
  get_speed_limit: null,
  get_key_storage: { system: false, supported: true },
  set_key_storage: null,
  set_speed_limit: 2097152,
  get_history: [
    { time: '2026-11-04T10:00:00Z', direction: 'download', peer: 'b'.repeat(64), file_name: 'holiday.jpg', size: 2048, status: 'completed', detail: '/home/me/Downloads/holiday.jpg', local_file: '/home/me/Downloads/holiday.jpg', ferry_peer: true },
    { time: '2026-11-04T08:00:00Z', kind: 'text', direction: 'upload', peer: 'b'.repeat(64), file_name: '', size: 27, status: 'completed' },
    { time: '2026-11-03T09:00:00Z', direction: 'upload', peer: 'browser:192.168.1.20', peer_name: 'Web browser (Safari, iPhone) at 192.168.1.20', file_name: 'report.pdf', size: 100, status: 'rejected', detail: 'busy', source: '/home/me/report.pdf', local_file: '/home/me/report.pdf', ferry_peer: false },
  ],
  clear_history: null,
  pause_transfer: true,
  set_pairing_open: null,
  get_welcome: false,
  permission_status: null,
  request_permissions: null,
  open_app_settings: null,
  dismiss_welcome: null,
  list_localsend: [{ alias: "Kate's iPhone", model: 'iPhone', device_type: 'mobile', fingerprint: 'AB12', address: '192.168.1.20:53317', https: true }],
  scan_localsend: [],
  send_to_localsend: null,
  get_quit_on_close: false,
  start_web_share: { url: 'http://192.168.1.10:47835/0123456789abcdef0123456789abcdef/', urls: ['http://192.168.1.10:47835/0123456789abcdef0123456789abcdef/', 'http://10.5.0.2:47835/0123456789abcdef0123456789abcdef/'], qr_svg: '<svg xmlns="http://www.w3.org/2000/svg"/>', expires_in: 1800, files: 0 },
  stop_web_share: null,
  web_share_status: { running: true, owner: null, refused: [] },
  take_shared_files: [],
  take_shared_text: null,
  take_share_target: null,
  pick_folder: null,
  take_picked_folder: null,
  create_backup: '/home/me/ferry-backup-2026-11-11.ferry-backup',
  restore_backup: { node_id: 'a'.repeat(64), key_changed: true, peers_added: 2 },
  'plugin:dialog|save': '/home/me/ferry-backup-2026-11-11.ferry-backup',
  'plugin:autostart|is_enabled': false,
};

/** Installed into the page before its scripts run. */
function installMock(responses) {
  window.__calls = [];
  const listeners = {};
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        window.__calls.push([cmd, args ?? {}]);
        if (cmd === 'set_language') return args.code;
        if (cmd === 'set_network_mode') responses.get_network_mode.saved = args.mode;
        if (cmd === 'set_network_mode' && args.relays) responses.get_network_mode.relays = args.relays;
        return cmd in responses ? structuredClone(responses[cmd]) : null;
      },
    },
    event: {
      listen: async (name, callback) => {
        listeners[name] = [...(listeners[name] ?? []), callback];
        return () => {};
      },
    },
  };
  window.__respond = (cmd, value) => {
    responses[cmd] = value;
  };
  window.__emit = (name, payload) => {
    for (const callback of listeners[name] ?? []) callback({ payload });
  };
}

export const test = base.extend({
  /** Answers that replace the defaults above (`test.use({ responses: … })`). */
  responses: [{}, { option: true }],
  /** The app page with the mocked API; console errors fail the test. */
  app: async ({ page, responses }, use) => {
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
    await page.route(`${ORIGIN}/**`, (route) => {
      const path = new URL(route.request().url()).pathname;
      const file = join(UI, path === '/' ? 'index.html' : path);
      const headers = path === '/' ? { 'Content-Security-Policy': CSP } : {};
      return route.fulfill({ path: file, headers });
    });
    await page.addInitScript(installMock, { ...structuredClone(RESPONSES), ...responses });
    await page.goto(`${ORIGIN}/`);
    await expect(page.locator('#node-status')).toContainText('Online');
    await use(page);
    expect(errors, 'no errors in the console').toEqual([]);
  },
});

/** Commands the page invoked so far, as `[name, args]` pairs. */
export const calls = (page) => page.evaluate(() => window.__calls);

/** Changes the answer of a mocked command. */
export const respond = (page, cmd, value) =>
  page.evaluate(([c, v]) => window.__respond(c, v), [cmd, value]);

/** Sends a Tauri event to the page. */
export const emit = (page, name, payload) =>
  page.evaluate(([n, p]) => window.__emit(n, p), [name, payload]);

export { expect };
