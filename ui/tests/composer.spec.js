import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, BINARY } from './engine.js';

const ROOT = path.resolve(import.meta.dirname, '../..');
/** At 150 BPM the example's chase cycles in a bar: 64 frames, as in its reference dump. */
const LAST_FRAME_MS = 63 * 25;
/** Ticks per beat of the MIDI files written here; at the default 120 BPM, a tick is 0.5 ms. */
const DIVISION = 1000;

/** A MIDI variable-length quantity. */
function variableLength(value) {
  const bytes = [value & 0x7f];
  for (let rest = value >>> 7; rest; rest >>>= 7) bytes.unshift((rest & 0x7f) | 0x80);
  return bytes;
}

/** A Standard MIDI File (format 0) of `[ms, message]` events in time order. */
function midiFile(events) {
  let tick = 0;
  const track = events.flatMap(([ms, message]) => {
    const delta = (ms * DIVISION) / 500 - tick;
    tick += delta;
    return [...variableLength(delta), ...message];
  });
  track.push(0, 0xff, 0x2f, 0);
  const length = [24, 16, 8, 0].map((shift) => (track.length >>> shift) & 0xff);
  return Buffer.from([
    ...Buffer.from('MThd'), 0, 0, 0, 6, 0, 0, 0, 1, DIVISION >> 8, DIVISION & 0xff,
    ...Buffer.from('MTrk'), ...length, ...track,
  ]);
}

/**
 * The dump `blimey replay` renders of `show`'s saved files: its song on program 1 selected and
 * `note` fired at 0 ms, held to the last frame of a bar at 150 BPM.
 */
function replayHeld(show, note) {
  const midi = path.join(show, 'fire.mid');
  // A note-off for a note no cue uses, so the replay runs to that frame and nothing changes.
  writeFileSync(midi, midiFile([[0, [0xc1, 1]], [0, [0x91, note, 127]], [LAST_FRAME_MS, [0x81, 0, 0]]]));
  return execFileSync(BINARY, ['replay', show, '--midi', midi], { encoding: 'utf8' });
}

const pad = (page, note) => page.locator(`[data-pad="${note}"]`);

/** Where canvas point (`x`, `y`) is on the page's front view. */
async function stagePoint(page, [x, y]) {
  const stage = await page.getByRole('img', { name: 'Front view' }).locator('rect.stage').boundingBox();
  return { x: stage.x + x * stage.width, y: stage.y + (1 - y) * stage.height };
}

async function dragTo(page, from, to) {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(to.x, to.y, { steps: 5 });
  await page.mouse.up();
}

const centre = ({ x, y, width, height }) => ({ x: x + width / 2, y: y + height / 2 });

/** Drags an ingredient from the tray onto the stage at canvas point `at`. */
async function dropIngredient(page, ingredient, at) {
  const button = page.getByRole('group', { name: 'Ingredients' }).getByRole('button', { name: ingredient });
  await dragTo(page, centre(await button.boundingBox()), await stagePoint(page, at));
}

/** Drags a handle of the selected area to canvas point `to`. */
async function resize(page, handle, to) {
  await dragTo(page, centre(await page.locator(`[data-handle="${handle}"]`).boundingBox()), await stagePoint(page, to));
}

/** A click whose pointer wobbles a few screen pixels while down, as a hand's does. */
async function unsteadyClick(page, { x, y }) {
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 3, y - 2, { steps: 3 });
  await page.mouse.up();
}

