import { readFileSync } from 'node:fs';
import path from 'node:path';
import { steadyAudioClock } from './audio.js';
import { test, expect, DARK, RED, pad, rgbPar, stage } from './engine.js';

// The C20 fixture: verse plays red (36) from bar 1, with blue (37) at 50% on bar 2 beat 3.
test.use({ showSource: 'tests/fixtures/shows/arranged' });
test.beforeEach(({ page }) => steadyAudioClock(page));
const BLACK = '#000000';
const BLUE = '#0000ff';

test('playing from bar 1 lights the stage view, which stays on through playback and section switches', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(rgbPar(page)).toHaveAttribute('fill', BLACK);
  const view = await stage(page).getByRole('img', { name: 'Front view' }).elementHandle();

  await page.getByRole('button', { name: 'Play' }).click();
  await expect(rgbPar(page)).toHaveAttribute('fill', RED);
  await page.getByRole('button', { name: /^waltz 2 bars/ }).click();
  await expect(page.getByLabel('Name', { exact: true })).toHaveValue('waltz');
  await expect(page.getByTestId('playhead')).toContainText('Bar 3');
  await page.getByRole('button', { name: 'Pause' }).click();
  await expect(rgbPar(page), 'pausing releases the scenes').toHaveAttribute('fill', BLACK);
  expect(await view.evaluate((node) => node.isConnected), 'the same view all along').toBe(true);
});

test('holding a block or a scene pad shows its scene on the stage view until let go', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(rgbPar(page)).toHaveAttribute('fill', BLACK);

  await page.locator('.section-editor [data-block="36"]').hover();
  await page.mouse.down();
  await expect(rgbPar(page)).toHaveAttribute('fill', RED);
  await page.mouse.up();
  await expect(rgbPar(page)).toHaveAttribute('fill', BLACK);

  await pad(page, 37).hover();
  await page.mouse.down();
  await expect(rgbPar(page)).toHaveAttribute('fill', BLUE);
  await page.mouse.up();
  await expect(rgbPar(page)).toHaveAttribute('fill', BLACK);
});

test("the stage view's toggles enlarge and hide it, keeping the focus", async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  const view = stage(page).getByRole('img', { name: 'Front view' });
  const large = stage(page).getByRole('button', { name: 'Large', exact: true });
  const shown = stage(page).getByRole('button', { name: 'Stage', exact: true });
  const small = (await view.boundingBox()).width;

  await large.click();
  await expect(large).toHaveAttribute('aria-pressed', 'true');
  await expect(large).toBeFocused();
  await expect.poll(async () => (await view.boundingBox()).width).toBeGreaterThan(small);
  await large.press('Enter');
  await expect(large).toHaveAttribute('aria-pressed', 'false');
  await expect.poll(async () => (await view.boundingBox()).width).toBe(small);

  await shown.click();
  await expect(shown).toHaveAttribute('aria-expanded', 'false');
  await expect(shown).toBeFocused();
  await expect(view).toBeHidden();
  await shown.press('Enter');
  await expect(shown).toHaveAttribute('aria-expanded', 'true');
  await expect(view).toBeVisible();
});

test('a fixture dragged on the stage view stays put: rig.yaml is untouched', async ({ page, show, startEngine }) => {
  const rigFile = path.join(show, 'rig.yaml');
  const before = readFileSync(rigFile, 'utf8');
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);

  const box = await rgbPar(page).boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x - 60, box.y + 40, { steps: 5 });
  await page.mouse.up();
  await page.waitForTimeout(500);
  expect(readFileSync(rigFile, 'utf8')).toBe(before);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('show mode still shows the stage view, read-only, with what the engine sends', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: ['--set', 'both', 'color=red'], edit: false });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByRole('button', { name: 'Play' })).toBeDisabled();
  await expect(rgbPar(page)).toHaveAttribute('fill', RED);
});
