// Browser tests of the GUI frontend (src-tauri/ui) with a mocked Tauri API;
// see tests/gui/. Run with `npm run test:ui` after `npx playwright install chromium`.
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: 'tests/gui',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? 'github' : 'list',
  use: { trace: 'retain-on-failure' },
  projects: [
    { name: 'desktop', use: { ...devices['Desktop Chrome'] } },
    { name: 'android', use: { ...devices['Pixel 7'] } },
  ],
});
