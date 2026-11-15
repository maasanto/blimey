import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect } from './engine.js';

const HEADER = 'program: 1\nbpm: 120\ncues:\n';
const scene = (number, name, values) => `  ${number}:\n    name: ${name}\n    type: scene\n    values:\n      ${values}\n`;
const HOUSE = scene(1, 'house', 'all-wash: { color: warm-white }');
const VERSE = scene(36, 'verse', 'all-wash: { color: deep-blue }');
const CHORUS = scene(37, 'chorus', 'outer-wash: { color: warm-white }');
const RISE = '  39:\n    name: rise\n    type: swell\n    target:\n      all-wash: { dimmer: 100% }\n    duration: 1 bar\n';
const CHASE = '  40:\n    name: chase\n    type: pixel\n    effect: chase\n    params: { color: deep-blue, width: 0.25 }\n';
const SONG = HEADER + HOUSE + VERSE + CHORUS + RISE + CHASE;
/** Words of the MIDI mapping the builder keeps to itself: scenes and songs go by number. */
const MIDI_WORDS = /\b(notes?|programs?|midi|velocity|cues?|banks?)\b/i;

function addSong(show, text = SONG) {
  mkdirSync(path.join(show, 'songs'));
  const file = path.join(show, 'songs/demo.yaml');
  writeFileSync(file, text);
  return file;
}

const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
const firstLight = (page, number) => pad(page, number).locator('circle').first();
const backToScenes = (page) => page.getByRole('link', { name: '← Scenes' });
const leavePrompt = (page) => page.getByRole('dialog');

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

test('the scenes home shows the song file in number order after a load, a save and an undo', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await expect(page.locator('[data-pad]')).toHaveCount(5);
  expect(await page.locator('[data-pad]').evaluateAll((pads) => pads.map((pad) => pad.getAttribute('aria-label')))).toEqual([
    'Scene 1: house',
    'Scene 36: verse',
    'Scene 37: chorus',
    'Scene 39: rise',
    'Scene 40: chase',
  ]);
  await expect(page.getByRole('button', { name: 'New scene' })).toBeVisible();
  await expect(page.getByLabel('Song')).toHaveText('Song 1 · demo');
  await expect(page.locator('main')).not.toContainText(MIDI_WORDS);
  // Thumbnails rendered by the engine at the rig's pixels, or a swell's envelope.
  await expect(firstLight(page, 36)).toHaveAttribute('fill', '#0028ff');
  await expect(pad(page, 37).locator('circle').nth(4), 'wash-2, inside the outer washes').toHaveAttribute('fill', '#000000');
  await expect(firstLight(page, 37)).toHaveAttribute('fill', '#ffd2a0');
  await expect(firstLight(page, 1)).toHaveAttribute('fill', '#ffd2a0');
  await expect(pad(page, 39).locator('polyline')).toHaveCount(1);
  await expect(pad(page, 40).locator('circle')).toHaveCount(16);

  // A click opens the editor on its own screen; the fixed look opens on the stage and saves as a look.
  await pad(page, 36).click();
  await expect(page).toHaveURL(/#builder\/demo\/36$/);
  await expect(page.getByText('Scene 36', { exact: true })).toBeVisible();
  await expect(page.locator('[data-pad]')).toHaveCount(0);
  await expect(page.locator('main')).not.toContainText(MIDI_WORDS);
  await page.getByLabel('Name').fill('verse-warm');
  await page.getByRole('button', { name: 'deep-blue wash everywhere', exact: true }).click();
  await page.getByRole('button', { name: 'Colour: warm-white' }).click();
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Saved scene 36 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toContain('    name: verse-warm\n');

  await backToScenes(page).click();
  await expect(pad(page, 36)).toHaveAccessibleName('Scene 36: verse-warm');
  await expect(firstLight(page, 36)).toHaveAttribute('fill', '#ffd2a0');
  await page.getByRole('button', { name: 'Undo (1)' }).click();
  await expect(pad(page, 36)).toHaveAccessibleName('Scene 36: verse');
  await expect(firstLight(page, 36)).toHaveAttribute('fill', '#0028ff');
  expect(readFileSync(songFile, 'utf8')).toBe(SONG);
});

