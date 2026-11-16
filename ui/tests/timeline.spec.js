import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, recordMidi } from './engine.js';

const ROOT = path.resolve(import.meta.dirname, '../..');
// The C20 fixture: verse, 2 bars of 4/4, and waltz, 2 bars of 3/4, played twice each.
const ARRANGED = readFileSync(path.join(ROOT, 'tests/fixtures/shows/arranged/songs/arranged.yaml'), 'utf8');
const UNARRANGED = ARRANGED.slice(0, ARRANGED.indexOf('sections:'));
const VERSE_BEATS = 8;
const WALTZ_BEATS = 6;
/** Words of the MIDI mapping the builder keeps to itself: scenes and songs go by number. */
const MIDI_WORDS = /\b(notes?|programs?|midi|velocity|cues?|banks?)\b/i;

test.use({ showSource: 'tests/fixtures/shows/arranged' });
/** `fire` needs a look to hold: the fixture's rig starts dark. */
const DARK = ['--set', 'both', 'dimmer=0%'];

const songFile = (show) => path.join(show, 'songs/arranged.yaml');
const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
const track = (page, lane) => page.locator(`[data-lane="${lane}"] .track`);
const block = (page, scene) => page.locator(`.section-editor [data-block="${scene}"]`);

/** Drags scene `scene` from the strip onto the open section, landing on beat `beat` (from 0). */
async function placeScene(page, scene, beat, sectionBeats) {
  const target = track(page, 'new');
  const box = await target.boundingBox();
  await pad(page, scene).dragTo(target, { targetPosition: { x: ((beat + 0.5) * box.width) / sectionBeats, y: box.height / 2 } });
}

/** Drags the block of `scene` by its `part`, its body or its end, `beats` to the right. */
async function dragBlock(page, scene, beats, sectionBeats, part = '.handle') {
  const beatWidth = (await track(page, scene).boundingBox()).width / sectionBeats;
  const grip = part === '.handle' ? block(page, scene).locator('.handle') : block(page, scene);
  const box = await grip.boundingBox();
  const [x, y] = [box.x + Math.min(box.width / 2, 20), box.y + box.height / 2];
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + beats * beatWidth, y, { steps: 5 });
  await page.mouse.up();
}

async function newSection(page, name) {
  await page.getByLabel('New section name').fill(name);
  await page.getByRole('button', { name: 'New section' }).click();
  await expect(page.getByLabel('Name', { exact: true })).toHaveValue(name);
}

async function setField(page, label, value) {
  await page.getByLabel(label, { exact: true }).fill(value);
  await page.getByLabel(label, { exact: true }).press('Tab');
}

