import { steadyAudioClock, withAudio } from '../tests/audio.js';
import { test, expect } from '../tests/engine.js';
import { capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c22', state);
const DARK = ['--set', 'both', 'dimmer=0%'];
const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
/** The engine-rendered thumbnail of a scene pad, lit. */
const lit = (page, number) => expect(pad(page, number).locator('circle').first()).not.toHaveAttribute('fill', '#000000');

test.use({ showSource: 'tests/fixtures/shows/arranged' });
test.beforeEach(({ page }) => steadyAudioClock(page));

test('playing with the audio, and setting where bar 1 falls', async ({ page, show, startEngine }) => {
  withAudio(show, 'audio/arranged.wav');
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByLabel('Waveform')).toBeVisible();
  await lit(page, 36);
  await shoot(page, 'waveform');

  await page.getByRole('button', { name: 'Play' }).click();
  await expect(page.getByTestId('playhead')).toContainText('Bar 2');
  await expect(page.locator('.section-editor [data-block="37"]')).toHaveClass(/playing/);
  await lit(page, 36);
  await shoot(page, 'playing');
  await page.getByRole('button', { name: 'Pause' }).click();

  const box = await page.getByLabel('Waveform').boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 - box.width / 28, box.y + box.height / 2, { steps: 4 });
  await page.mouse.up();
  await expect(page.locator('.notice')).toContainText('Bar 1 now falls');
  await lit(page, 36);
  await shoot(page, 'offset-set');
});

test('without audio, and with a missing file, it plays with a click', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await lit(page, 36);
  await shoot(page, 'no-audio');

  withAudio(show, 'audio/missing.wav', { isMissing: true });
  await page.reload();
  await expect(page.getByTestId('audio')).toContainText('No file at audio/missing.wav');
  await lit(page, 36);
  await shoot(page, 'missing-audio');
});

test('show mode disables playback', async ({ page, show, startEngine }) => {
  withAudio(show, 'audio/arranged.wav');
  const { url } = await startEngine({ look: DARK, edit: false });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByLabel('Waveform')).toBeVisible();
  await lit(page, 36);
  await shoot(page, 'show-mode');
});