test('browser back returns home, and leaving with unsaved edits asks first', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await expect(page.getByLabel('Name')).toHaveValue('chase');
  await expect(page).toHaveTitle('Blimey');
  await page.goBack();
  await expect(pad(page, 40)).toBeVisible();
  await expect(leavePrompt(page), 'nothing to lose').toBeHidden();

  await pad(page, 40).click();
  await page.getByLabel('Name').fill('changed');
  await expect(page).toHaveTitle('• Scene 40 — Blimey');
  await backToScenes(page).click();
  await expect(leavePrompt(page)).toContainText('This scene has unsaved changes.');
  await leavePrompt(page).getByRole('button', { name: 'Stay' }).click();
  await expect(leavePrompt(page)).toBeHidden();
  await expect(page.getByLabel('Name'), 'kept, on the same scene').toHaveValue('changed');
  await expect(page).toHaveURL(/#builder\/demo\/40$/);

  await page.goBack();
  await leavePrompt(page).getByRole('button', { name: 'Leave without saving' }).click();
  await expect(pad(page, 40)).toHaveAccessibleName('Scene 40: chase');
  await expect(page).toHaveTitle('Blimey');
  expect(readFileSync(songFile, 'utf8')).toBe(SONG);
});

test('save and leave writes the scene, then goes where the user was heading', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);
  await pad(page, 40).click();

  await page.getByLabel('Name').fill('chase-saved');
  await page.goBack();
  await leavePrompt(page).getByRole('button', { name: 'Save and leave' }).click();

  await expect(pad(page, 40)).toHaveAccessibleName('Scene 40: chase-saved');
  await expect(page).toHaveURL(/#builder$/);
  expect(readFileSync(songFile, 'utf8')).toContain('    name: chase-saved\n');

  // A scene that can't be saved yet says so in the prompt, with Save and leave off.
  await pad(page, 40).click();
  await page.getByLabel('Name').fill('');
  await backToScenes(page).click();
  await expect(leavePrompt(page).getByRole('button', { name: 'Save and leave' })).toBeDisabled();
  await expect(leavePrompt(page)).toContainText('Name the scene to save it.');
  await leavePrompt(page).getByRole('button', { name: 'Stay' }).click();
});