test('the fixture arrangement built from empty by drag, resize and level saves the fixture, with undo and redo', async ({ page, show, startEngine }) => {
  writeFileSync(songFile(show), UNARRANGED);
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);
  await page.getByRole('button', { name: 'Timeline' }).click();
  await expect(page).toHaveURL(/#builder\/arranged\/timeline$/);
  await expect(page.getByText('Create a section, then drag scenes onto it.')).toBeVisible();

  await newSection(page, 'verse');
  await setField(page, 'Bars', '2');
  await expect(page.getByRole('button', { name: /^verse 2 bars/ })).toBeVisible();
  await placeScene(page, 36, 0, VERSE_BEATS);
  await expect(block(page, 36)).toHaveAccessibleName('red, scene 36: bar 1 beat 1, 1 bar, 100%');
  await dragBlock(page, 36, 4, VERSE_BEATS);
  await expect(block(page, 36)).toHaveAccessibleName('red, scene 36: bar 1 beat 1, 2 bars, 100%');
  await placeScene(page, 37, 6, VERSE_BEATS);
  await expect(block(page, 37)).toHaveAccessibleName('blue, scene 37: bar 2 beat 3, 2 beats, 100%');
  await setField(page, 'Level', '50');
  await expect(block(page, 37)).toHaveAccessibleName('blue, scene 37: bar 2 beat 3, 2 beats, 50%');

  await newSection(page, 'waltz');
  await setField(page, 'Bars', '2');
  await setField(page, 'Time signature', '3/4');
  await expect(page.getByRole('button', { name: /^waltz 2 bars · 3\/4/ })).toBeVisible();
  await placeScene(page, 36, 0, WALTZ_BEATS);
  await expect(block(page, 36)).toHaveAccessibleName('red, scene 36: bar 1 beat 1, 1 bar, 100%');
  await placeScene(page, 42, 0, WALTZ_BEATS);
  await dragBlock(page, 42, 3, WALTZ_BEATS);
  await expect(block(page, 42)).toHaveAccessibleName('pulse, scene 42: bar 1 beat 1, 2 bars, 100%');

  for (const [name, count] of [['verse', 1], ['waltz', 2], ['verse', 3], ['waltz', 4]]) {
    await page.getByRole('button', { name: `Add ${name} to the arrangement` }).click();
    await expect(page.locator('.tile')).toHaveCount(count);
  }
  await expect.poll(() => readFileSync(songFile(show), 'utf8')).toBe(ARRANGED);
  await expect(page.getByTestId('song-length')).toHaveText('8 bars · 0:14');
  await expect(page.locator('main')).not.toContainText(MIDI_WORDS);

  await page.getByRole('button', { name: /^Undo/ }).click();
  await expect(page.locator('.tile')).toHaveCount(3);
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED.replace('[verse, waltz, verse, waltz]', '[verse, waltz, verse]'));
  await page.getByRole('button', { name: /^Redo/ }).click();
  await expect(page.locator('.tile')).toHaveCount(4);
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED);
});

test('editing a block of a reused section changes every placement on the overview, which a click opens', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  const verses = page.locator('[data-placement="verse"]');
  await expect(verses).toHaveCount(2);
  await expect(page.locator('.placement.open'), 'the open section, everywhere it plays').toHaveCount(2);

  await block(page, 37).click();
  await setField(page, 'Level', '80');
  for (const verse of await verses.all()) {
    await expect(verse.locator('.mini[title="blue, scene 37: bar 2 beat 3, 2 beats, 80%"]')).toHaveCount(1);
  }
  await dragBlock(page, 37, -1, VERSE_BEATS, 'body');
  for (const verse of await verses.all()) {
    await expect(verse.locator('.mini[title="blue, scene 37: bar 2 beat 2, 2 beats, 80%"]')).toHaveCount(1);
  }
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED.replace('bar: 2, beat: 3, length: 2 beats, level: 50%', 'bar: 2, beat: 2, length: 2 beats, level: 80%'));

  await page.locator('[data-placement="waltz"]').first().click();
  await expect(page.getByLabel('Name', { exact: true })).toHaveValue('waltz');
  await expect(page.locator('.tile.open')).toHaveCount(2);
});

test('reordering the strip writes the new order, and a placed section is not deleted', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  const tiles = page.locator('.tile');

  await tiles.nth(0).dragTo(tiles.nth(1));
  await expect(tiles.nth(0)).toContainText('waltz');
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED.replace('[verse, waltz, verse, waltz]', '[waltz, verse, verse, waltz]'));

  await page.getByRole('button', { name: /^verse 2 bars/ }).click();
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveText('verse plays in the arrangement: take it out of the arrangement before deleting it.');
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED.replace('[verse, waltz, verse, waltz]', '[waltz, verse, verse, waltz]'));

  // Taken out everywhere, it goes.
  await page.getByRole('button', { name: 'Take verse at bar 3 out of the arrangement' }).click();
  await expect(tiles).toHaveCount(3);
  await page.getByRole('button', { name: 'Take verse at bar 3 out of the arrangement' }).click();
  await expect(tiles).toHaveCount(2);
  await page.getByRole('button', { name: /^verse 2 bars/ }).click();
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByRole('button', { name: /^verse 2 bars/ })).toHaveCount(0);
  expect(readFileSync(songFile(show), 'utf8')).not.toContain('verse');
});

