// Shared by every chunk's screenshot spec.

import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '../..');

/** The fossile show starts dark, so what a state builds stands out. */
export const DARK = ['--set', 'all-wash', 'dimmer=0%'];

/** Saves the full page as claude-artifacts/screenshots/<chunk>/<state>.png. */
export function capture(page, chunk, state) {
  return page.screenshot({
    path: path.join(ROOT, 'claude-artifacts/screenshots', chunk, `${state}.png`),
    fullPage: true,
  });
}