test.describe('on three rows of washes', () => {
  test.use({ showSource: 'tests/fixtures/shows/three-rows' });
  const DARK = ['--set', 'all-wash', 'dimmer=0%'];

  test("plan §16's example look built on the stage saves C14's reference and plays its dump", async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const reference = readFileSync(songFile, 'utf8');
    const { url } = await startEngine({ look: DARK });
    expect((await page.request.delete(`${url}api/songs/chorus-build/cues/43`)).status()).toBe(204);
    await page.goto(`${url}#builder`);

    await pad(page, 43).click();
    await page.getByLabel('Type').selectOption('pixel');
    await page.getByLabel('Name').fill('chorus-build');
    await page.getByLabel('Release').fill('1s');

    // A warm-white wash on the left half, under everything.
    await dropIngredient(page, 'Wash', [0.25, 0.5]);
    await resize(page, 'bottom-left', [0, 0]);
    await resize(page, 'top-right', [0.5, 1]);
    await page.getByRole('button', { name: 'Colour: warm-white' }).click();
    await page.getByLabel('Blend').selectOption('max');

    // An amber chase on the right half, covering what is below.
    await dropIngredient(page, 'Chase', [0.75, 0.5]);
    await resize(page, 'bottom-left', [0.5, 0]);
    await resize(page, 'top-right', [1, 1]);
    await page.getByRole('button', { name: 'Colour: amber' }).click();

    // A steel-blue sparkle added across the top row, the inner washes left alone.
    await dropIngredient(page, 'Sparkle', [0.5, 0.8]);
    await resize(page, 'bottom-left', [0, 0.65]);
    await resize(page, 'top-right', [1, 1]);
    await page.getByRole('button', { name: 'Colour: steel-blue' }).click();
    await page.getByLabel('Blend').selectOption('add');
    await expect(page.locator('circle.caught')).toHaveCount(16);
    await page.getByRole('button', { name: 'Mute inner-wash' }).click();
    await expect(page.locator('[data-crossed-out]')).toHaveCount(2);
    await expect(page.locator('[data-fixture="high-2"]')).toHaveAttribute('data-crossed-out');

    await expect(
      page.getByText('Warm-white wash on the left, amber chase on the right, steel-blue sparkle across the top except inner-wash.'),
    ).toBeVisible();
    await page.getByRole('button', { name: 'Save cue' }).click();
    await expect(page.getByText('Saved cue 43 in chorus-build.')).toBeVisible();
    expect(readFileSync(songFile, 'utf8')).toBe(reference);
    expect(replayHeld(show, 43)).toBe(readFileSync(path.join(ROOT, 'tests/fixtures/dumps/pixel/layers/three-rows.dump'), 'utf8'));
  });

  test('a click selects the topmost area, and another, even unsteady, the one beneath', async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const saved = readFileSync(songFile, 'utf8');
    const { url } = await startEngine({ look: DARK });
    await page.goto(`${url}#builder`);
    await pad(page, 43).click();
    const selected = page.locator('.area.selected');

    const point = await stagePoint(page, [0.25, 0.8]);
    await page.mouse.click(point.x, point.y);
    await expect(selected).toHaveAttribute('data-area', '2');
    await unsteadyClick(page, point);
    await expect(selected).toHaveAttribute('data-area', '0');
    await page.getByRole('button', { name: 'Save cue' }).click();
    await expect(page.getByText('Saved cue 43 in chorus-build.')).toBeVisible();
    expect(readFileSync(songFile, 'utf8'), 'no area moved').toBe(saved);

    // An area moved off every fixture says it catches nothing.
    await resize(page, 'top-right', [0.05, 0.05]);
    await expect(page.getByText('This area catches no pixels: move it over some fixtures.')).toBeVisible();
    await expect(page.locator('circle.caught')).toHaveCount(0);
  });

  test('a cancelled press adds no area and puts a resized one back', async ({ page, startEngine }) => {
    const { url } = await startEngine({ look: DARK });
    await page.goto(`${url}#builder`);
    await pad(page, 43).click();
    await page.evaluate(() => addEventListener('pointerdown', (event) => (window.lastPointer = event.pointerId), true));
    const cancelPress = async () => {
      await page.evaluate(() => dispatchEvent(new PointerEvent('pointercancel', { pointerId: window.lastPointer })));
      await page.mouse.up();
    };

    const wash = await page.getByRole('group', { name: 'Ingredients' }).getByRole('button', { name: 'Wash' }).boundingBox();
    const drop = await stagePoint(page, [0.5, 0.5]);
    await page.mouse.move(wash.x + wash.width / 2, wash.y + wash.height / 2);
    await page.mouse.down();
    await page.mouse.move(drop.x, drop.y, { steps: 5 });
    await cancelPress();
    await expect(page.locator('[data-area]')).toHaveCount(3);

    const chase = await stagePoint(page, [0.75, 0.3]);
    await page.mouse.click(chase.x, chase.y);
    const handle = centre(await page.locator('[data-handle="top-right"]').boundingBox());
    const inward = await stagePoint(page, [0.8, 0.6]);
    await page.mouse.move(handle.x, handle.y);
    await page.mouse.down();
    await page.mouse.move(inward.x, inward.y, { steps: 5 });
    await expect(page.getByLabel('Area width')).toHaveValue('0.3');
    await cancelPress();
    await expect(page.getByLabel('Area width')).toHaveValue('0.5');
    await expect(page.getByLabel('Area height')).toHaveValue('1');
  });

  test('the inspector keeps an area on the stage, sets its phase and lists every mute', async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const { url } = await startEngine({ look: DARK });
    await page.goto(`${url}#builder`);
    await pad(page, 43).click();

    await page.getByRole('button', { name: 'amber chase on the right', exact: true }).click();
    await page.getByLabel('Area width').fill('1.5');
    await page.getByLabel('Area width').press('Enter');
    await expect(page.getByLabel('Area width'), 'the rest of the stage from x 0.5').toHaveValue('0.5');
    await page.getByLabel('Phase').fill('0.5');

    // The sparkle squeezed onto high-1 no longer catches the inner washes it mutes: still listed.
    await page.getByRole('button', { name: /^steel-blue sparkle/ }).click();
    await page.getByLabel('Area width').fill('0.2');
    await page.getByLabel('Area width').press('Enter');
    const muteInner = page.getByRole('button', { name: 'Mute inner-wash' });
    await expect(muteInner).toHaveAttribute('aria-pressed', 'true');
    await muteInner.click();
    await expect(page.getByRole('button', { name: 'Mute inner-wash' })).toHaveCount(0);

    await page.getByRole('button', { name: 'Save cue' }).click();
    await expect(page.getByText('Saved cue 43 in chorus-build.')).toBeVisible();
    const written = readFileSync(songFile, 'utf8');
    expect(written).toContain('        params: { color: amber, width: 0.25, speed: 1 bar, phase: 0.5 }\n');
    expect(written).toContain('        area: { x: 0, y: 0.65, width: 0.2, height: 0.35 }\n');
    expect(written).not.toContain('mute:');
  });

  test('with the engine gone, an area still opens read-only', async ({ page, startEngine }) => {
    const engine = await startEngine({ look: DARK });
    await page.goto(`${engine.url}#builder`);
    await pad(page, 43).click();
    await engine.stop();
    await expect(page.locator('.connection')).toHaveAttribute('data-connected', 'false');

    const chase = await stagePoint(page, [0.75, 0.3]);
    await page.mouse.click(chase.x, chase.y);
    await expect(page.locator('.area.selected')).toHaveAttribute('data-area', '1');
    await expect(page.getByLabel('Blend')).toHaveValue('over');
    await expect(page.getByLabel('Blend')).toBeDisabled();
    await expect(page.locator('[data-handle]')).toHaveCount(0);
  });

  test('a hidden area leaves the preview but stays in the saved cue', async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const saved = readFileSync(songFile, 'utf8');
    const { url } = await startEngine({ look: DARK });
    await page.goto(`${url}#builder`);
    await pad(page, 43).click();
    const lowLeft = page.locator('[data-fixture="low-1"] circle').first();
    await expect(lowLeft).toHaveAttribute('fill', '#ffd2a0');

    await page.getByRole('button', { name: 'Hide while editing: warm-white wash on the left' }).click();
    await expect(lowLeft, 'no layer left draws it: the dark look below shows').toHaveAttribute('fill', '#000000');
    await page.getByRole('button', { name: 'Save cue' }).click();

    await expect(page.getByText('Saved cue 43 in chorus-build.')).toBeVisible();
    expect(readFileSync(songFile, 'utf8')).toBe(saved);
  });
});

