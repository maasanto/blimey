import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { markedWav, steadyAudioClock, withAudio } from './audio.js';
import { test, expect } from './engine.js';

// The C20 fixture at 120 BPM: verse bars 1-2, waltz (3/4) bars 3-4, verse bars 5-6, waltz bars 7-8.
test.use({ showSource: 'tests/fixtures/shows/arranged' });
const DARK = ['--set', 'both', 'dimmer=0%'];
/** Bar 3 starts 4 s in, after the first verse's 8 beats; bar 5 at 7 s, after the waltz's 6. */
const REAPER_MARKERS = [
  { name: 'Waltz', at: 4 },
  { name: 'Second verse', at: 7 },
];
const MARKED_WAV = { name: 'arranged.wav', mimeType: 'audio/wav', buffer: markedWav(16, REAPER_MARKERS) };

const songFile = (show) => path.join(show, 'songs/arranged.yaml');
const song = (show) => readFileSync(songFile(show), 'utf8');
const overviewFlag = (page, placement, name) => page.locator('.overview .placement').nth(placement).locator(`[data-marker="${name}"]`);

test('an uploaded WAV brings its markers, drawn on their bars, and plays', async ({ page, show, startEngine }) => {
  // Played on a clock of the test's own, not the host's audio output, which may never start.
  await steadyAudioClock(page);
  await page.addInitScript(() => {
    window.clicks = 0;
    const start = OscillatorNode.prototype.start;
    OscillatorNode.prototype.start = function (...when) {
      window.clicks += 1;
      return start.apply(this, when);
    };
  });
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  const before = song(show);

  await page.getByLabel('Audio file').setInputFiles({ name: 'notes.txt', mimeType: 'audio/wav', buffer: Buffer.from('not audio at all') });
  await expect(page.getByRole('alert')).toContainText('the file is not WAV, MP3 or FLAC audio');
  expect(song(show)).toBe(before);

  await page.getByLabel('Audio file').setInputFiles(MARKED_WAV);
  await expect(page.getByTestId('audio')).toHaveText('Bar 1 falls 0 s into audio/arranged.wav: drag the waveform to move it against the bars.');
  await expect.poll(() => song(show)).toContain('\nmarkers:\n  - { name: Waltz, at: 4s }\n  - { name: Second verse, at: 7s }\n');
  expect(song(show)).toContain('\naudio: { file: audio/arranged.wav }\n');
  expect(readFileSync(path.join(show, 'audio/arranged.wav'))).toEqual(MARKED_WAV.buffer);

  await expect(overviewFlag(page, 1, 'Waltz')).toHaveAttribute('title', 'Waltz · bar 3');
  await expect(overviewFlag(page, 2, 'Second verse')).toHaveAttribute('title', 'Second verse · bar 5');
  const placement = await page.locator('.overview .placement').nth(1).boundingBox();
  const flag = await overviewFlag(page, 1, 'Waltz').boundingBox();
  expect(Math.abs(flag.x + flag.width / 2 - placement.x), 'at the start of bar 3').toBeLessThan(3);
  await page.getByRole('button', { name: 'waltz from bar 3' }).click();
  await expect(page.getByRole('button', { name: 'Marker Waltz, bar 3' })).toBeVisible();

  await page.getByRole('button', { name: 'Play' }).click();
  await expect(page.getByTestId('playhead')).toContainText('Bar 2');
  await page.getByRole('button', { name: 'Pause' }).click();
  expect(await page.evaluate(() => window.clicks), 'the uploaded audio plays, not the click').toBe(0);
});

