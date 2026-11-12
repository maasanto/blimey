import { defineConfig } from '@playwright/test';

// Each test starts its own engine (see tests/engine.js); build it first with `cargo build`.
export default defineConfig({
  testDir: 'tests',
  use: { browserName: 'chromium', viewport: { width: 1280, height: 900 } },
  reporter: process.env.CI ? 'github' : 'list',
});