test('a refused leave keeps history and the Builder tab on the scene', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#monitor`);

  await page.getByRole('link', { name: 'Builder' }).click();
  await pad(page, 40).click();
  await page.getByLabel('Name').fill('changed');
  await page.goBack();
  await leavePrompt(page).getByRole('button', { name: 'Stay' }).click();
  await expect(page).toHaveURL(/#builder\/demo\/40$/);
  await expect(page.getByLabel('Name')).toHaveValue('changed');

  // Back once more leaves the builder without asking: the edits wait there, as on any tab switch.
  await page.goBack();
  await expect(page).toHaveURL(/#monitor$/);
  await expect(leavePrompt(page)).toHaveCount(0);
  await page.getByRole('link', { name: 'Builder' }).click();
  await expect(page).toHaveURL(/#builder\/demo\/40$/);
  await expect(page.getByLabel('Name')).toHaveValue('changed');

  await backToScenes(page).click();
  await leavePrompt(page).getByRole('button', { name: 'Leave without saving' }).click();
  await expect(pad(page, 40)).toBeVisible();
});

test('leaving without saving after Back keeps history as it was: Back again reaches the page before', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#monitor`);

  await page.getByRole('link', { name: 'Builder' }).click();
  await pad(page, 40).click();
  await page.getByLabel('Name').fill('changed');
  await page.goBack();
  await leavePrompt(page).getByRole('button', { name: 'Leave without saving' }).click();
  await expect(page).toHaveURL(/#builder$/);
  await expect(pad(page, 40)).toBeVisible();

  await page.goBack();
  await expect(page).toHaveURL(/#monitor$/);
});

test('the Builder tab opened on another URL with unsaved edits shows them again rather than failing', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url } = await startEngine();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await page.getByLabel('Name').fill('changed');
  await page.getByRole('link', { name: 'Monitor' }).click();
  await page.evaluate(() => (location.hash = '#builder'));

  await expect(page).toHaveURL(/#builder\/demo\/40$/);
  await expect(page.getByLabel('Name')).toHaveValue('changed');
  await expect(leavePrompt(page)).toBeHidden();
  expect(errors).toEqual([]);
});

test('unsaved edits are guarded with the engine gone and when the page unloads', async ({ page, show, startEngine }) => {
  addSong(show);
  const engine = await startEngine();
  await page.goto(`${engine.url}#builder`);
  await pad(page, 40).click();
  await page.getByLabel('Name').fill('changed');
  await engine.stop();
  await expect(page.locator('.connection')).toHaveAttribute('data-connected', 'false');

  await backToScenes(page).click();
  await expect(leavePrompt(page).getByRole('button', { name: 'Save and leave' }), 'no engine to save to').toBeDisabled();
  await leavePrompt(page).getByRole('button', { name: 'Stay' }).click();
  await expect(page.getByLabel('Name')).toHaveValue('changed');

  const unloading = page.waitForEvent('dialog');
  await page.close({ runBeforeUnload: true });
  const prompt = await unloading;
  expect(prompt.type()).toBe('beforeunload');
  await prompt.accept();
});

test('a URL to an unknown song or a number the engine never takes goes home, saying why', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url } = await startEngine();

  await page.goto(`${url}#builder/no-such-song/3`);
  await expect(page).toHaveURL(/#builder$/);
  await expect(page.getByRole('alert')).toContainText('no-such-song');
  await expect(page.getByLabel('Song')).toHaveValue('demo');
  await expect(pad(page, 36)).toBeVisible();

  await page.goto(`${url}#builder/demo/300`);
  await expect(page).toHaveURL(/#builder$/);
  await expect(page.getByRole('alert')).toHaveText('There is no scene 300: scene numbers go from 0 to 127.');
  await expect(pad(page, 36)).toBeVisible();
});

test("a slow load of the song switched away from doesn't replace the one opened from the URL", async ({ page, show, startEngine }) => {
  addSong(show);
  writeFileSync(path.join(show, 'songs/encore.yaml'), 'program: 2\ncues:\n  5:\n    name: encore-look\n    type: pixel\n    params: { color: deep-blue }\n');
  const { url } = await startEngine();
  await page.route('**/api/show/songs/demo.yaml', async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 1500));
    await route.continue();
  });

  // The home starts loading demo; a link to encore's scene comes in before it arrives.
  await page.goto(`${url}#builder`);
  await expect(page.getByLabel('Song')).toHaveValue('demo');
  await page.evaluate(() => (location.hash = '#builder/encore/5'));
  await expect(page.getByLabel('Name')).toHaveValue('encore-look');
  await page.waitForTimeout(2000);
  await backToScenes(page).click();
  await expect(pad(page, 5)).toHaveAccessibleName('Scene 5: encore-look');
  await expect(pad(page, 36)).toHaveCount(0);
});

test('a new scene takes the lowest free number and saves under it', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await page.getByRole('button', { name: 'New scene' }).click();
  await expect(page).toHaveURL(/#builder\/demo\/2$/);
  await expect(page.getByText('New scene', { exact: true })).toBeVisible();
  await expect(page.getByLabel('Number')).toHaveValue('2');
  await page.getByLabel('Name').fill('fresh');
  await page.getByRole('group', { name: 'Ingredients' }).getByRole('button', { name: 'Wash' }).click();
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText('Saved scene 2 in demo.')).toBeVisible();
  await expect(page.getByText('Scene 2', { exact: true })).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    HEADER + HOUSE + '  2:\n    name: fresh\n    type: pixel\n    layers:\n      -\n        shape: fill\n        params: { color: deep-blue }\n        blend: over\n' +
      VERSE + CHORUS + RISE + CHASE,
  );
  await backToScenes(page).click();
  await expect(page.locator('[data-pad]').nth(1)).toHaveAccessibleName('Scene 2: fresh');
});