test('markers are added, renamed, moved and deleted, each saved and undone', async ({ page, show, startEngine }) => {
  const before = withAudio(show, 'audio/arranged.wav');
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByLabel('Waveform')).toBeVisible();

  await page.keyboard.press('m');
  await expect.poll(() => song(show)).toContain('\nmarkers:\n  - { name: Marker 1, at: 0s }\n');

  const ruler = await page.getByTestId('ruler').boundingBox();
  const beat = ruler.width / 8;
  await page.mouse.dblclick(ruler.x + 4 * beat + 1, ruler.y + ruler.height / 2);
  await expect.poll(() => song(show)).toMatch(/\n {2}- \{ name: Marker 2, at: 2(\.0\d*)?s \}\n/);

  await page.getByRole('button', { name: 'Marker Marker 1, bar 1' }).click();
  await page.getByLabel('Marker name').fill('Intro');
  await page.getByLabel('Marker name').press('Enter');
  await expect.poll(() => song(show)).toContain('\n  - { name: Intro, at: 0s }\n');

  const intro = await page.getByRole('button', { name: 'Marker Intro, bar 1' }).boundingBox();
  await page.mouse.move(intro.x + 4, intro.y + intro.height / 2);
  await page.mouse.down();
  await page.mouse.move(intro.x + 4 + 3 * beat, intro.y + intro.height / 2, { steps: 6 });
  await page.mouse.up();
  await expect.poll(() => song(show)).toMatch(/\nmarkers:\n {2}- \{ name: Intro, at: 1\.(49|5)\d*s \}\n {2}- \{ name: Marker 2, at: 2(\.0\d*)?s \}\n/);
  await expect(page.getByRole('button', { name: 'Marker Intro, bar 1' })).toHaveClass(/selected/);

  await page.getByRole('button', { name: 'Marker Marker 2, bar 2' }).click();
  await page.getByRole('button', { name: 'Delete marker' }).click();
  await expect.poll(() => song(show)).not.toContain('Marker 2');
  await page.getByRole('button', { name: 'Marker Intro, bar 1' }).focus();
  await page.keyboard.press('Delete');
  await expect.poll(() => song(show)).toBe(before);

  await page.getByRole('button', { name: /^Undo/ }).click();
  await expect.poll(() => song(show)).toContain('{ name: Intro');
});

test('a new marker takes a number no other marker uses', async ({ page, show, startEngine }) => {
  writeFileSync(songFile(show), song(show).replace('bpm: 120\n', 'bpm: 120\nmarkers:\n  - { name: Marker 2, at: 1s }\n'));
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByRole('button', { name: 'Marker Marker 2, bar 1' })).toBeVisible();

  await page.keyboard.press('m');
  await expect.poll(() => song(show)).toContain('\nmarkers:\n  - { name: Marker 3, at: 0s }\n  - { name: Marker 2, at: 1s }\n');
});

test('replacing the song\'s markers with the audio\'s asks first', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  writeFileSync(songFile(show), song(show).replace('bpm: 120\n', 'bpm: 120\nmarkers:\n  - { name: Mine, at: 1s }\n'));
  await page.goto(`${url}#builder/arranged/timeline`);
  const dialog = page.getByRole('dialog');

  await page.getByLabel('Audio file').setInputFiles(MARKED_WAV);
  await expect(dialog).toHaveText(/This song has 1 marker\. Replace them with the 2 markers in the audio file\?/);
  await dialog.getByRole('button', { name: "Keep the song's" }).click();
  await expect(dialog).toBeHidden();
  await expect.poll(() => song(show)).toContain('\naudio: { file: audio/arranged.wav }\n');
  expect(song(show)).toContain('\nmarkers:\n  - { name: Mine, at: 1s }\n');

  await page.getByRole('button', { name: 'Import markers from audio' }).click();
  await dialog.getByRole('button', { name: 'Replace markers' }).click();
  await expect.poll(() => song(show)).toContain('\nmarkers:\n  - { name: Waltz, at: 4s }\n  - { name: Second verse, at: 7s }\n');
});

test('show mode is read-only', async ({ page, show, startEngine }) => {
  withAudio(show, 'audio/arranged.wav');
  writeFileSync(songFile(show), song(show).replace('bpm: 120\n', 'bpm: 120\nmarkers:\n  - { name: Mine, at: 1s }\n'));
  const before = song(show);
  const { url } = await startEngine({ look: DARK, edit: false });
  await page.goto(`${url}#builder/arranged/timeline`);
  const marker = page.getByRole('button', { name: 'Marker Mine, bar 1' });
  await expect(marker).toBeVisible();

  await expect(page.getByRole('button', { name: 'Upload audio…' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Import markers from audio' })).toBeDisabled();
  await page.keyboard.press('m');
  await page.getByTestId('ruler').dblclick();
  const box = await marker.boundingBox();
  await page.mouse.move(box.x + 4, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 100, box.y + box.height / 2, { steps: 4 });
  await page.mouse.up();
  await marker.click();
  await expect(page.getByLabel('Marker name')).toBeDisabled();
  const upload = await fetch(`${url}api/songs/arranged/audio`, { method: 'PUT', body: MARKED_WAV.buffer });
  expect(upload.status).toBe(403);
  await page.waitForTimeout(300);
  expect(song(show)).toBe(before);
});
