import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect } from './engine.js';

const HEADER = 'program: 1\nbpm: 120\ncues:\n';
const scene = (note, name, values) => `  ${note}:\n    name: ${name}\n    type: scene\n    values:\n      ${values}\n`;
const HOUSE = scene(1, 'house', 'all-wash: { color: warm-white }');
const VERSE = scene(36, 'verse', 'all-wash: { color: deep-blue }');
const CHORUS = scene(37, 'chorus', 'outer-wash: { color: warm-white }');
const RISE = '  39:\n    name: rise\n    type: swell\n    target:\n      all-wash: { dimmer: 100% }\n    duration: 1 bar\n';
const CHASE = '  40:\n    name: chase\n    type: pixel\n    effect: chase\n    params: { color: deep-blue, width: 0.25 }\n';
const SONG = HEADER + HOUSE + VERSE + CHORUS + RISE + CHASE;

function addSong(show, text = SONG) {
  mkdirSync(path.join(show, 'songs'));
  const file = path.join(show, 'songs/demo.yaml');
  writeFileSync(file, text);
  return file;
}

const pad = (page, note) => page.locator(`[data-pad="${note}"]`);
const firstLight = (page, note) => pad(page, note).locator('circle').first();

async function dragPad(page, from, to) {
  const [start, end] = [await pad(page, from).boundingBox(), await pad(page, to).boundingBox()];
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
  await page.mouse.down();
  await page.mouse.move(end.x + end.width / 2, end.y + end.height / 2, { steps: 5 });
  await page.mouse.up();
}

/** Every MIDI message the engine at `address` takes in from now on, as the stream sends them. */
function recordMidi(address) {
  const messages = [];
  const socket = new WebSocket(`ws://${address}/ws`);
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.type === 'midi') messages.push(message.message);
  };
  return new Promise((resolve) => (socket.onopen = () => resolve({ messages, close: () => socket.close() })));
}

test('the pads show the song file after a load, a save and an undo', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await expect(page.getByRole('button', { name: 'Notes 36–51, 4 cues' })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByRole('button', { name: 'Notes 52–67', exact: true })).toBeVisible();
  await expect(page.locator('[data-pad]')).toHaveCount(16);
  for (const [note, name] of [[36, 'verse'], [37, 'chorus'], [38, 'empty'], [39, 'rise'], [40, 'chase']]) {
    await expect(pad(page, note)).toHaveAccessibleName(`Pad ${note}: ${name}`);
  }
  // Thumbnails rendered by the engine at the rig's pixels, or a swell's envelope.
  await expect(firstLight(page, 36)).toHaveAttribute('fill', '#0028ff');
  await expect(pad(page, 37).locator('circle').nth(4), 'wash-2, inside the outer washes').toHaveAttribute('fill', '#000000');
  await expect(firstLight(page, 37)).toHaveAttribute('fill', '#ffd2a0');
  await expect(pad(page, 39).locator('polyline')).toHaveCount(1);
  await expect(pad(page, 40).locator('circle')).toHaveCount(16);

  // Notes below 36 are a bank away.
  await page.getByRole('button', { name: 'Notes 0–3, 1 cue' }).click();
  await expect(page.locator('[data-pad]')).toHaveCount(4);
  await pad(page, 1).click();
  await expect(page.getByText('Cue 1', { exact: true })).toBeVisible();
  await expect(firstLight(page, 1)).toHaveAttribute('fill', '#ffd2a0');

  await page.getByRole('button', { name: 'Notes 36–51, 4 cues' }).click();
  await pad(page, 36).click();
  await page.getByLabel('Name').fill('verse-warm');
  await page.locator('[data-target="all-wash"]').getByRole('button', { name: 'warm-white' }).click();
  await page.getByRole('button', { name: 'Save cue' }).click();
  await expect(pad(page, 36)).toHaveAccessibleName('Pad 36: verse-warm');
  await expect(firstLight(page, 36)).toHaveAttribute('fill', '#ffd2a0');
  expect(readFileSync(songFile, 'utf8')).toContain('    name: verse-warm\n');

  await page.getByRole('button', { name: 'Undo (1)' }).click();
  await expect(pad(page, 36)).toHaveAccessibleName('Pad 36: verse');
  await expect(firstLight(page, 36)).toHaveAttribute('fill', '#0028ff');
  expect(readFileSync(songFile, 'utf8')).toBe(SONG);
});