test('an old-form pixel cue opened and saved untouched saves as layers and plays the same', async ({ page, show, startEngine }) => {
  mkdirSync(path.join(show, 'songs'));
  const songFile = path.join(show, 'songs/demo.yaml');
  writeFileSync(
    songFile,
    'program: 1\nbpm: 150\ncues:\n  40:\n    name: bridge\n    type: pixel\n    effect: chase\n' +
      '    params: { color: deep-blue, width: 0.25, speed: 1 beat }\n' +
      '    layer_2: { shape: circle, effect: pulse, blend: add, params: { color: "#ff0000", width: 0.2 } }\n    seed: 7\n',
  );
  const before = replayHeld(show, 40);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await expect(page.getByText('Deep-blue chase everywhere, #ff0000 breathe everywhere.')).toBeVisible();
  // Selecting a whole-stage area with an unsteady click doesn't give it an area of its own.
  await unsteadyClick(page, await stagePoint(page, [0.5, 0.5]));
  await expect(page.locator('.area.selected')).toHaveAttribute('data-area', '1');
  await page.getByRole('button', { name: 'Save cue' }).click();

  await expect(page.getByText('Saved cue 40 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    'program: 1\nbpm: 150\ncues:\n  40:\n    name: bridge\n    type: pixel\n    seed: 7\n    layers:\n' +
      '      -\n        effect: chase\n        params: { color: deep-blue, width: 0.25, speed: 1 beat }\n' +
      '      -\n        shape: circle\n        effect: pulse\n        params: { color: "#ff0000", width: 0.2 }\n        blend: add\n',
  );
  expect(replayHeld(show, 40)).toBe(before);
});
