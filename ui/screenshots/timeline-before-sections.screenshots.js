import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { withAudio } from '../tests/audio.js';
import { test, expect, DARK, lit } from '../tests/engine.js';
import { capture } from './capture.js';

test.use({ showSource: 'tests/fixtures/shows/arranged' });
const shoot = (page, state) => capture(page, 'timeline-before-sections', state);

test('the audio shows and plays before any section is in the arrangement', async ({ page, show, startEngine }) => {
  withAudio(show, 'audio/arranged.wav');
  const songFile = path.join(show, 'songs/arranged.yaml');
  writeFileSync(songFile, readFileSync(songFile, 'utf8').replace('arrangement: [verse, waltz, verse, waltz]\n', ''));
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.locator('.placement.unplaced')).toHaveText('1 · no section yet');
  await lit(page, 36);
  await shoot(page, 'no-section-yet');

  await page.getByRole('button', { name: 'Add verse to the arrangement' }).click();
  await expect(page.locator('.placement.unplaced')).toHaveText('3 · no section yet');
  await lit(page, 36);
  await shoot(page, 'first-section-placed');
});
