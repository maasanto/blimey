import { steadyAudioClock, withAudio } from '../tests/audio.js';
import { test, expect, DARK, RED, lit, rgbPar, stage } from '../tests/engine.js';
import { capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c24', state);

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

  await stage(page).getByRole('button', { name: 'Large', exact: true }).click();
  await page.locator('.section-editor [data-block="37"]').hover();
  await page.mouse.down();
  await expect(rgbPar(page)).not.toHaveAttribute('fill', '#000000');
  await lit(page, 36);
  await shoot(page, 'holding-block-large');
  await page.mouse.up();

  await page.setViewportSize({ width: 960, height: 800 });
  await lit(page, 36);
  await shoot(page, 'narrow-large');
  await page.setViewportSize({ width: 1280, height: 800 });

  await stage(page).getByRole('button', { name: 'Stage', exact: true }).click();
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
