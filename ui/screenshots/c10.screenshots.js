import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c10', state);

/** Fossile has no pixel grid yet: hang a 16×8 one on Art-Net under the washes in the scratch copy. */
function addGrid(show) {
  const rig = path.join(show, 'rig.yaml');
  const grid =
    '- { name: back-grid, type: pixel-grid, columns: 16, rows: 8, colour_order: RGB, output: artnet, universe: 1, address: 1, canvas: { x: 0.15, y: 0.15, width: 0.7, height: 0.45 } }\n';
  writeFileSync(rig, readFileSync(rig, 'utf8').replace('groups:', `${grid}groups:`));
}

const isPainted = (button) =>
  button.locator('canvas').evaluate((canvas) => {
    const { data } = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height);
    return data.some((value, index) => index % 4 !== 3 && value > 0);
  });

test('C10 pixel scene editor', async ({ page, show, startEngine }) => {
  addGrid(show);
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);

  await page.getByLabel('Type').selectOption('pixel');
  await page.getByLabel('Note').fill('44');
  await page.getByLabel('Name').fill('bridge-sweep');
  await expect.poll(() => isPainted(page.getByRole('button', { name: 'Layer 1 shape: circle' }))).toBe(true);
  await shoot(page, 'new-pixel-cue');

  await page.getByRole('button', { name: 'Layer 1 shape: gradient' }).click();
  await page.getByRole('button', { name: 'Layer 1 effect: move' }).click();
  await page.getByRole('button', { name: 'Layer 1 colour: deep-blue' }).click();
  await page.getByLabel('Layer 2', { exact: true }).check();
  await page.getByRole('button', { name: 'Layer 2 shape: bar' }).click();
  await page.getByRole('button', { name: 'Layer 2 effect: chase' }).click();
  await page.getByRole('button', { name: 'Layer 2 colour: amber' }).click();
  await page.getByLabel('Blend').selectOption('add');
  await expect.poll(() => isPainted(page.getByRole('button', { name: 'Layer 2 effect: sparkle' }))).toBe(true);
  await expect(page.locator('[data-grid="back-grid"] rect').first()).not.toHaveAttribute('fill', '#000000');
  await shoot(page, 'two-layer-live-preview');

  await page.getByRole('link', { name: 'Front view' }).click();
  await shoot(page, 'front-view-grid-preview');
  await page.goBack();

  await page.getByRole('button', { name: 'Save cue' }).click();
  await expect(page.getByText('Saved cue 44 in tell-me.')).toBeVisible();
  await page.getByLabel('Swell or pattern to link').selectOption('42');
  await page.getByRole('button', { name: 'Link to intensity' }).click();
  await expect(page.getByText('driven by cue 42, verse-pulse (pattern)')).toBeVisible();
  await shoot(page, 'intensity-linked');
});
