import { markedWav } from '../tests/audio.js';
import { test, expect } from '../tests/engine.js';
import { capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c23', state);
const DARK = ['--set', 'both', 'dimmer=0%'];
const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
/** The engine-rendered thumbnail of a scene pad, lit. */
const lit = (page, number) => expect(pad(page, number).locator('circle').first()).not.toHaveAttribute('fill', '#000000');
const MARKED_WAV = {
  name: 'arranged.wav',
  mimeType: 'audio/wav',
  buffer: markedWav(16, [
    { name: 'Waltz', at: 4 },
    { name: 'Second verse', at: 7 },
  ]),
};

test.use({ showSource: 'tests/fixtures/shows/arranged' });

test('uploading a bounce with its markers, then editing them', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await lit(page, 36);
  await shoot(page, 'no-audio');

  await page.getByLabel('Audio file').setInputFiles(MARKED_WAV);
  await expect(page.getByLabel('Waveform')).toBeVisible();
  await expect(page.locator('.overview [data-marker="Second verse"]')).toBeVisible();
  await page.getByRole('button', { name: 'waltz from bar 3' }).click();
  await page.getByRole('button', { name: 'Marker Waltz, bar 3' }).click();
  await lit(page, 36);
  await shoot(page, 'uploaded-markers');

  await page.getByRole('button', { name: 'Import markers from audio' }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await lit(page, 36);
  await shoot(page, 'replace-asks');
  await page.getByRole('button', { name: "Keep the song's" }).click();

  await page.getByRole('button', { name: 'verse from bar 1' }).click();
  const ruler = await page.getByTestId('ruler').boundingBox();
  await page.mouse.dblclick(ruler.x + ruler.width / 2, ruler.y + ruler.height / 2);
  await expect(page.getByRole('button', { name: 'Marker Marker 3, bar 2' })).toBeVisible();
  await page.getByLabel('Marker name').fill('Hook');
  await page.getByLabel('Marker name').press('Enter');
  await expect(page.getByRole('button', { name: 'Marker Hook, bar 2' })).toBeVisible();
  await lit(page, 36);
  await shoot(page, 'marker-added');
});

test('show mode keeps the markers read-only', async ({ page, startEngine }) => {
  const editing = await startEngine({ look: DARK });
  await fetch(`${editing.url}api/songs/arranged/audio`, { method: 'PUT', body: MARKED_WAV.buffer });
  await fetch(`${editing.url}api/songs/arranged/markers`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ markers: [{ name: 'Waltz', at: '4s' }, { name: 'Second verse', at: '7s' }] }),
  });
  await editing.stop();
  const { url } = await startEngine({ look: DARK, edit: false });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByLabel('Waveform')).toBeVisible();
  await lit(page, 36);
  await shoot(page, 'show-mode');
});
