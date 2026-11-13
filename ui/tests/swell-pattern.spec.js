import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect } from './engine.js';

const HEADER = '# Demo song for the builder tests.\nprogram: 1\nbpm: 120\ncues:\n';
const BLACKOUT = '  38:\n    name: blackout\n    type: scene\n    values:\n      all-wash: { dimmer: 0% }\n';

/** The plan's example song, in canonical form: C4's swell and C5's pattern references. */
const TELL_ME = readFileSync(path.resolve(import.meta.dirname, '../../shows/fossile/songs/tell-me.yaml'), 'utf8');
const cueText = (from, to) => TELL_ME.slice(TELL_ME.indexOf(`  ${from}:\n`), to ? TELL_ME.indexOf(`  ${to}:\n`) : undefined);
const CHORUS_SWELL = cueText(39, 40);
const VERSE_PULSE = cueText(42);

function addSong(show, text) {
  mkdirSync(path.join(show, 'songs'));
  const file = path.join(show, 'songs/demo.yaml');
  writeFileSync(file, text);
  return file;
}

async function newCue(page, note, name, type) {
  await page.getByLabel('Note').fill(String(note));
  await page.getByLabel('Name').fill(name);
  await page.getByLabel('Type').selectOption(type);
}

const step = (page, at) => page.getByRole('button', { name: `at ${at}`, exact: true });

test("the 1 / 2 / 3.5 pattern built in the grid saves as C5's reference", async ({ page, show, startEngine }) => {
  const songFile = addSong(show, HEADER + BLACKOUT);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await newCue(page, 42, 'verse-pulse', 'pattern');
  await expect(page.getByLabel('Target', { exact: true })).toHaveValue('all-wash');
  await expect(page.getByLabel('Drives')).toHaveValue('dimmer');
  await step(page, 1).click();
  await step(page, 2).click();
  await page.getByLabel('Hit shape').selectOption('sine');
  await page.getByLabel('Hit level').fill('80');
  await step(page, 3.5).click();
  await page.getByLabel('Hit shape').selectOption('ramp-up');
  await page.getByLabel('Hit level').fill('100');
  await expect(page.locator('.hit-level')).toHaveCount(6);
  await page.getByRole('button', { name: 'Save cue' }).click();

  await expect(page.getByText('Saved cue 42 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(HEADER + BLACKOUT + VERSE_PULSE);

  // The saved loop plays from the next bar line, the playhead following it.
  await page.getByRole('button', { name: 'Audition loop' }).click();
  await expect(page.locator('.lane .playhead')).toHaveCount(1);
  await page.getByRole('button', { name: 'Stop audition' }).click();
  await expect(page.locator('.lane .playhead')).toHaveCount(0);
});

test('editing a pattern changes only the hits edited', async ({ page, show, startEngine }) => {
  const pattern =
    '  42:\n    name: groove\n    type: pattern\n    length: 2 bars\n' +
    '    target:\n      outer-wash: { color: deep-blue }\n    hits:\n' +
    '      - { at: 1, shape: sine, length: 1 beat, level: 65% }\n' +
    '      -\n        at: 1.5\n        shape:\n          points:\n            - [0, 0]\n            - [0.5, 1]\n            - [1, 0]\n' +
    '        length: 1 bar\n        level: 50%\n' +
    '      - { at: 7, shape: hold, length: 0.25 beats, level: 40% }\n' +
    '      - { at: 2.75, shape: ease-in, length: 1 bar, level: 100% }\n' +
    '    release: 1 beat\n';
  const songFile = addSong(show, HEADER + BLACKOUT + pattern);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await page.locator('tr[data-note="42"]').getByRole('button', { name: 'Edit' }).click();
  await expect(page.getByLabel('Drives')).toHaveValue('colour');
  await expect(page.getByLabel('Target colour')).toHaveValue('deep-blue');
  await step(page, 7).click();
  await page.getByLabel('Hit level').fill('90');
  await step(page, 8).click();
  await page.getByRole('button', { name: 'Save cue' }).click();

  await expect(page.getByText('Saved cue 42 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    HEADER +
      BLACKOUT +
      pattern
        .replace('level: 40%', 'level: 90%')
        .replace('    release:', '      - { at: 8, shape: hold, length: 0.25 beats, level: 90% }\n    release:'),
  );

  // Copying bar 1 over bar 2 replaces bar 2's hits with bar 1's.
  await page.getByLabel('Copy bar').selectOption('1');
  await page.getByLabel('to bar').selectOption('2');
  await page.getByRole('button', { name: 'Copy', exact: true }).click();
  for (const at of [5, 5.5, 6.75]) await expect(step(page, at)).toHaveAttribute('aria-pressed', 'true');
  for (const at of [7, 8]) await expect(step(page, at)).toHaveAttribute('aria-pressed', 'false');
});

test("a 4-bar ease-in swell saves as C4's reference and draws the engine's curve", async ({ page, show, startEngine }) => {
  const songFile = addSong(show, HEADER + BLACKOUT);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await newCue(page, 39, 'chorus-swell', 'swell');
  await page.getByLabel('Curve', { exact: true }).selectOption('ease-in');
  await page.getByLabel('Swell length', { exact: true }).fill('4');
  await page.getByLabel('Swell length unit').selectOption('bars');
  await page.getByLabel('Release').fill('500ms');
  // Halfway through, ease-in is at a quarter: 120 of the curve's 160 units down.
  await expect(page.locator('.curve polyline')).toHaveAttribute('points', /(^| )300,120( |$)/);
  await page.getByRole('button', { name: 'Save cue' }).click();

  await expect(page.getByText('Saved cue 39 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(HEADER + BLACKOUT + CHORUS_SWELL);

  // Custom points start from the curve on screen; dragging the middle one to the top moves it.
  await page.getByLabel('Curve', { exact: true }).selectOption('custom');
  await expect(page.locator('.curve .point')).toHaveCount(5);
  await page.locator('.curve').scrollIntoViewIfNeeded();
  const curve = await page.locator('.curve').boundingBox();
  const middle = await page.locator('.curve .point[data-point="2"]').boundingBox();
  await page.mouse.move(middle.x + middle.width / 2, middle.y + middle.height / 2);
  await page.mouse.down();
  await page.mouse.move(curve.x + curve.width / 2, curve.y - 20, { steps: 5 });
  await page.mouse.up();
  await page.getByLabel('Swell length', { exact: true }).fill('1');
  await page.getByLabel('Swell length unit').selectOption('beats');
  await page.getByRole('button', { name: 'Save cue' }).click();

  await expect(page.getByText('Saved cue 39 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    HEADER +
      BLACKOUT +
      CHORUS_SWELL.replace('duration: 4 bars', 'duration: 1 beat').replace(
        'curve: ease-in',
        'curve:\n      points:\n        - [0, 0]\n        - [0.25, 0.06]\n        - [0.5, 1]\n        - [0.75, 0.56]\n        - [1, 1]',
      ),
  );

  // A looped audition plays the swell, holds its end a bar, then plays it again.
  await page.getByRole('button', { name: 'Audition loop' }).click();
  const playhead = page.locator('.curve .playhead');
  await expect(playhead).toHaveAttribute('x1', '600');
  await expect(playhead).not.toHaveAttribute('x1', '600', { timeout: 5000 });
  await page.getByRole('button', { name: 'Stop audition' }).click();
  await expect(playhead).toHaveCount(0);
});
