import { defineConfig } from '@playwright/test';
if (!process.env.QT_BROWSER_BASE_URL) throw new Error('Use npm run test:browser to start the isolated Rust fixture.');
export default defineConfig({
  testDir: './tests/browser', fullyParallel: false, workers: 1, timeout: 30000,
  testIgnore: '**/performance.spec.mjs',
  forbidOnly: true, retries: 0, reporter: [['list'], ['./scripts/tests/ui-browser-reporter.mjs']],
  use: { baseURL: process.env.QT_BROWSER_BASE_URL, trace: 'retain-on-failure', screenshot: 'only-on-failure' },
  projects: [
    {name: 'chromium-desktop', use: {browserName: 'chromium', viewport: {width: 1280, height: 900}}},
    {name: 'chromium-mobile', use: {browserName: 'chromium', viewport: {width: 390, height: 844}}},
    {name: 'firefox-desktop', use: {browserName: 'firefox', viewport: {width: 1280, height: 900}}},
    {name: 'firefox-mobile', use: {browserName: 'firefox', viewport: {width: 390, height: 844}}},
  ],
});
