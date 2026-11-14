import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

const shoot = (page, state) => capture(page, 'ux', state);
const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
const WIDE = { width: 1920, height: 1080 };

/** Waits for the engine-rendered thumbnail on a picker button. */
const painted = (button) =>
  expect
    .poll(() => button.locator('canvas').evaluate((canvas) => canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data.some((value, index) => index % 4 !== 3 && value > 0)))
    .toBe(true);

async function stagePoint(page, [x, y]) {
  const stage = await page.getByRole('img', { name: 'Front view' }).locator('rect.stage').boundingBox();
  return { x: stage.x + x * stage.width, y: stage.y + (1 - y) * stage.height };
}

// On the config's scratch copy of shows/fossile: tell-me, song 3.
test('Fossile: monitor, patch, front view, scenes home and the old cue types', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });

  await page.goto(`${url}#monitor`);
  await expect(page.getByTestId('engine-time')).not.toHaveText('—');
  await shoot(page, 'monitor');

  await page.goto(`${url}#patch`);
  await expect(page.locator('[data-fixture]').first()).toBeVisible();
  await shoot(page, 'patch');

  await page.goto(`${url}#front`);
  await expect(page.locator('[data-fixture="wash-1"] circle').first()).toBeVisible();
  await shoot(page, 'front-view');

  await page.goto(`${url}#builder`);
  await expect(pad(page, 36).locator('circle').first()).not.toHaveAttribute('fill', '#000000');
  await expect(pad(page, 39).locator('polyline')).toBeVisible();
  await shoot(page, 'scenes-home');

  await page.getByRole('button', { name: 'Audition scene 36' }).click();
  await expect(pad(page, 36)).toHaveClass(/playing/);
  await shoot(page, 'scenes-home-playing');
  await page.getByRole('button', { name: 'Release scene 36' }).click();

  await pad(page, 37).click();
  await expect(page.getByText('Magenta wash everywhere except inner-wash, amber wash everywhere except outer-wash.')).toBeVisible();
  await expect(page.locator('[data-fixture="wash-2"] circle').first()).not.toHaveAttribute('fill', '#000000');
  await painted(page.getByRole('button', { name: 'Chase', exact: true }));
  await shoot(page, 'converted-scene');

  await page.goto(`${url}#builder/tell-me/38`);
  await expect(page.getByRole('status')).toContainText('fixed look');
  await shoot(page, 'fixed-look-read-only');

  await page.goto(`${url}#builder/tell-me/39`);
  await expect(page.locator('.curve polyline')).toBeVisible();
  await page.getByRole('button', { name: 'Audition loop' }).click();
  await expect(page.locator('.curve .playhead')).toHaveCount(1);
  await page.waitForTimeout(1500);
  await shoot(page, 'swell-scene');
  await page.getByRole('button', { name: 'Stop audition' }).click();

  await page.goto(`${url}#builder/tell-me/42`);
  await expect(page.locator('.hit-level').first()).toBeVisible();
  await shoot(page, 'pattern-scene');
});

test('Fossile: show mode is read-only', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK, edit: false });
  await page.goto(`${url}#builder`);
  await expect(pad(page, 36).locator('circle').first()).not.toHaveAttribute('fill', '#000000');
  await shoot(page, 'scenes-home-show-mode');
});

test.describe('on three rows of washes', () => {
  test.use({ showSource: 'tests/fixtures/shows/three-rows' });

  test('the stage composer', async ({ page, startEngine }) => {
    const { url } = await startEngine({ look: DARK });

    await page.goto(`${url}#builder/chorus-build/43`);
    await expect(page.getByText('Warm-white wash on the left, amber chase on the right')).toBeVisible();
    await expect(page.locator('[data-fixture="low-1"] circle').first()).not.toHaveAttribute('fill', '#000000');
    await painted(page.getByRole('button', { name: 'Chase', exact: true }));
    await shoot(page, 'scene-editor');

    const chase = await stagePoint(page, [0.75, 0.3]);
    await page.mouse.click(chase.x, chase.y);
    await expect(page.locator('circle.caught')).not.toHaveCount(0);
    await painted(page.getByRole('button', { name: 'Shape: bar' }));
    await painted(page.getByRole('button', { name: 'Motion: chase' }));
    await shoot(page, 'scene-editor-area-selected');

    await page.setViewportSize(WIDE);
    await shoot(page, 'scene-editor-area-selected-1920');
    await page.setViewportSize({ width: 1280, height: 800 });

    await page.getByRole('button', { name: /steel-blue sparkle/ }).first().click();
    await expect(page.locator('[data-crossed-out]')).toHaveCount(2);
    await page.getByRole('button', { name: 'Rhythm: swell' }).click();
    await expect(page.locator('.curve polyline')).toBeVisible();
    await painted(page.getByRole('button', { name: 'Shape: fill' }));
    await page.getByRole('group', { name: 'Rhythm' }).scrollIntoViewIfNeeded();
    await shoot(page, 'scene-editor-rhythm-and-mute');

    page.once('dialog', (dialog) => dialog.accept());
    await page.goto(`${url}#builder/chorus-build/44`);
    await expect(page.getByText('Nothing on stage yet.')).toBeVisible();
    await painted(page.getByRole('button', { name: 'Hits', exact: true }));
    await shoot(page, 'new-scene-empty');

    const sweep = await page.getByRole('group', { name: 'Ingredients' }).getByRole('button', { name: 'Sweep' }).boundingBox();
    await page.mouse.move(sweep.x + sweep.width / 2, sweep.y + sweep.height / 2);
    await page.mouse.down();
    const drop = await stagePoint(page, [0.5, 0.5]);
    await page.mouse.move(drop.x, drop.y, { steps: 5 });
    await page.waitForTimeout(300);
    await shoot(page, 'new-scene-dragging-ingredient');
    await page.mouse.up();
  });
});
