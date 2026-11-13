import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, nextDmx, BINARY } from './engine.js';

const HEADER = 'program: 1\nbpm: 120\ncues:\n';
const SELECT_SONG = [0xc1, 1];
const FIRE_40 = [0x91, 40, 127];

function addSong(show, text) {
  mkdirSync(path.join(show, 'songs'));
  const file = path.join(show, 'songs/demo.yaml');
  writeFileSync(file, text);
  return file;
}

/** A Standard MIDI File (format 0) with `messages` all at time 0. */
function midiFile(messages) {
  const track = [...messages.flatMap((message) => [0, ...message]), 0, 0xff, 0x2f, 0];
  const length = [24, 16, 8, 0].map((shift) => (track.length >>> shift) & 0xff);
  return Buffer.from([
    ...Buffer.from('MThd'), 0, 0, 0, 6, 0, 0, 0, 1, 0, 96,
    ...Buffer.from('MTrk'), ...length, ...track,
  ]);
}

/** The first frame `blimey replay` renders for `messages` on `show`: the engine's own render of the saved files. */
function replayFirstFrame(show, messages) {
  const midi = path.join(show, 'fire.mid');
  writeFileSync(midi, midiFile(messages));
  const dump = execFileSync(BINARY, ['replay', show, '--midi', midi], { encoding: 'utf8' });
  const frame = new Array(512).fill(0);
  const [, ...channels] = dump.split('\n').find((line) => /^\d/.test(line)).split(' ');
  for (const channel of channels) {
    const [number, value] = channel.split('=').map(Number);
    frame[number - 1] = value;
  }
  return frame;
}

/** Whether the picker's thumbnail canvas has a lit pixel. */
const isPainted = (button) =>
  button.locator('canvas').evaluate((canvas) => {
    const { data } = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height);
    return data.some((value, index) => index % 4 !== 3 && value > 0);
  });

