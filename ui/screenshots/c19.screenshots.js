import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c19', state);
const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
const backToScenes = (page) => page.getByRole('link', { name: '← Scenes' });

/** Waits for the engine-rendered thumbnail on a picker button. */
const painted = (button) =>
  expect
    .poll(() => button.locator('canvas').evaluate((canvas) => canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data.some((value, index) => index % 4 !== 3 && value > 0)))
    .toBe(true);

// On the config's scratch copy of shows/fossile: tell-me, song 3.
test('C19 scenes home and scene editor', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);
  await expect(pad(page, 36).locator('circle').first()).not.toHaveAttribute('fill', '#000000');
  await expect(pad(page, 39).locator('polyline')).toBeVisible();
  await shoot(page, 'scenes-home');

  await pad(page, 37).click();
  await expect(page.getByText('Magenta wash everywhere except inner-wash, amber wash everywhere except outer-wash.')).toBeVisible();
  await expect(page.locator('[data-fixture="wash-2"] circle').first()).not.toHaveAttribute('fill', '#000000');
  await painted(page.getByRole('button', { name: 'Chase', exact: true }));
  await shoot(page, 'scene-editor');

  await backToScenes(page).click();
  await page.getByRole('button', { name: 'New scene' }).click();
  await expect(page.getByLabel('Number')).toHaveValue('1');
  await painted(page.getByRole('button', { name: 'Wash', exact: true }));
  await shoot(page, 'new-scene-takes-the-lowest-free-number');

  await backToScenes(page).click();
  await pad(page, 39).click();
  await expect(page.locator('.curve polyline')).toBeVisible();
  await page.getByRole('button', { name: 'Audition loop' }).click();
  await expect(page.locator('.curve .playhead')).toHaveCount(1);
  await expect(page.locator('[data-fixture="wash-1"] circle').first()).not.toHaveAttribute('fill', '#000000');
  await shoot(page, 'swell-scene-in-the-editor');
  await page.getByRole('button', { name: 'Stop audition' }).click();
});
