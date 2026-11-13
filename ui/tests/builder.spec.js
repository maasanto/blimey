import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, nextDmx } from './engine.js';

const HEADER = '# Demo song for the builder tests.\nprogram: 1\nbpm: 120\ncues:\n';
const BLACKOUT = '  38:\n    name: blackout\n    type: scene\n    values:\n      all-wash: { dimmer: 0% }\n';

/** Writes `songs/demo.yaml` into the test's show; returns its path. */
function addSong(show, text = HEADER + BLACKOUT) {
  mkdirSync(path.join(show, 'songs'));
  const file = path.join(show, 'songs/demo.yaml');
  writeFileSync(file, text);
  return file;
}

async function addToLook(page, target) {
  await page.getByLabel('Fixture or group to add').selectOption(target);
  await page.getByRole('button', { name: 'Add', exact: true }).click();
}

const pixel = (page, fixture) => page.locator(`[data-fixture="${fixture}"] circle`).first();

test('a look built in the builder plays the same from note 36 after a restart', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const first = await startEngine();
  await page.goto(`${first.url}#builder`);

  // 1. Build the look: all washes deep blue at 50%, then wash-1 green at full.
  await addToLook(page, 'all-wash');
  const allWash = page.locator('[data-target="all-wash"]');
  await allWash.getByRole('button', { name: 'deep-blue' }).click();
  await allWash.getByLabel('all-wash dimmer').fill('50');
  await addToLook(page, 'wash-1');
  await page.getByLabel('wash-1 colour').fill('#00ff00');

  // Live through the engine: the front view on the page follows the output.
  await expect(pixel(page, 'wash-1')).toHaveAttribute('fill', '#00ff00');
  await expect(pixel(page, 'wash-2')).toHaveAttribute('fill', '#001480');
  const built = await nextDmx(first.address);
  expect(built.slice(0, 4), 'wash-1 pixel 1: red, green, blue, white').toEqual([0, 255, 0, 0]);
  expect(built.slice(16, 20), 'wash-2 pixel 1').toEqual([0, 20, 128, 0]);

  // 2. Save it as cue 36.
  await page.getByLabel('Note').fill('36');
  await page.getByLabel('Name').fill('verse-green');
  await page.getByRole('button', { name: 'Save cue' }).click();
  await expect(page.getByText('Saved cue 36 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    HEADER +
      '  36:\n    name: verse-green\n    type: scene\n    values:\n' +
      '      all-wash: { color: deep-blue, dimmer: 50% }\n      wash-1: { color: "#00ff00", dimmer: 100% }\n' +
      BLACKOUT,
  );

  // Audition fires and releases it through the engine, the cue showing over a look edited since.
  await page.getByLabel('wash-1 colour').fill('#0000ff');
  await expect(pixel(page, 'wash-1')).toHaveAttribute('fill', '#0000ff');
  const row = page.locator('tr[data-note="36"]');
  await row.getByRole('button', { name: 'Audition' }).click();
  await expect(pixel(page, 'wash-1'), 'the cue, not the look being edited').toHaveAttribute('fill', '#00ff00');
  await expect(page.getByText('Off the output while a cue is auditioned.')).toBeVisible();
  await row.getByRole('button', { name: 'Release' }).click();
  await expect(row.getByRole('button', { name: 'Audition' })).toBeVisible();

  // 3. Restart the engine on the same show: the preview is gone.
  await first.stop();
  const second = await startEngine();
  expect((await nextDmx(second.address)).slice(0, 4), "fire's own look on wash-1").toEqual([255, 0, 0, 0]);

  // 4. Fire note 36 as Reaper would: song select, then the note.
  for (const message of [[0xc1, 1], [0x91, 36, 127]]) {
    expect((await page.request.post(`${second.url}api/midi`, { data: { message } })).status()).toBe(204);
  }

  // 5. The output is the look built in step 1.
  await expect.poll(() => nextDmx(second.address)).toEqual(built);
});

test('a cue on a note already in use is refused', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const before = readFileSync(songFile, 'utf8');
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await page.getByLabel('Note').fill('38');
  await page.getByLabel('Name').fill('another');
  await page.getByRole('button', { name: 'Save cue' }).click();

  await expect(page.getByRole('alert')).toHaveText(
    'songs/demo.yaml: note 38 already fires cue "blackout"; pick a free note, or edit that cue',
  );
  expect(readFileSync(songFile, 'utf8')).toBe(before);
});

