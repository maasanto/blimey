import { readFileSync } from 'node:fs';
import path from 'node:path';
import { withAudio } from './audio.js';
import { test, expect } from './engine.js';

// The C20 fixture: 8 bars, 14 s at 120 BPM.
test.use({ showSource: 'tests/fixtures/shows/arranged' });
const DARK = ['--set', 'both', 'dimmer=0%'];
const NOTE_ON = 0x91;
const NOTE_OFF = 0x81;
/** How far from its time each scene may be sent, as the plan allows. */
const TOLERANCE_MS = 50;

const songFile = (show) => path.join(show, 'songs/arranged.yaml');

/** Every MIDI message the page posts, with when it posted it. */
function recordPosts(page) {
  const posts = [];
  page.on('request', (request) => {
    if (request.url().endsWith('/api/midi')) posts.push({ at: Date.now(), message: request.postDataJSON().message });
  });
  return posts;
}

/** Counts the clicks the page sounds: each is an oscillator started. */
const countClicks = (page) =>
  page.addInitScript(() => {
    window.clicks = 0;
    const start = OscillatorNode.prototype.start;
    OscillatorNode.prototype.start = function (...when) {
      window.clicks += 1;
      return start.apply(this, when);
    };
  });

test('playing from bar 1 with the audio sends the arrangement in order, each within 50 ms, without a click', async ({ page, show, startEngine }) => {
  withAudio(show, 'audio/arranged.wav');
  await countClicks(page);
  const { url } = await startEngine({ look: DARK });
  const { events } = await (await fetch(`${url}api/songs/arranged/arrangement/events`)).json();
  const scenes = events.filter((event) => event.type === 'note_on' || event.type === 'note_off');
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByTestId('audio')).toHaveText('Bar 1 falls 0 s into audio/arranged.wav: drag the waveform to move it against the bars.');
  const posts = recordPosts(page);

  await page.getByRole('button', { name: 'Play' }).click();
  await expect(page.getByRole('button', { name: 'Pause' })).toBeVisible();
  await expect(page.locator('.section-editor .playhead')).toBeVisible();
  await expect.poll(() => posts.length, { timeout: 20_000 }).toBe(scenes.length + 2);
  await expect(page.getByRole('button', { name: 'Play' }), 'stops at the end').toBeVisible();

  expect(posts.slice(0, 2).map(({ message }) => message), 'the song, then where it starts').toEqual([[0xc1, 1], [0xf2, 0, 0]]);
  const sent = posts.slice(2);
  expect(sent.map(({ message }) => message)).toEqual(
    scenes.map((event) => (event.type === 'note_on' ? [NOTE_ON, event.scene, event.velocity] : [NOTE_OFF, event.scene, 0])),
  );
  const start = sent[0].at;
  for (const [index, { at }] of sent.entries()) {
    expect(Math.abs(at - start - scenes[index].seconds * 1000), `${JSON.stringify(scenes[index])} sent at ${at - start} ms`).toBeLessThanOrEqual(TOLERANCE_MS);
  }
  expect(await page.evaluate(() => window.clicks), 'the audio plays instead').toBe(0);
});

test('a missing audio file plays silent with a click; pausing releases the scenes', async ({ page, show, startEngine }) => {
  withAudio(show, 'audio/missing.wav', { isMissing: true });
  await countClicks(page);
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByTestId('audio')).toHaveText('No file at audio/missing.wav: the song plays silent, with a click.');
  const posts = recordPosts(page);

  await page.keyboard.press(' ');
  await expect(page.getByRole('button', { name: 'Pause' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.clicks)).toBeGreaterThanOrEqual(3);
  await page.keyboard.press(' ');
  await expect(page.getByRole('button', { name: 'Play' })).toBeVisible();
  await expect.poll(() => posts.map(({ message }) => message)).toEqual([[0xc1, 1], [0xf2, 0, 0], [NOTE_ON, 36, 127], [NOTE_OFF, 36, 0]]);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('a click on the ruler seeks there, firing the scenes playing at that point', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  const posts = recordPosts(page);

  await page.getByRole('button', { name: 'Play' }).click();
  await expect.poll(() => posts.length).toBe(3);
  const ruler = await page.getByTestId('ruler').boundingBox();
  // Beat 6.5 of the verse: red holds, blue has started.
  await page.mouse.click(ruler.x + (6.5 * ruler.width) / 8, ruler.y + ruler.height / 2);
  await expect.poll(() => posts.slice(3).map(({ message }) => message)).toEqual([[NOTE_OFF, 36, 0], [0xf2, 26, 0], [NOTE_ON, 36, 127], [NOTE_ON, 37, 64]]);
  await expect(page.getByTestId('playhead')).toContainText('Bar 2');
  await page.getByRole('button', { name: 'Back to start' }).click();
  await expect.poll(() => posts.slice(7).map(({ message }) => message)).toEqual([[NOTE_OFF, 36, 0], [NOTE_OFF, 37, 0], [0xf2, 0, 0], [NOTE_ON, 36, 127]]);
  await page.getByRole('button', { name: 'Pause' }).click();
});

test('dragging the waveform sets where bar 1 falls in the audio, saved and undone', async ({ page, show, startEngine }) => {
  const before = withAudio(show, 'audio/arranged.wav');
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  const waveform = page.getByLabel('Waveform');
  await expect(waveform).toBeVisible();

  const box = await waveform.boundingBox();
  const halfSecond = box.width / 28;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 - halfSecond, box.y + box.height / 2, { steps: 4 });
  await page.mouse.up();
  await expect(page.getByTestId('audio')).toContainText(/^Bar 1 falls 0\.(49|5)\d* s into audio\/arranged\.wav/);
  await expect.poll(() => readFileSync(songFile(show), 'utf8')).toMatch(/^audio: \{ file: audio\/arranged\.wav, offset: 0\.(49|5)\d*s \}$/m);

  await page.getByRole('button', { name: /^Undo/ }).click();
  await expect(page.getByTestId('audio')).toContainText('Bar 1 falls 0 s');
  expect(readFileSync(songFile(show), 'utf8')).toBe(before);
});

test('show mode disables playback', async ({ page, show, startEngine }) => {
  withAudio(show, 'audio/arranged.wav');
  const { url } = await startEngine({ look: DARK, edit: false });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByTestId('audio')).toHaveText('Bar 1 falls 0 s into audio/arranged.wav.');
  const posts = recordPosts(page);

  await expect(page.getByRole('button', { name: 'Play' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Back to start' })).toBeDisabled();
  await page.keyboard.press(' ');
  await page.getByTestId('ruler').click();
  await page.waitForTimeout(500);
  expect(posts).toEqual([]);
});
