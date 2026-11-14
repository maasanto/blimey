import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
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

const ingredient = (page, name) => page.getByRole('group', { name: 'Ingredients' }).getByRole('button', { name });
const pixel = (page, fixture) => page.locator(`[data-fixture="${fixture}"] circle`).first();
const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
const backToScenes = (page) => page.getByRole('link', { name: '← Scenes' });

test('a look built in the builder plays the same from scene 36 after a restart', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const first = await startEngine();
  // 1. Build the look as a new scene 36: all washes deep blue at 50%, then wash-1 green at full.
  await page.goto(`${first.url}#builder/demo/36`);
  await expect(page.getByText('New scene', { exact: true })).toBeVisible();
  await ingredient(page, 'Wash').click();
  await page.getByRole('button', { name: 'Colour: deep-blue' }).click();
  await page.getByLabel('Level', { exact: true }).fill('50');
  await ingredient(page, 'Wash').click();
  await page.getByLabel('Colour', { exact: true }).fill('#00ff00');
  for (const fixture of ['wash-2', 'wash-3', 'wash-4']) await page.getByRole('button', { name: `Mute ${fixture}` }).click();

  // Live through the engine: the front view on the page follows the output.
  await expect(pixel(page, 'wash-1')).toHaveAttribute('fill', '#00ff00');
  await expect(pixel(page, 'wash-2')).toHaveAttribute('fill', '#001480');
  await expect(pixel(page, 'wash-4'), 'the last mute on the output').toHaveAttribute('fill', '#001480');
  const built = await nextDmx(first.address);
  expect(built.slice(0, 4), 'wash-1 pixel 1: red, green, blue, white').toEqual([0, 255, 0, 0]);

  // 2. Save it as scene 36.
  await page.getByLabel('Name').fill('verse-green');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Saved scene 36 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    HEADER +
      '  36:\n    name: verse-green\n    type: pixel\n    layers:\n' +
      '      -\n        shape: fill\n        params: { color: deep-blue }\n        level: 50%\n        blend: over\n' +
      '      -\n        shape: fill\n        params: { color: "#00ff00" }\n        blend: over\n        mute: [wash-2, wash-3, wash-4]\n' +
      BLACKOUT,
  );

  // An audition fires it through the engine, the saved scene showing over a look edited since.
  await page.getByLabel('Colour', { exact: true }).fill('#0000ff');
  await expect(pixel(page, 'wash-1')).toHaveAttribute('fill', '#0000ff');
  await page.getByRole('button', { name: 'Audition', exact: true }).click();
  await expect(pixel(page, 'wash-1'), 'the saved scene, not the look being edited').toHaveAttribute('fill', '#00ff00');
  await expect(page.getByText('Off the output while a scene is auditioned.')).toBeVisible();
  await page.getByRole('button', { name: 'Release', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Audition', exact: true })).toBeVisible();

  // 3. Restart the engine on the same show: the preview is gone.
  await first.stop();
  const second = await startEngine();
  expect((await nextDmx(second.address)).slice(0, 4), "fire's own look on wash-1").toEqual([255, 0, 0, 0]);

  // 4. Fire scene 36 as Reaper would until it has its own control: song 1's program, then note 36.
  for (const message of [[0xc1, 1], [0x91, 36, 127]]) {
    expect((await page.request.post(`${second.url}api/midi`, { data: { message } })).status()).toBe(204);
  }

  // 5. The output is the look built in step 1.
  await expect.poll(() => nextDmx(second.address)).toEqual(built);
});

test('editing a look in the scene editor keeps its layers, in the layers form', async ({ page, show, startEngine }) => {
  const songFile = addSong(
    show,
    HEADER + BLACKOUT +
      '  40:\n    name: bridge-chase\n    type: pixel\n    effect: chase\n' +
      '    params: { color: deep-blue, width: 0.2, speed: 1 bar, direction: left-to-right }\n' +
      '    layer_2: { shape: sweep, blend: add }\n    seed: 7\n',
  );
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await page.getByLabel('Name').fill('bridge-sweep');
  await page.getByLabel('Fade in').fill('1 beat');
  await page.getByLabel('Release').fill('2s');
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText('Saved scene 40 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    HEADER + BLACKOUT +
      '  40:\n    name: bridge-sweep\n    type: pixel\n    fade_in: 1 beat\n    release: 2s\n    seed: 7\n    layers:\n' +
      '      -\n        effect: chase\n        params: { color: deep-blue, width: 0.2, speed: 1 bar, direction: left-to-right }\n' +
      '      - { shape: sweep, blend: add }\n',
  );
});