test('editing a pixel cue in the form keeps the fields the form does not show', async ({ page, show, startEngine }) => {
  const pixelCue = (head) =>
    `  40:\n${head}    effect: chase\n` +
    '    params: { color: deep-blue, width: 0.2, speed: 1 bar, direction: left-to-right }\n' +
    '    layer_2: { shape: sweep, blend: add }\n    seed: 7\n';
  const songFile = addSong(show, HEADER + BLACKOUT + pixelCue('    name: bridge-chase\n    type: pixel\n'));
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await page.locator('tr[data-note="40"]').getByRole('button', { name: 'Edit' }).click();
  await page.getByLabel('Name').fill('bridge-sweep');
  await page.getByLabel('Fade in').fill('1 beat');
  await page.getByLabel('Release').fill('2s');
  await page.getByRole('button', { name: 'Save cue' }).click();

  await expect(page.getByText('Saved cue 40 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    HEADER + BLACKOUT + pixelCue('    name: bridge-sweep\n    type: pixel\n    fade_in: 1 beat\n    release: 2s\n'),
  );
});

test('the look preview draws above a playing pixel cue', async ({ page, show, startEngine }) => {
  addSong(show, `${HEADER}  40:\n    name: all-blue\n    type: pixel\n    params: { color: deep-blue }\n`);
  const { url, address } = await startEngine();
  for (const message of [[0xc1, 1], [0x91, 40, 127]]) {
    expect((await page.request.post(`${url}api/midi`, { data: { message } })).status()).toBe(204);
  }
  await expect.poll(async () => (await nextDmx(address)).slice(16, 20), 'wash-2 lit by the pixel cue').not.toEqual([0, 0, 0, 0]);
  const pixelCue = (await nextDmx(address)).slice(16, 20);

  await page.request.put(`${url}api/preview`, { data: { 'wash-1': { color: '#00ff00' } } });

  await expect.poll(async () => (await nextDmx(address)).slice(0, 4), 'wash-1 shows the preview').toEqual([0, 255, 0, 0]);
  expect((await nextDmx(address)).slice(16, 20), 'wash-2 keeps the pixel cue').toEqual(pixelCue);
});

test('the metronome tempo is saved to the song', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await page.getByLabel('Metronome').fill('92');
  await page.getByRole('button', { name: 'Set tempo' }).click();

  await expect(page.getByText('demo runs at 92 BPM when no MIDI clock comes in.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(HEADER.replace('bpm: 120', 'bpm: 92') + BLACKOUT);
});

test('undo restores the previous file byte for byte', async ({ page, show, startEngine }) => {
  const handWritten = '# By hand.\nprogram: 1\n\ncues:\n  38: { name: blackout, type: scene, values: { all-wash: { dimmer: 0% } } }  # keep\n';
  const songFile = addSong(show, handWritten);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await page.locator('tr[data-note="38"]').getByRole('button', { name: 'Edit' }).click();
  await page.getByLabel('Note').fill('40');
  await page.getByRole('button', { name: 'Save cue' }).click();
  await expect(page.locator('tr[data-note="40"]')).toBeVisible();
  await expect(page.locator('tr[data-note="38"]'), 'renumbered, not copied').toHaveCount(0);
  const saved = readFileSync(songFile, 'utf8');
  expect(saved).not.toBe(handWritten);

  // A hand edit since the save blocks the undo, which can be tried again once it is reverted.
  writeFileSync(songFile, `${saved}# a note by hand\n`);
  await page.getByRole('button', { name: 'Undo last save' }).click();
  await expect(page.getByRole('alert')).toContainText('songs/demo.yaml: changed since the save');
  writeFileSync(songFile, saved);
  await page.getByRole('button', { name: 'Undo last save' }).click();

  await expect(page.getByText('Undid the last save of songs/demo.yaml.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(handWritten);
  await expect(page.locator('tr[data-note="38"]')).toBeVisible();
});

test('malformed request bodies are refused with 422', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url } = await startEngine();

  for (const [path, method] of [
    ['api/songs/demo/cues/40', 'put'],
    ['api/songs/demo/bpm', 'put'],
    ['api/midi', 'post'],
    ['api/preview', 'put'],
  ]) {
    for (const data of [[1, 2], 7, { cue: 'x', replaces: 'y', bpm: 'fast', message: 'note' }]) {
      const response = await page.request[method](`${url}${path}`, { data });
      expect(response.status(), `${method} ${path} ${JSON.stringify(data)}`).toBe(422);
      expect((await response.json()).error).toBeTruthy();
    }
  }
});

test('show mode locks the builder', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const before = readFileSync(songFile, 'utf8');
  const { url } = await startEngine({ edit: false });
  await page.goto(`${url}#builder`);

  await expect(page.getByText('Show mode: the builder is read-only.')).toBeVisible();
  for (const control of [
    page.getByLabel('Fixture or group to add'),
    page.getByLabel('Note'),
    page.getByRole('button', { name: 'Set tempo' }),
    page.getByRole('button', { name: 'Undo last save' }),
    page.locator('tr[data-note="38"]').getByRole('button', { name: 'Audition' }),
    page.locator('tr[data-note="38"]').getByRole('button', { name: 'Delete' }),
  ]) {
    await expect(control).toBeDisabled();
  }

  const refusals = [
    page.request.put(`${url}api/songs/demo/cues/40`, { data: { cue: { name: 'x', type: 'scene', values: {} } } }),
    page.request.delete(`${url}api/songs/demo/cues/38`),
    page.request.put(`${url}api/songs/demo/bpm`, { data: { bpm: 90 } }),
    page.request.put(`${url}api/preview`, { data: { 'all-wash': { color: '#ffffff' } } }),
    page.request.post(`${url}api/midi`, { data: { message: [0x91, 38, 127] } }),
    page.request.post(`${url}api/undo`),
  ];
  for (const response of await Promise.all(refusals)) expect(response.status(), response.url()).toBe(403);
  expect(readFileSync(songFile, 'utf8')).toBe(before);
});