test('duplicate goes to the next free number and delete returns home', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);
  const copy = scene(38, 'verse-copy', 'all-wash: { color: deep-blue }');

  await pad(page, 36).click();
  await page.getByRole('button', { name: 'Duplicate' }).click();
  await expect(page.getByText('Duplicated scene 36 as scene 38.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(HEADER + HOUSE + VERSE + CHORUS + copy + RISE + CHASE);

  await backToScenes(page).click();
  await pad(page, 38).click();
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByText('Deleted scene 38. Undo brings it back.')).toBeVisible();
  await expect(pad(page, 38)).toHaveCount(0);
  await expect(page).toHaveURL(/#builder$/);
  expect(readFileSync(songFile, 'utf8')).toBe(SONG);
});

test('renumbering to a taken number is refused, leaving the file untouched; a free one moves the scene', async ({ page, show, startEngine }) => {
  const songFile = addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByText('Scene 40', { exact: true })).toBeVisible();
  await page.getByLabel('Number').fill('36');
  await expect(page.getByText('Taken by verse', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await expect(page.locator('.status')).toHaveText('Scene 36 is taken by verse.');
  await page.getByRole('button', { name: 'Save', exact: true }).press('Enter');
  expect(readFileSync(songFile, 'utf8')).toBe(SONG);

  // A number taken since the song was loaded is refused by the engine.
  await page.getByLabel('Number').fill('41');
  await expect(page.getByText('Taken by verse', { exact: true })).toHaveCount(0);
  writeFileSync(songFile, SONG.replace('  40:\n', '  41:\n    name: taken-by-hand\n    type: scene\n    values:\n      all-wash: { color: deep-blue }\n  40:\n'));
  await page.getByRole('button', { name: 'Save', exact: true }).press('Enter');
  await expect(page.getByRole('alert')).toHaveText('Scene 41 is already taken: pick a free number, or open that scene.');
  writeFileSync(songFile, SONG);

  // A scene changed on disk since it was opened is refused as such, whatever the number.
  writeFileSync(songFile, SONG.replace('name: chase', 'name: chase-by-hand'));
  await page.getByRole('button', { name: 'Save', exact: true }).press('Enter');
  await expect(page.getByRole('alert')).toContainText('cue 40 changed since it was loaded');
  writeFileSync(songFile, SONG);

  await page.getByLabel('Number').fill('44');
  await page.getByRole('button', { name: 'Save', exact: true }).press('Enter');
  await expect(page.getByText('Scene 44', { exact: true })).toBeVisible();
  await expect(page).toHaveURL(/#builder\/demo\/44$/);
  // Saved from the stage composer, the look takes the layers form.
  const moved = '  44:\n    name: chase\n    type: pixel\n    layers:\n      -\n        effect: chase\n        params: { color: deep-blue, width: 0.25 }\n';
  expect(readFileSync(songFile, 'utf8')).toBe(HEADER + HOUSE + VERSE + CHORUS + RISE + moved);

  await backToScenes(page).click();
  await expect(pad(page, 44)).toHaveAccessibleName('Scene 44: chase');
  await expect(pad(page, 40)).toHaveCount(0);
});

test('holding a pad sends note-on and letting go sends note-off; a click only opens it', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url, address } = await startEngine();
  await page.goto(`${url}#builder`);
  await expect(firstLight(page, 37)).toHaveAttribute('fill', '#ffd2a0');
  const midi = await recordMidi(address);

  await pad(page, 36).hover();
  await page.mouse.down();
  await expect.poll(() => midi.messages).toEqual([[0xc1, 1], [0x91, 36, 127]]);
  await expect(pad(page, 36)).toHaveClass(/playing/);

  await page.mouse.up();
  await expect.poll(() => midi.messages).toEqual([[0xc1, 1], [0x91, 36, 127], [0x81, 36, 0]]);
  await expect(pad(page, 36)).not.toHaveClass(/playing/);
  await expect(page, 'a hold is not a click').toHaveURL(/#builder$/);

  await pad(page, 37).click();
  await expect(page.getByText('Scene 37', { exact: true })).toBeVisible();
  expect(midi.messages, 'a click sends nothing').toHaveLength(3);
  midi.close();
});

const SELECTED_36 = [[0xc1, 1], [0x91, 36, 127]];

test('a second pointer pressed while a pad is held is ignored, so the held scene is always released', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url, address } = await startEngine();
  await page.goto(`${url}#builder`);
  const midi = await recordMidi(address);
  const pointer = async (number, pointerId) => {
    const box = await pad(page, number).boundingBox();
    return { pointerId, button: 0, bubbles: true, clientX: box.x + box.width / 2, clientY: box.y + box.height / 2 };
  };

  await pad(page, 36).dispatchEvent('pointerdown', await pointer(36, 1));
  await pad(page, 37).dispatchEvent('pointerdown', await pointer(37, 2));
  await expect.poll(() => midi.messages).toEqual(SELECTED_36);
  await pad(page, 37).dispatchEvent('pointerup', await pointer(37, 2));
  await page.waitForTimeout(500);
  expect(midi.messages, 'the second pointer neither fires nor releases').toEqual(SELECTED_36);

  await pad(page, 36).dispatchEvent('pointerup', await pointer(36, 1));
  await expect.poll(() => midi.messages).toEqual([...SELECTED_36, [0x81, 36, 0]]);
  midi.close();
});

