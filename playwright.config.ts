import { defineConfig } from '@playwright/test';
export default defineConfig({ testDir: './tests/desktop', timeout: 60_000, workers: 1,
  use: { baseURL: 'http://127.0.0.1:1430', viewport: { width: 1440, height: 960 } },
  outputDir: '.cache/playwright-results' });
