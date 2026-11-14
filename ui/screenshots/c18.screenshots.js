import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c18', state);
const pad = (page, note) => page.locator(`[data-pad="${note}"]`);

/** Waits for the engine-rendered thumbnail on a picker button. */
const painted = (button) =>
  expect
    .poll(() => button.locator('canvas').evaluate((canvas) => canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data.some((value, index) => index % 4 !== 3 && value > 0)))
    .toBe(true);

/** Waits for every groove's curve, sampled by the engine. */
async function groovesDrawn(page) {
  for (const groove of ['on the beat', 'off-beat', 'heartbeat', 'backbeat', 'swell']) {
    await expect(page.getByRole('button', { name: `Rhythm: ${groove}` }).locator('polyline').first()).toBeVisible();
  }
}

test.describe('on three rows of washes', () => {
  // The rig with height that plan §16's example look is built for, with that look on note 43.
  test.use({ showSource: 'tests/fixtures/shows/three-rows' });

  test('C18 rhythm per area', async ({ page, startEngine }) => {
    const { url } = await startEngine({ look: DARK });
    await page.goto(`${url}#builder`);
    await page.getByLabel('Song').selectOption('chorus-build');

    await pad(page, 43).click();
    await page.getByRole('button', { name: 'warm-white wash on the left', exact: true }).click();
    await page.getByRole('button', { name: 'Rhythm: swell' }).click();
    await groovesDrawn(page);
    await painted(page.getByRole('button', { name: 'Shape: fill' }));
    await page.getByRole('group', { name: 'Rhythm' }).scrollIntoViewIfNeeded();
    await shoot(page, 'groove-thumbnails-in-the-inspector');

    // The chase's own hits, saved and auditioned so the grid's playhead runs.
    await page.getByRole('button', { name: 'amber chase on the right', exact: true }).click();
    await page.getByRole('button', { name: 'Rhythm: custom' }).click();
    for (const at of [1, 2, 3.5]) await page.getByRole('button', { name: `at ${at}`, exact: true }).click();
    await page.getByRole('button', { name: 'Save cue' }).click();
    await expect(page.getByText('Saved cue 43 in chorus-build.')).toBeVisible();
    await page.getByRole('button', { name: 'Audition loop' }).click();
    await expect(page.locator('.lane .playhead')).toHaveCount(1);
    await groovesDrawn(page);
    await page.locator('.lane').scrollIntoViewIfNeeded();
    await shoot(page, 'custom-pattern-grid-with-playhead');
    await page.getByRole('button', { name: 'Stop audition' }).click();

    await pad(page, 44).click();
    await expect(page.getByText('Nothing on stage yet.')).toBeVisible();
    await painted(page.getByRole('button', { name: 'Hits', exact: true }));
    await painted(page.getByRole('button', { name: 'Swell', exact: true }));
    await shoot(page, 'empty-pad-starts-a-look');
  });
});

// On the config's scratch copy of shows/fossile: tell-me's chorus-amber scene.
test('C18 converted scene', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);

  await pad(page, 37).click();
  await expect(page.getByText('Magenta wash everywhere except inner-wash, amber wash everywhere except outer-wash.')).toBeVisible();
  await page.getByRole('button', { name: 'amber wash everywhere except outer-wash', exact: true }).click();
  await expect(page.locator('[data-crossed-out]')).toHaveCount(2);
  await expect(page.locator('[data-fixture="wash-2"] circle').first()).not.toHaveAttribute('fill', '#000000');
  await painted(page.getByRole('button', { name: 'Shape: fill' }));
  await shoot(page, 'converted-scene-in-the-composer');
});
