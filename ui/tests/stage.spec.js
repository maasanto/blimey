import { steadyAudioClock } from './audio.js';
import { test, expect } from './engine.js';

// The C20 fixture: verse plays red (36) from bar 1, with blue (37) at 50% on bar 2 beat 3.
test.use({ showSource: 'tests/fixtures/shows/arranged' });
test.beforeEach(({ page }) => steadyAudioClock(page));
const DARK = ['--set', 'both', 'dimmer=0%'];
const BLACK = '#000000';
const RED = '#ff0000';

const stage = (page) => page.getByRole('complementary', { name: 'Stage' });
/** The colour the stage view gives the RGB par, which has no dimmer: the dark look leaves it black. */
const rgbPar = (page) => stage(page).locator('[data-fixture="rgb-par"] circle');

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

test('holding a block or a scene pad shows its scene on the stage view', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(rgbPar(page)).toHaveAttribute('fill', BLACK);

  await page.locator('.section-editor [data-block="36"]').hover();
  await page.mouse.down();
  await expect(rgbPar(page)).toHaveAttribute('fill', RED);
  await page.mouse.up();
  await expect(rgbPar(page)).toHaveAttribute('fill', BLACK);

  await page.getByRole('button', { name: 'Audition scene 37' }).click();
  await expect(rgbPar(page)).toHaveAttribute('fill', '#0000ff');
  await page.getByRole('button', { name: 'Release scene 37' }).click();
  await expect(rgbPar(page)).toHaveAttribute('fill', BLACK);
});

test('the stage view enlarges, shrinks and hides', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  const view = stage(page).getByRole('img', { name: 'Front view' });
  const small = (await view.boundingBox()).width;

  await stage(page).getByRole('button', { name: 'Larger' }).click();
  await expect.poll(async () => (await view.boundingBox()).width).toBeGreaterThan(small);
  await stage(page).getByRole('button', { name: 'Smaller' }).click();
  await expect.poll(async () => (await view.boundingBox()).width).toBe(small);
  await stage(page).getByRole('button', { name: 'Hide stage' }).click();
  await expect(view).toBeHidden();
  await stage(page).getByRole('button', { name: 'Show stage' }).click();
  await expect(view).toBeVisible();
});

test('show mode still shows the stage view, read-only, with what the engine sends', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: ['--set', 'both', 'color=red'], edit: false });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByRole('button', { name: 'Play' })).toBeDisabled();
  await expect(rgbPar(page)).toHaveAttribute('fill', RED);
});