test('the pixel scene preview draws above a playing pixel cue', async ({ page, show, startEngine }) => {
  addSong(show, `${HEADER}  40:\n    name: all-blue\n    type: pixel\n    params: { color: deep-blue }\n`);
  const { url, address } = await startEngine();
  for (const message of [[0xc1, 1], [0x91, 40, 127]]) {
    expect((await page.request.post(`${url}api/midi`, { data: { message } })).status()).toBe(204);
  }
  await expect.poll(async () => (await nextDmx(address)).slice(16, 20), 'wash-2 lit by the pixel cue').not.toEqual([0, 0, 0, 0]);
  const pixelCue = (await nextDmx(address)).slice(16, 20);

  const onWash1 = { shape: 'fill', params: { color: '#00ff00' }, mute: ['wash-2', 'wash-3', 'wash-4'] };
  await page.request.put(`${url}api/preview/pixel`, { data: { song: 'demo', cue: { name: 'green', type: 'pixel', layers: [onWash1] } } });

  await expect.poll(async () => (await nextDmx(address)).slice(0, 4), 'wash-1 shows the preview').toEqual([0, 255, 0, 0]);
  expect((await nextDmx(address)).slice(16, 20), 'wash-2 keeps the pixel cue').toEqual(pixelCue);
});

test('the metronome tempo is saved to the song', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await page.getByLabel('Metronome').fill('92');
  await page.getByRole('button', { name: 'Set tempo' }).click();

  await expect(page.getByText('demo runs at 92 BPM when no clock comes in.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(HEADER.replace('bpm: 120', 'bpm: 92') + BLACKOUT);
});

test('undo restores the previous file byte for byte', async ({ page, show, startEngine }) => {
  const handWritten = '# By hand.\nprogram: 1\n\ncues:\n  38: { name: blackout, type: scene, values: { all-wash: { dimmer: 0% } } }  # keep\n';
  const songFile = addSong(show, handWritten);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  // The fixed look stays one, and a new number moves it.
  await pad(page, 38).click();
  await page.getByLabel('Number').fill('40');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Saved scene 40 in demo.')).toBeVisible();
  await backToScenes(page).click();
  await expect(pad(page, 40)).toHaveAccessibleName('Scene 40: blackout');
  await expect(pad(page, 38), 'moved, not copied').toHaveCount(0);
  const saved = readFileSync(songFile, 'utf8');
  expect(saved).toBe(`# By hand.\nprogram: 1\ncues:\n${BLACKOUT.replace('38:', '40:')}`);
  expect(saved).not.toBe(handWritten);

  // A hand edit since the save blocks the undo, which can be tried again once it is reverted.
  writeFileSync(songFile, `${saved}# a note by hand\n`);
  await page.getByRole('button', { name: 'Undo (1)' }).click();
  await expect(page.getByRole('alert')).toContainText('songs/demo.yaml: changed since the save');
  writeFileSync(songFile, saved);
  await page.getByRole('button', { name: 'Undo (1)' }).click();

  await expect(page.getByText('Undid the last save of songs/demo.yaml.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(handWritten);
  await expect(pad(page, 38)).toHaveAccessibleName('Scene 38: blackout');
  await expect(page.getByRole('button', { name: 'Undo (0)' })).toBeDisabled();
});

test('a new song takes the next free number and a scene, and two undos remove the scene then the song', async ({ page, show, startEngine }) => {
  const demoFile = addSong(show);
  const demo = readFileSync(demoFile, 'utf8');
  const encoreFile = path.join(show, 'songs/encore.yaml');
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);
  const songPicker = page.locator('.bar select');
  const number = page.getByLabel('Number', { exact: true });

  await page.getByRole('button', { name: 'New song' }).click();
  await expect(number, 'song 1 is demo').toHaveValue('2');
  // Taken names and numbers are refused by the engine.
  await page.getByLabel('Song name').fill('demo');
  await page.getByRole('button', { name: 'Create song' }).click();
  await expect(page.getByRole('alert')).toHaveText('songs/demo.yaml: the show already has song "demo"; pick another name');
  await page.getByLabel('Song name').fill('encore');
  await number.fill('1');
  await page.getByRole('button', { name: 'Create song' }).click();
  await expect(page.getByRole('alert')).toHaveText('Song 1 is already demo: pick a free number.');
  expect(existsSync(encoreFile)).toBe(false);

  await number.fill('2');
  await page.getByLabel('Tempo').fill('100');
  await page.getByRole('button', { name: 'Create song' }).click();
  await expect(page.getByText('Created song 2 · encore.')).toBeVisible();
  await expect(songPicker).toHaveValue('encore');
  expect(readFileSync(encoreFile, 'utf8')).toBe('program: 2\nbpm: 100\n');

  await page.getByRole('button', { name: 'New scene' }).click();
  await ingredient(page, 'Wash').click();
  await page.getByLabel('Name').fill('first-look');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Saved scene 1 in encore.')).toBeVisible();
  await backToScenes(page).click();
  await expect(pad(page, 1)).toHaveAccessibleName('Scene 1: first-look');
  const withScene = readFileSync(encoreFile, 'utf8');
  expect(withScene).toContain('  1:\n    name: first-look\n');

  await page.getByRole('button', { name: 'Undo (2)' }).click();
  await expect(page.locator('[data-pad]')).toHaveCount(0);
  expect(readFileSync(encoreFile, 'utf8')).toBe('program: 2\nbpm: 100\n');
  await page.getByRole('button', { name: 'Undo (1)' }).click();
  await expect(page.getByText('Undid the last save of songs/encore.yaml.')).toBeVisible();
  expect(existsSync(encoreFile), 'the song is gone').toBe(false);
  await expect(songPicker.locator('option')).toHaveText(['Song 1 · demo']);
  expect(readFileSync(demoFile, 'utf8')).toBe(demo);

  // Redo brings both back, as they were.
  await page.getByRole('button', { name: 'Redo (2)' }).click();
  await page.getByRole('button', { name: 'Redo (1)' }).click();
  await expect(page.getByRole('button', { name: 'Redo (0)' })).toBeDisabled();
  expect(readFileSync(encoreFile, 'utf8')).toBe(withScene);
});

test('a fixture dragged on the front view is a step to undo in the builder', async ({ page, startEngine }) => {
  const { url } = await startEngine();
  await page.goto(`${url}#front`);
  const pixel = await page.locator('[data-fixture="wash-2"] circle').first().boundingBox();

  await page.mouse.move(pixel.x + pixel.width / 2, pixel.y + pixel.height / 2);
  await page.mouse.down();
  await page.mouse.move(pixel.x + 60, pixel.y + 40, { steps: 5 });
  await page.mouse.up();

  await page.getByRole('link', { name: 'Builder' }).click();
  await expect(page.getByRole('button', { name: 'Undo (1)' })).toBeEnabled();
});

test('malformed request bodies are refused with 422', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url } = await startEngine();

  for (const [path, method] of [
    ['api/songs/demo/cues/40', 'put'],
    ['api/songs/demo/bpm', 'put'],
    ['api/midi', 'post'],
    ['api/preview/pixel', 'put'],
    ['api/pixel-thumbnail', 'post'],
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
    page.getByRole('button', { name: 'Set tempo' }),
    page.getByRole('button', { name: 'New song' }),
    page.getByRole('button', { name: /^Undo/ }),
    page.getByRole('button', { name: 'New scene' }),
    page.getByRole('button', { name: 'Audition scene 38' }),
  ]) {
    await expect(control).toBeDisabled();
  }
  // The pads still show what each scene looks like: thumbnails are reads.
  await expect(pad(page, 38).locator('circle')).toHaveCount(16);
  expect((await page.request.get(`${url}api/songs/demo/thumbnails`)).status()).toBe(200);

  // A scene still opens, read-only.
  await pad(page, 38).click();
  await expect(page.getByText('Scene 38', { exact: true })).toBeVisible();
  for (const control of [ingredient(page, 'Wash'), page.getByLabel('Name'), page.getByLabel('Number'), page.getByRole('button', { name: 'Audition', exact: true })]) {
    await expect(control).toBeDisabled();
  }

  const refusals = [
    page.request.put(`${url}api/songs/demo/cues/40`, { data: { cue: { name: 'x', type: 'scene', values: {} } } }),
    page.request.delete(`${url}api/songs/demo/cues/38`),
    page.request.put(`${url}api/songs/demo/bpm`, { data: { bpm: 90 } }),
    page.request.post(`${url}api/songs`, { data: { name: 'encore', program: 7 } }),
    page.request.post(`${url}api/redo`),
    page.request.delete(`${url}api/preview`),
    page.request.put(`${url}api/preview/pixel`, { data: { song: 'demo', cue: { name: 'x', type: 'pixel' } } }),
    page.request.post(`${url}api/midi`, { data: { message: [0x91, 38, 127] } }),
    page.request.post(`${url}api/undo`),
  ];
  for (const response of await Promise.all(refusals)) expect(response.status(), response.url()).toBe(403);
  expect(readFileSync(songFile, 'utf8')).toBe(before);
});