test('holding the pad of a scene already playing leaves it playing', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url, address } = await startEngine();
  await page.goto(`${url}#builder`);
  for (const message of SELECTED_36) await page.request.post(`${url}api/midi`, { data: { message } });
  await expect(pad(page, 36)).toHaveClass(/playing/);
  const midi = await recordMidi(address);

  await pad(page, 36).hover();
  await page.mouse.down();
  await page.waitForTimeout(500);
  await page.mouse.up();
  await page.waitForTimeout(300);

  expect(midi.messages, 'no note-on to restart it, no note-off to stop it').toEqual([]);
  await expect(pad(page, 36)).toHaveClass(/playing/);
  midi.close();
});

test('from the keyboard, a pad auditions and releases its scene, and the editor does too', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url, address } = await startEngine();
  await page.goto(`${url}#builder`);
  const midi = await recordMidi(address);

  await page.getByRole('button', { name: 'Audition scene 40' }).focus();
  await page.keyboard.press('Enter');
  await expect.poll(() => midi.messages).toEqual([[0xc1, 1], [0x91, 40, 127]]);
  await page.getByRole('button', { name: 'Release scene 40' }).press('Enter');
  await expect.poll(() => midi.messages).toEqual([[0xc1, 1], [0x91, 40, 127], [0x81, 40, 0]]);

  await pad(page, 37).focus();
  await page.keyboard.press('Enter');
  await page.getByRole('button', { name: 'Audition', exact: true }).press('Enter');
  await expect.poll(() => midi.messages.slice(3)).toEqual([[0x91, 37, 127]]);
  await page.getByRole('button', { name: 'Release', exact: true }).press('Enter');
  await expect.poll(() => midi.messages.slice(3)).toEqual([[0x91, 37, 127], [0x81, 37, 0]]);
  midi.close();
});

test('thumbnails the engine refuses are cleared, saying why', async ({ page, show, startEngine }) => {
  addSong(show);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);
  await expect(firstLight(page, 36)).toHaveAttribute('fill', '#0028ff');

  const refusal = 'songs/demo.yaml: cue 40: speed is too long to render a cycle of';
  await page.route('**/api/songs/demo/thumbnails', (route) => route.fulfill({ status: 422, json: { error: refusal } }));
  await page.getByRole('button', { name: 'Set tempo' }).click();

  await expect(page.getByRole('alert')).toHaveText(`Thumbnails: ${refusal}`);
  await expect(pad(page, 36).locator('circle'), 'no stale picture').toHaveCount(0);
});
