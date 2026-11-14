import { appendFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c16', state);
const pad = (page, note) => page.locator(`[data-pad="${note}"]`);

test('C16 pad bank', async ({ page, show, startEngine }) => {
  // A cue below note 36 in the scratch copy, so the bank switcher marks a second bank.
  appendFileSync(path.join(show, 'songs/tell-me.yaml'), '  1:\n    name: house-warm\n    type: scene\n    values:\n      all-wash: { color: warm-white, dimmer: 40% }\n');
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);
  await page.locator('.bar select').selectOption('tell-me');
  await expect(pad(page, 36).locator('circle').first()).not.toHaveAttribute('fill', '#000000');
  await expect(pad(page, 39).locator('polyline')).toBeVisible();
  await shoot(page, 'pad-bank');

  await page.getByRole('button', { name: 'Notes 0–3, 1 cue' }).click();
  await expect(pad(page, 1).locator('circle').first()).not.toHaveAttribute('fill', '#000000');
  await shoot(page, 'bank-switcher');

  await page.getByRole('button', { name: 'Notes 36–51, 6 cues' }).click();
  await pad(page, 40).hover();
  await page.mouse.down();
  await expect(pad(page, 40)).toHaveClass(/playing/);
  await page.waitForTimeout(500);
  await shoot(page, 'pad-held-auditioning');
  await page.mouse.up();

  await pad(page, 40).click();
  await expect(page.getByText('Cue 40', { exact: true })).toBeVisible();
  const gradient = page.getByRole('button', { name: 'Layer 1 shape: gradient' }).locator('canvas');
  await expect
    .poll(() => gradient.evaluate((canvas) => canvas.getContext('2d').getImageData(0, 0, 12, 12).data.some((value, index) => index % 4 !== 3 && value > 0)))
    .toBe(true);
  await shoot(page, 'editor-opened-from-pad');
});