test('an edit the engine refuses shows its error and leaves the file untouched', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.getByLabel('Name', { exact: true })).toHaveValue('verse');

  await setField(page, 'Bars', '1');
  await expect(page.getByRole('alert')).toContainText('runs 4 beats past the end of the section');
  await expect(page.getByLabel('Bars', { exact: true }), 'back to what the file says').toHaveValue('2');
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED);

  // Two blocks of one scene can't overlap.
  await placeScene(page, 36, 4, VERSE_BEATS);
  await expect(page.getByRole('alert')).toContainText('overlaps block 1 of scene 36');
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED);
});

test('holding a block sends note-on at its level and letting go note-off', async ({ page, show, startEngine }) => {
  const { url, address } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(block(page, 37)).toBeVisible();
  const midi = await recordMidi(address);

  await block(page, 37).hover();
  await page.mouse.down();
  await expect.poll(() => midi.messages).toEqual([[0xc1, 1], [0x91, 37, 64]]);
  await expect(block(page, 37)).toHaveClass(/playing/);
  await page.mouse.up();
  await expect.poll(() => midi.messages).toEqual([[0xc1, 1], [0x91, 37, 64], [0x81, 37, 0]]);
  expect(readFileSync(songFile(show), 'utf8'), 'a hold moves nothing').toBe(ARRANGED);
  midi.close();
});

test('Export to DAW downloads the file the engine exports', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);

  const downloading = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Export to DAW' }).click();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe('arranged.mid');
  expect(readFileSync(await download.path())).toEqual(readFileSync(path.join(show, 'export/arranged.mid')));
});

test('show mode is read-only', async ({ page, show, startEngine }) => {
  const { url, address } = await startEngine({ look: DARK, edit: false });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(block(page, 37)).toBeVisible();
  const midi = await recordMidi(address);

  await expect(page.getByRole('button', { name: 'New section' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Add verse to the arrangement' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Take verse at bar 1 out of the arrangement' })).toBeDisabled();
  await expect(page.getByLabel('Bars', { exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: /^Undo/ })).toBeDisabled();

  await dragBlock(page, 37, -2, VERSE_BEATS, 'body');
  await page.locator('.tile').nth(0).dragTo(page.locator('.tile').nth(1));
  await placeScene(page, 42, 0, VERSE_BEATS);
  await page.waitForTimeout(500);
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED);
  expect(midi.messages, 'no audition').toEqual([]);
  midi.close();
});

const undo = (page) => page.getByRole('button', { name: /^Undo/ });

test('a new time signature keeps blocks in their bars and beats, and refuses what no longer fits', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await page.getByRole('button', { name: /^waltz 2 bars/ }).click();

  await setField(page, 'Time signature', '4/4');
  const inFourFour = ARRANGED.replace('    time_signature: 3/4\n', '    time_signature: 4/4\n');
  await expect.poll(() => readFileSync(songFile(show), 'utf8'), 'one bar stays one bar, two stay two').toBe(inFourFour);

  await page.getByRole('button', { name: /^verse 2 bars/ }).click();
  await setField(page, 'Time signature', '3/4');
  await expect(page.getByRole('alert')).toHaveText("blue (scene 37) at bar 2 beat 3 doesn't fit in 2 bars of 3/4: move or shorten it first.");
  await setField(page, 'Time signature', '3/x');
  await expect(page.getByRole('alert')).toHaveText('"3/x" is not a time signature: write beats over the note value, as in 3/4 or 6/8.');
  await expect(page.getByLabel('Time signature')).toHaveValue('');
  expect(readFileSync(songFile(show), 'utf8')).toBe(inFourFour);
});

