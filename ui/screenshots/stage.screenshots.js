import { steadyAudioClock, withAudio } from '../tests/audio.js';
import { test, expect } from '../tests/engine.js';
import { capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c24', state);
const DARK = ['--set', 'both', 'dimmer=0%'];
const RED = '#ff0000';
const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
/** The engine-rendered thumbnail of a scene pad, lit. */
const lit = (page, number) => expect(pad(page, number).locator('circle').first()).not.toHaveAttribute('fill', '#000000');
const stage = (page) => page.getByRole('complementary', { name: 'Stage' });
/** The stage view's RGB par, which the dark look leaves black, in the colour the engine sends. */
const rgbPar = (page) => stage(page).locator('[data-fixture="rgb-par"] circle');

test.use({ showSource: 'tests/fixtures/shows/arranged' });
test.beforeEach(({ page }) => steadyAudioClock(page));

test('the stage view follows playback and auditions on the timeline', async ({ page, show, startEngine }) => {
  withAudio(show, 'audio/arranged.wav');
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByLabel('Waveform')).toBeVisible();
  await lit(page, 36);
  await shoot(page, 'dark');

  await page.getByRole('button', { name: 'Play' }).click();
  await expect(rgbPar(page)).toHaveAttribute('fill', RED);
  await lit(page, 36);
  await shoot(page, 'playing');
  await page.getByRole('button', { name: 'Pause' }).click();

  await stage(page).getByRole('button', { name: 'Larger' }).click();
  await page.locator('.section-editor [data-block="37"]').hover();
  await page.mouse.down();
  await expect(rgbPar(page)).not.toHaveAttribute('fill', '#000000');
  await lit(page, 36);
  await shoot(page, 'holding-block-large');
  await page.mouse.up();

  await stage(page).getByRole('button', { name: 'Hide stage' }).click();
  await lit(page, 36);
  await shoot(page, 'hidden');
});

test('show mode shows the stage view', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: ['--set', 'both', 'color=red'], edit: false });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(rgbPar(page)).toHaveAttribute('fill', RED);
  await lit(page, 36);
  await shoot(page, 'show-mode');
});