test('a two-layer pixel scene plays after a restart as the engine renders its saved file', async ({ page, show, startEngine }) => {
  const songFile = addSong(show, HEADER.replace('cues:\n', ''));
  const first = await startEngine();
  await page.goto(`${first.url}#builder`);

  // 1. A new pixel cue: gradient in deep blue, plus a red bar added on layer 2.
  await page.getByLabel('Type').selectOption('pixel');
  await page.getByLabel('Note').fill('40');
  await page.getByLabel('Name').fill('two-layer');
  const gradient = page.getByRole('button', { name: 'Layer 1 shape: gradient' });
  await expect.poll(() => isPainted(gradient), 'thumbnails rendered by the engine').toBe(true);
  await gradient.click();
  await page.getByRole('button', { name: 'Layer 1 colour: deep-blue' }).click();
  await page.getByLabel('Layer 2', { exact: true }).check();
  await page.getByRole('button', { name: 'Layer 2 shape: bar' }).click();
  await page.getByLabel('Layer 2 colour', { exact: true }).fill('#ff0000');
  await page.getByLabel('Layer 2 width').fill('0.5');
  await page.getByLabel('Blend').selectOption('add');

  // 2. Save it.
  await page.getByRole('button', { name: 'Save cue' }).click();
  await expect(page.getByText('Saved cue 40 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    HEADER +
      '  40:\n    name: two-layer\n    type: pixel\n    shape: gradient\n    effect: static\n' +
      '    params: { color: deep-blue, width: 0.25 }\n' +
      '    layer_2:\n      shape: bar\n      effect: static\n      params: { color: "#ff0000", width: 0.5 }\n      blend: add\n' +
      '    seed: 7\n',
  );
  const rendered = replayFirstFrame(show, [SELECT_SONG, FIRE_40]);
  expect(rendered.slice(0, 64).some((value) => value > 0), 'the washes are lit').toBe(true);

  // The live preview while editing was that render, on the output and the front view.
  await expect.poll(() => nextDmx(first.address)).toEqual(rendered);
  await expect(page.locator('[data-fixture="wash-1"] circle').first()).not.toHaveAttribute('fill', '#ff0000');

  // 3. Restart the engine on the same show: the preview is gone.
  await first.stop();
  const second = await startEngine();
  expect((await nextDmx(second.address)).slice(0, 4), "fire's own look on wash-1").toEqual([255, 0, 0, 0]);

  // 4. Fire note 40 as Reaper would: the output is the engine's render of the saved file.
  for (const message of [SELECT_SONG, FIRE_40]) {
    expect((await page.request.post(`${second.url}api/midi`, { data: { message } })).status()).toBe(204);
  }
  await expect.poll(() => nextDmx(second.address)).toEqual(rendered);
});

test("linking a pattern to a pixel scene's intensity saves it as the pattern's target", async ({ page, show, startEngine }) => {
  const pattern =
    '  42:\n    name: pulse\n    type: pattern\n    length: 1 bar\n    target: { all-wash: dimmer }\n' +
    '    hits:\n      - { at: 1, shape: ramp-up, length: 1 beat, level: 100% }\n';
  const songFile = addSong(show, `${HEADER}  40:\n    name: red-wash\n    type: pixel\n    params: { color: "#ff0000" }\n${pattern}`);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await page.locator('tr[data-note="40"]').getByRole('button', { name: 'Edit' }).click();
  await expect(page.getByText('velocity only')).toBeVisible();
  // A rename not saved yet: links go by the name in the file.
  await page.getByLabel('Name').fill('red-wash-2');
  await page.getByLabel('Swell or pattern to link').selectOption('42');
  await page.getByRole('button', { name: 'Link to intensity' }).click();

  await expect(page.getByText('driven by cue 42, pulse (pattern)')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    `${HEADER}  40:\n    name: red-wash\n    type: pixel\n    params: { color: "#ff0000" }\n` +
      pattern.replace('{ all-wash: dimmer }', '{ red-wash: intensity }'),
  );
  await page.getByRole('button', { name: 'Undo (1)' }).click();
  await expect(page.getByText('velocity only')).toBeHidden();
});

test('a thumbnail the engine refuses says why', async ({ page, show, startEngine }) => {
  addSong(show, HEADER.replace('cues:\n', ''));
  const { url } = await startEngine();
  await page.route('**/api/pixel-thumbnail', (route) =>
    route.fulfill({ status: 422, contentType: 'application/json', body: JSON.stringify({ error: 'speed is too long to render a cycle of' }) }),
  );
  await page.goto(`${url}#builder`);

  await page.getByLabel('Type').selectOption('pixel');

  await expect(page.getByRole('alert')).toHaveText('Thumbnails: speed is too long to render a cycle of');
});

test('pixel grids show on the front view with the colours the engine sends them', async ({ page, show, startEngine }) => {
  const rig = path.join(show, 'rig.yaml');
  const grid = '  - { name: back-grid, type: pixel-grid, columns: 4, rows: 2, colour_order: RGB, output: artnet, universe: 1, address: 1, canvas: { x: 0.3, y: 0.6, width: 0.4, height: 0.2 } }\n';
  writeFileSync(rig, readFileSync(rig, 'utf8').replace('\ngroups:', `${grid}\ngroups:`));
  addSong(show, `${HEADER}  40: { name: green, type: pixel, params: { color: "#00ff00" } }\n`);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);
  const tiles = page.locator('[data-grid="back-grid"] rect');
  await expect(tiles).toHaveCount(8);
  await expect(tiles.first()).toHaveAttribute('fill', '#000000');

  for (const message of [SELECT_SONG, FIRE_40]) {
    await page.request.post(`${url}api/midi`, { data: { message } });
  }

  await expect(tiles.first()).toHaveAttribute('fill', '#00ff00');
  await expect(tiles.last()).toHaveAttribute('fill', '#00ff00');
});
