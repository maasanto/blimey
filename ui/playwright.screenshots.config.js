import { defineConfig } from '@playwright/test';

// `pnpm screenshots`: PR screenshots, not tests. Each chunk's spec in screenshots/ walks its UI
// states on a scratch copy of tests/fixtures/shows/demo and saves full-page PNGs to
// claude-artifacts/screenshots/<chunk>/<state>.png. Build the engine first with `cargo build`.
export default defineConfig({
  testDir: 'screenshots',
  testMatch: '*.screenshots.js',
  use: {
    browserName: 'chromium',
    viewport: { width: 1280, height: 800 },
    deviceScaleFactor: 2,
    showSource: 'tests/fixtures/shows/demo',
  },
  reporter: 'list',
});