test('thumbnails the engine refuses are cleared, saying why', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);
  await expect(firstLight(page, 36)).toHaveAttribute('fill', '#0028ff');

  const refusal = 'songs/demo.yaml: cue 40: speed is too long to render a cycle of';
  await page.route('**/api/songs/demo/thumbnails', (route) => route.fulfill({ status: 422, json: { error: refusal } }));
  await pad(page, 36).click();
  await page.getByRole('button', { name: 'Save cue' }).click();

  await expect(page.getByRole('alert')).toHaveText(`Thumbnails: ${refusal}`);
  await expect(pad(page, 36).locator('circle'), 'no stale picture').toHaveCount(0);
});

test('duplicate goes to the next empty pad and a drag moves a cue, never onto a taken note', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);
  const copy = (note) => scene(note, 'verse-copy', 'all-wash: { color: deep-blue }');

  await pad(page, 36).click();
  await page.getByRole('button', { name: 'Duplicate' }).click();
  await expect(page.getByText('Duplicated cue 36 to 38.')).toBeVisible();
  await expect(pad(page, 38)).toHaveAccessibleName('Pad 38: verse-copy');
  expect(readFileSync(songFile, 'utf8')).toBe(HEADER + HOUSE + VERSE + CHORUS + copy(38) + RISE + CHASE);

  await dragPad(page, 38, 41);
  await expect(page.getByText('Moved cue 38 to 41.')).toBeVisible();
  await expect(pad(page, 38)).toHaveAccessibleName('Pad 38: empty');
  await expect(pad(page, 41)).toHaveAccessibleName('Pad 41: verse-copy');
  const moved = HEADER + HOUSE + VERSE + CHORUS + RISE + CHASE + copy(41);
  expect(readFileSync(songFile, 'utf8')).toBe(moved);

  await dragPad(page, 41, 40);
  await expect(page.getByRole('alert')).toHaveText(
    'songs/demo.yaml: note 40 already fires cue "chase"; pick a free note, or edit that cue',
  );
  await expect(pad(page, 41)).toHaveAccessibleName('Pad 41: verse-copy');
  expect(readFileSync(songFile, 'utf8')).toBe(moved);

  // Delete, from the open cue.
  await pad(page, 41).click();
  await page.getByRole('button', { name: 'Delete' }).click();
  await expect(pad(page, 41)).toHaveAccessibleName('Pad 41: empty');
  expect(readFileSync(songFile, 'utf8')).toBe(SONG);
});

test('holding a pad sends note-on and letting go sends note-off; a click only opens it', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url, address } = await startEngine();
  await page.goto(`${url}#builder`);
  await expect(firstLight(page, 37)).toHaveAttribute('fill', '#ffd2a0');
  const midi = await recordMidi(address);

  await pad(page, 37).click();
  await expect(page.getByText('Cue 37', { exact: true })).toBeVisible();

  await pad(page, 36).hover();
  await page.mouse.down();
  await expect.poll(() => midi.messages).toEqual([[0xc1, 1], [0x91, 36, 127]]);
  await expect(pad(page, 36)).toHaveClass(/playing/);
  await expect(page.getByText('Cue 37', { exact: true }), 'a hold is not a click').toBeVisible();

  await page.mouse.up();
  await expect.poll(() => midi.messages).toEqual([[0xc1, 1], [0x91, 36, 127], [0x81, 36, 0]]);
  await expect(pad(page, 36)).not.toHaveClass(/playing/);
  midi.close();
});