test('arrow keys move a block a beat a press, keeping focus, and a move against the edge saves nothing', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);

  await block(page, 37).focus();
  for (let press = 0; press < 3; press += 1) await page.keyboard.press('ArrowLeft');
  await expect(block(page, 37)).toHaveAccessibleName('blue, scene 37: bar 1 beat 4, 2 beats, 50%');
  await expect(block(page, 37)).toBeFocused();
  await expect(undo(page)).toHaveText('Undo (3)');
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED.replace('bar: 2, beat: 3, length: 2 beats', 'bar: 1, beat: 4, length: 2 beats'));

  await block(page, 36).focus();
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('Shift+ArrowRight');
  await page.waitForTimeout(500);
  await expect(undo(page), 'red fills the section already').toHaveText('Undo (3)');
});

test('a gesture that ends where it started saves nothing', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);

  await block(page, 37).click();
  const box = await block(page, 37).boundingBox();
  const beatWidth = (await track(page, 37).boundingBox()).width / VERSE_BEATS;
  await page.mouse.move(box.x + 20, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 20 - 2 * beatWidth, box.y + box.height / 2, { steps: 4 });
  await page.mouse.move(box.x + 20, box.y + box.height / 2, { steps: 4 });
  await page.mouse.up();
  await page.waitForTimeout(500);
  await expect(undo(page)).toHaveText('Undo (0)');
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED);
});

test('a refused level keeps the block selected, and a save that leaves the scenes alone renders no thumbnails', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(pad(page, 36).locator('circle').first()).not.toHaveAttribute('fill', '#000000');
  let thumbnailLoads = 0;
  page.on('request', (sent) => sent.url().endsWith('/thumbnails') && (thumbnailLoads += 1));

  await block(page, 37).click();
  await setField(page, 'Level', '150');
  await expect(page.getByRole('alert')).toContainText('150%');
  await expect(page.getByLabel('Level'), 'still selected, back to the file').toHaveValue('50');

  await setField(page, 'Level', '80');
  await expect(block(page, 37)).toHaveAccessibleName('blue, scene 37: bar 2 beat 3, 2 beats, 80%');
  await page.waitForTimeout(500);
  expect(thumbnailLoads).toBe(0);
});

test('a quick second click on a placement already taken out takes out nothing more', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);

  // Both clicks land before the first save reloads the strip.
  await page.getByRole('button', { name: 'Take waltz at bar 3 out of the arrangement' }).evaluate((button) => {
    button.click();
    button.click();
  });
  await expect(page.locator('.tile')).toHaveCount(3);
  await page.waitForTimeout(500);
  expect(readFileSync(songFile(show), 'utf8')).toBe(ARRANGED.replace('[verse, waltz, verse, waltz]', '[verse, verse, waltz]'));
  await expect(undo(page)).toHaveText('Undo (1)');
});

test("switching to another song's timeline shows only its own, and an unknown song goes home", async ({ page, show, startEngine }) => {
  const encore = 'program: 2\ncues:\n  5: { name: amber, type: scene, values: { both: { color: red } } }\nsections:\n  chorus:\n    bars: 1\n    blocks:\n      - { scene: 5, bar: 1, length: 1 bar }\narrangement: [chorus]\n';
  writeFileSync(path.join(show, 'songs/encore.yaml'), encore);
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder/arranged/timeline`);
  await expect(page.locator('.tile')).toHaveCount(4);
  let isLoaded = null;
  const loading = new Promise((resolve) => (isLoaded = resolve));
  await page.route('**/api/show/songs/encore.yaml', async (route) => {
    await loading;
    await route.continue();
  });

  await page.evaluate(() => (location.hash = '#builder/encore/timeline'));
  await expect(page.getByText('Song 2 · encore: timeline')).toBeVisible();
  await expect(page.locator('.tile'), "nothing of arranged's under encore").toHaveCount(0);
  isLoaded();
  await expect(page.locator('.tile')).toHaveCount(1);
  await expect(page.getByLabel('Name', { exact: true })).toHaveValue('chorus');

  await page.evaluate(() => (location.hash = '#builder/nope/timeline'));
  await expect(page).toHaveURL(/#builder$/);
  await expect(page.getByRole('alert')).toHaveText('There is no song nope in the show.');
});
