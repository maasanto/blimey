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

/** The dump `blimey replay` renders of `show`'s saved files for `[ms, message]` events. */
function replay(show, events) {
  const midi = path.join(show, 'fire.mid');
  writeFileSync(midi, midiFile(events));
  return execFileSync(BINARY, ['replay', show, '--midi', midi], { encoding: 'utf8' });
}

/** A note-off for a note no cue uses, so a replay runs to that frame and nothing changes. */
const runTo = (ms) => [ms, [0x81, 0, 0]];

/**
 * The dump of `show`'s song on program 1 selected and `notes` fired at 0 ms, held to the frame
 * at `lastFrameMs`: by default the last of a bar at 150 BPM.
 */
const replayHeld = (show, notes, lastFrameMs = LAST_FRAME_MS) =>
  replay(show, [[0, [0xc1, 1]], ...[notes].flat().map((note) => [0, [0x91, note, 127]]), runTo(lastFrameMs)]);

/** A dump's frames as `[ms, 512 channel values]`. */
const framesOf = (dump) =>
  dump
    .split('\n')
    .filter((line) => /^\d/.test(line))
    .map((line) => {
      const [ms, ...channels] = line.split(' ');
      const values = new Array(512).fill(0);
      for (const channel of channels) {
        const [number, value] = channel.split('=').map(Number);
        values[number - 1] = value;
      }
      return [Number(ms), values];
    });

const ingredient = (page, name) => page.getByRole('group', { name: 'Ingredients' }).getByRole('button', { name, exact: true });
const step = (page, at) => page.getByRole('button', { name: `at ${at}`, exact: true });

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
async function dropIngredient(page, name, at) {
  await dragTo(page, centre(await ingredient(page, name).boundingBox()), await stagePoint(page, at));
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

  /** The example without its swell, scene 43 of chorus-build, in the scene editor. */
  const openChorusBuild = (page, url) => page.goto(`${url}#builder/chorus-build/43`);

  test("plan §16's example look built on the stage saves C14's reference and plays its dump", async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const reference = readFileSync(songFile, 'utf8');
    const { url } = await startEngine({ look: DARK });
    expect((await page.request.delete(`${url}api/songs/chorus-build/cues/43`)).status()).toBe(204);
    await openChorusBuild(page, url);

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
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Saved scene 43 in chorus-build.')).toBeVisible();
    expect(readFileSync(songFile, 'utf8')).toBe(reference);
    expect(replayHeld(show, 43)).toBe(readFileSync(path.join(ROOT, 'tests/fixtures/dumps/pixel/layers/three-rows.dump'), 'utf8'));
  });

  test('a click selects the topmost area, and another, even unsteady, the one beneath', async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const saved = readFileSync(songFile, 'utf8');
    const { url } = await startEngine({ look: DARK });
    await openChorusBuild(page, url);
    const selected = page.locator('.area.selected');

    const point = await stagePoint(page, [0.25, 0.8]);
    await page.mouse.click(point.x, point.y);
    await expect(selected).toHaveAttribute('data-area', '2');
    await unsteadyClick(page, point);
    await expect(selected).toHaveAttribute('data-area', '0');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Saved scene 43 in chorus-build.')).toBeVisible();
    expect(readFileSync(songFile, 'utf8'), 'no area moved').toBe(saved);

    // An area moved off every fixture says it catches nothing.
    await resize(page, 'top-right', [0.05, 0.05]);
    await expect(page.getByText('This area catches no pixels: move it over some fixtures.')).toBeVisible();
    await expect(page.locator('circle.caught')).toHaveCount(0);
  });

  test('a cancelled press adds no area and puts a resized one back', async ({ page, startEngine }) => {
    const { url } = await startEngine({ look: DARK });
    await openChorusBuild(page, url);
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

  test('the inspector keeps an area on the stage, sets its phase and repeat and lists every mute', async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const { url } = await startEngine({ look: DARK });
    await openChorusBuild(page, url);

    await page.getByRole('button', { name: 'amber chase on the right', exact: true }).click();
    await page.getByText('Position and phase').click();
    await page.getByLabel('Area width').fill('1.5');
    await page.getByLabel('Area width').press('Enter');
    await expect(page.getByLabel('Area width'), 'the rest of the stage from x 0.5').toHaveValue('0.5');
    await page.getByLabel('Phase').fill('0.5');
    await page.getByLabel('Repeat').fill('3');

    // The sparkle squeezed onto high-1 no longer catches the inner washes it mutes: still listed.
    await page.getByRole('button', { name: /^steel-blue sparkle/ }).click();
    await page.getByLabel('Area width').fill('0.2');
    await page.getByLabel('Area width').press('Enter');
    const muteInner = page.getByRole('button', { name: 'Mute inner-wash' });
    await expect(muteInner).toHaveAttribute('aria-pressed', 'true');
    await muteInner.click();
    await expect(page.getByRole('button', { name: 'Mute inner-wash' })).toHaveCount(0);

    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Saved scene 43 in chorus-build.')).toBeVisible();
    const written = readFileSync(songFile, 'utf8');
    expect(written).toContain('        params: { color: amber, width: 0.25, repeat: 3, speed: 1 bar, phase: 0.5 }\n');
    expect(written).toContain('        area: { x: 0, y: 0.65, width: 0.2, height: 0.35 }\n');
    expect(written).not.toContain('mute:');
  });

  test('from the keyboard, arrows nudge the selected area, Ctrl+S saves and Backspace deletes it', async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const { url } = await startEngine({ look: DARK });
    await openChorusBuild(page, url);

    // Selected from the list, the focus is on its button: the keys are the button's, not the area's.
    await page.getByRole('button', { name: 'amber chase on the right', exact: true }).click();
    await page.keyboard.press('Backspace');
    await page.keyboard.press('ArrowLeft');
    await expect(page.locator('[data-area]')).toHaveCount(3);

    const chase = await stagePoint(page, [0.75, 0.3]);
    await page.mouse.click(chase.x, chase.y);
    await page.keyboard.press('Alt+ArrowLeft');
    await page.keyboard.press('ArrowLeft');
    await page.keyboard.press('Shift+ArrowUp');
    await page.getByText('Position and phase').click();
    await expect(page.getByLabel('Area left'), 'one plain arrow, the browser shortcut left alone').toHaveValue('0.475');
    await expect(page.getByLabel('Area bottom'), 'a full-height area has nowhere to go').toHaveValue('0');
    await expect(page.locator('.status')).toHaveText('Unsaved changes');

    // Behind the leave prompt the keys are the prompt's: Escape closes it, nothing is deleted.
    await page.getByRole('link', { name: '← Scenes' }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.keyboard.press('Backspace');
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toBeHidden();
    await expect(page.locator('[data-area]')).toHaveCount(3);

    await page.mouse.click(chase.x, chase.y);
    await page.keyboard.press('Control+s');
    await expect(page.getByText('Saved scene 43 in chorus-build.')).toBeVisible();
    await expect(page.getByText('Saved', { exact: true })).toBeVisible();
    expect(readFileSync(songFile, 'utf8')).toContain('        area: { x: 0.475, y: 0, width: 0.5, height: 1 }\n');

    await page.keyboard.press('Backspace');
    await expect(page.locator('[data-area]')).toHaveCount(2);
    // Typing in a field is left alone.
    await page.getByLabel('Name').press('End');
    await page.getByLabel('Name').press('Backspace');
    await expect(page.locator('[data-area]')).toHaveCount(2);
    await expect(page.getByLabel('Name')).toHaveValue('chorus-buil');
  });

  test('with the engine gone, an area still opens read-only', async ({ page, startEngine }) => {
    const engine = await startEngine({ look: DARK });
    await openChorusBuild(page, engine.url);
    await expect(page.getByText('Warm-white wash on the left, amber chase on the right')).toBeVisible();
    await engine.stop();
    await expect(page.locator('.connection')).toHaveAttribute('data-connected', 'false');

    const chase = await stagePoint(page, [0.75, 0.3]);
    await page.mouse.click(chase.x, chase.y);
    await expect(page.locator('.area.selected')).toHaveAttribute('data-area', '1');
    await expect(page.getByLabel('Blend')).toHaveValue('over');
    await expect(page.getByLabel('Blend')).toBeDisabled();
    await expect(page.locator('[data-handle]')).toHaveCount(0);
  });

  test("a swell given to one area leaves the other layers' YAML untouched and plays C15's reference", async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const withoutSwell = readFileSync(songFile, 'utf8');
    const { url } = await startEngine({ look: DARK });
    await openChorusBuild(page, url);

    await page.getByRole('button', { name: 'warm-white wash on the left', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Rhythm: steady' })).toHaveAttribute('aria-pressed', 'true');
    await page.getByRole('button', { name: 'Rhythm: swell' }).click();
    await expect(page.getByLabel('Curve', { exact: true })).toHaveValue('ease-in');
    await page.getByRole('button', { name: 'Save', exact: true }).click();

    await expect(page.getByText('Saved scene 43 in chorus-build.')).toBeVisible();
    const warmWhite = '        params: { color: warm-white }\n';
    expect(readFileSync(songFile, 'utf8')).toBe(
      withoutSwell.replace(warmWhite, `${warmWhite}        swell: { duration: 4 bars, curve: ease-in }\n`),
    );
    // Held its 4 bars and 8 frames, as C15's reference of the same cue.
    expect(replayHeld(show, 43, (4 * 64 + 7) * 25)).toBe(
      readFileSync(path.join(ROOT, 'tests/fixtures/dumps/pixel/layers/three-rows-swell.dump'), 'utf8'),
    );
  });

  test('a hidden area leaves the preview but stays in the saved cue', async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/chorus-build.yaml');
    const saved = readFileSync(songFile, 'utf8');
    const { url } = await startEngine({ look: DARK });
    await openChorusBuild(page, url);
    const lowLeft = page.locator('[data-fixture="low-1"] circle').first();
    await expect(lowLeft).toHaveAttribute('fill', '#ffd2a0');

    await page.getByRole('button', { name: 'Hide while editing: warm-white wash on the left' }).click();
    await expect(lowLeft, 'no layer left draws it: the dark look below shows').toHaveAttribute('fill', '#000000');
    await page.getByRole('button', { name: 'Save', exact: true }).click();

    await expect(page.getByText('Saved scene 43 in chorus-build.')).toBeVisible();
    expect(readFileSync(songFile, 'utf8')).toBe(saved);
  });

  test("each area shows its layer's shape moving between the fixtures, and a hidden one shows none", async ({ page, startEngine }) => {
    const { url } = await startEngine({ look: DARK });
    await openChorusBuild(page, url);
    const chase = page.locator('[data-shape="1"] canvas');
    await expect(chase).toHaveAttribute('width', '64');
    /** The columns of the 64 × 64 picture's top row the amber bar lights, as "24,25,26". */
    const litColumns = () =>
      chase.evaluate((canvas) => {
        const { data } = canvas.getContext('2d').getImageData(0, 0, canvas.width, 1);
        return [...Array(canvas.width).keys()].filter((column) => data[column * 4] > 0).join();
      });

    await expect.poll(litColumns, 'a bar a quarter of the area wide, not a fill').toMatch(/^\d+(,\d+){1,17}$/);
    const before = await litColumns();
    await expect.poll(litColumns, 'the chase steps it along').not.toBe(before);

    await page.getByRole('button', { name: 'Hide while editing: amber chase on the right' }).click();
    await expect(chase).toHaveCount(0);
    await expect(page.locator('[data-shape]')).toHaveCount(2);
  });

  test("an area's shape steps in time with the lights it previews on", async ({ page, show, startEngine }) => {
    // Four steps of 2.7 s at 90 BPM, one per column of fixtures, opened a while after the page:
    // a shape looping on the page's clock, or at another tempo, lights another column.
    writeFileSync(
      path.join(show, 'songs/slow-chase.yaml'),
      'program: 9\nbpm: 90\ncues:\n  50:\n    name: slow-chase\n    type: pixel\n    layers:\n' +
        '      - { shape: bar, effect: chase, params: { color: amber, width: 0.25, speed: 4 bars } }\n',
    );
    const { url } = await startEngine({ look: DARK });
    await page.goto(`${url}#builder`);
    await page.waitForTimeout(1300);
    await page.goto(`${url}#builder/slow-chase/50`);
    await expect(page.locator('[data-shape="0"] canvas')).toHaveAttribute('width', '64');

    /** The quarter of the stage the shape lights, and the column of fixtures lit, as "2/2". */
    const litQuarters = () =>
      page.evaluate(() => {
        const canvas = document.querySelector('[data-shape="0"] canvas');
        const { data } = canvas.getContext('2d').getImageData(0, 0, canvas.width, 1);
        const shape = [...Array(canvas.width).keys()].find((column) => data[column * 4] > 0);
        const isLit = (column) => document.querySelector(`[data-fixture="low-${column}"] circle`).getAttribute('fill') !== '#000000';
        const lights = [1, 2, 3, 4].find(isLit);
        return `${Math.floor((shape * 4) / canvas.width) + 1}/${lights}`;
      });

    await expect.poll(litQuarters, 'the preview reaches the lights').toMatch(/\/\d$/);
    for (let sample = 0; sample < 8; sample += 1) {
      expect(await litQuarters()).toMatch(/^(\d)\/\1$/);
      await page.waitForTimeout(700);
    }
  });

  test("an area's hits light with the lights, whose loop waits for a bar line", async ({ page, show, startEngine }) => {
    // Two beats dark, two lit, in 4 s bars counted from engine start. Opened mid-bar, the lights
    // show the wash as it is until the next bar line, then loop: a picture looping from the
    // preview's start is out of step, and one looping from the bar line is dark while they wait.
    writeFileSync(
      path.join(show, 'songs/slow-hits.yaml'),
      'program: 9\nbpm: 60\ncues:\n  50:\n    name: slow-hits\n    type: pixel\n    layers:\n' +
        '      - { shape: fill, params: { color: amber }, pattern: { length: 1 bar, hits: [{ at: 3, shape: hold, length: 2 beats, level: 100% }] } }\n',
    );
    const { address, url } = await startEngine({ look: DARK });
    await page.goto(`${url}#builder`);
    // Opens the scene a beat into a bar, so the lights wait three beats for the next one.
    await page.evaluate(
      (address) =>
        new Promise((resolve) => {
          const socket = new WebSocket(`ws://${address}/ws`);
          socket.onmessage = (event) => {
            const { type, at_ms: at } = JSON.parse(event.data);
            if (type !== 'frame' || at % 4000 < 1000 || at % 4000 > 1200) return;
            socket.close();
            location.hash = '#builder/slow-hits/50';
            resolve();
          };
        }),
      address,
    );
    await expect(page.locator('[data-shape="0"] canvas')).toHaveAttribute('width', '64');

    /** Over 8 s, the share of animation frames where the area and the fixtures disagree on being lit. */
    const outOfStep = await page.evaluate(
      () =>
        new Promise((resolve) => {
          let [samples, disagreements] = [0, 0];
          const end = performance.now() + 8000;
          requestAnimationFrame(function sample(time) {
            const shape = document.querySelector('[data-shape="0"]');
            const isPainted = shape.querySelector('canvas').getContext('2d').getImageData(32, 32, 1, 1).data[0] > 0;
            const isAreaLit = isPainted && Number(shape.getAttribute('opacity')) > 0.5;
            const isLightLit = document.querySelector('[data-fixture="low-1"] circle').getAttribute('fill') !== '#000000';
            samples += 1;
            disagreements += Number(isAreaLit !== isLightLit);
            if (time < end) requestAnimationFrame(sample);
            else resolve(disagreements / samples);
          });
        }),
    );
    // Only the frames around each switch may disagree, while the stream catches up.
    expect(outOfStep).toBeLessThan(0.1);
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
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText('Saved scene 40 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toBe(
    'program: 1\nbpm: 150\ncues:\n  40:\n    name: bridge\n    type: pixel\n    seed: 7\n    layers:\n' +
      '      -\n        effect: chase\n        params: { color: deep-blue, width: 0.25, speed: 1 beat }\n' +
      '      -\n        shape: circle\n        effect: pulse\n        params: { color: "#ff0000", width: 0.2 }\n        blend: add\n',
  );
  expect(replayHeld(show, 40)).toBe(before);
});

/** The plan's example song: C5's pattern reference is its `verse-pulse`. */
const DEMO_SONG = readFileSync(path.join(ROOT, 'tests/fixtures/shows/demo/songs/demo-song.yaml'), 'utf8');
const C5_HITS = DEMO_SONG.slice(DEMO_SONG.indexOf('    hits:\n', DEMO_SONG.indexOf('  42:\n')));

test("the 1 / 2 / 3.5 pattern built on one area saves the reference and plays as C15's linked pattern", async ({ page, show, startEngine }) => {
  mkdirSync(path.join(show, 'songs'));
  const songFile = path.join(show, 'songs/demo.yaml');
  const header = 'program: 1\nbpm: 120\n';
  writeFileSync(songFile, header);
  const { url } = await startEngine();
  await page.goto(`${url}#builder/demo/42`);

  await page.getByLabel('Name').fill('verse-pulse');
  await ingredient(page, 'Hits').click();
  await expect(page.getByRole('button', { name: 'Rhythm: on the beat' })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByRole('button', { name: 'Rhythm: heartbeat' }).locator('polyline'), 'drawn from the engine curves').toHaveCount(2);
  await page.getByRole('button', { name: 'Colour: deep-blue' }).click();
  await page.getByRole('button', { name: 'Rhythm: custom' }).click();
  await step(page, 1).click();
  await step(page, 2).click();
  await page.getByLabel('Hit shape').selectOption('sine');
  await page.getByLabel('Hit level').fill('80');
  await step(page, 3.5).click();
  await page.getByLabel('Hit shape').selectOption('ramp-up');
  await page.getByLabel('Hit level').fill('100');
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText('Saved scene 42 in demo.')).toBeVisible();
  const layer = '      -\n        shape: fill\n        params: { color: deep-blue }\n';
  const pattern = `        pattern:\n          length: 1 bar\n${C5_HITS.replace(/^(?=.)/gm, '      ')}`;
  const cue = (layers) => `cues:\n  42:\n    name: verse-pulse\n    type: pixel\n    layers:\n${layers}`;
  expect(readFileSync(songFile, 'utf8')).toBe(header + cue(`${layer}${pattern}        blend: over\n`));

  // The saved loop auditions from the next bar line, the grid's playhead following it.
  await page.getByRole('button', { name: 'Audition loop' }).click();
  await expect(page.locator('.lane .playhead')).toHaveCount(1);
  await page.getByRole('button', { name: 'Stop audition' }).click();
  await expect(page.locator('.lane .playhead')).toHaveCount(0);

  // As C15: the same frames as the area without it plus the same pattern on the cue's intensity.
  const twoBars = 4000;
  const inLayer = replay(show, [[0, [0xc1, 1]], [0, [0x91, 42, 127]], runTo(twoBars)]);
  writeFileSync(
    songFile,
    header + cue(`${layer}        blend: over\n`) +
      `  43:\n    name: pulse\n    type: pattern\n    length: 1 bar\n    target: { verse-pulse: intensity }\n${C5_HITS}`,
  );
  const still = replay(show, [[0, [0xc1, 1]], [0, [0x91, 42, 127]], runTo(twoBars)]);
  const linked = replay(show, [[0, [0xc1, 1]], [0, [0x91, 42, 127]], [0, [0x91, 43, 127]], runTo(twoBars)]);
  expect(inLayer).toBe(linked);
  expect(inLayer, 'the rhythm shows').not.toBe(still);
});

test('a groove written by hand in another key and hit order still reads as that groove', async ({ page, show, startEngine }) => {
  mkdirSync(path.join(show, 'songs'));
  const backbeat =
    '      - shape: fill\n        pattern:\n          hits:\n' +
    '            - { level: 100%, length: 0.5 beats, shape: ramp-down, at: 4 }\n' +
    '            - { at: 2, shape: ramp-down, level: 100%, length: 0.5 beats }\n' +
    '          length: 1 bar\n';
  writeFileSync(path.join(show, 'songs/demo.yaml'), `program: 1\ncues:\n  40:\n    name: back\n    type: pixel\n    layers:\n${backbeat}`);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await page.getByRole('button', { name: 'white wash everywhere', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Rhythm: backbeat' })).toHaveAttribute('aria-pressed', 'true');
});

test('the speed slider runs slow to fast, left to right, and saves the period as before', async ({ page, show, startEngine }) => {
  mkdirSync(path.join(show, 'songs'));
  const songFile = path.join(show, 'songs/demo.yaml');
  writeFileSync(songFile, 'program: 1\ncues:\n  40:\n    name: run\n    type: pixel\n    layers:\n      - { effect: chase, params: { speed: 1 bar } }\n');
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await page.getByRole('button', { name: 'white chase everywhere', exact: true }).click();
  const speed = page.getByLabel('Speed', { exact: true });
  const width = page.getByLabel('Width', { exact: true });
  expect((await speed.boundingBox()).width, 'the speed slider gets a row of its own').toBeGreaterThan((await width.boundingBox()).width);

  await speed.press('ArrowRight');
  await expect(speed).toHaveAttribute('aria-valuetext', '0.75 bars');
  await speed.press('End');
  await expect(speed).toHaveAttribute('aria-valuetext', '0.25 bars');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Saved scene 40 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toContain('params: { speed: 0.25 bars }');
});

test("an area's shape plays at the layer's speed: a slow sparkle twinkles slowly, a fast one quickly", async ({ page, show, startEngine }) => {
  mkdirSync(path.join(show, 'songs'));
  writeFileSync(path.join(show, 'songs/demo.yaml'), 'program: 1\ncues:\n  40:\n    name: twinkle\n    type: pixel\n    layers:\n      - { shape: fill, effect: sparkle, params: { speed: 1 bar } }\n');
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await page.getByRole('button', { name: 'white sparkle everywhere', exact: true }).click();
  await expect(page.locator('[data-shape="0"] canvas')).toHaveAttribute('width', '64');
  /** How many different pictures the area shows over 2 s. */
  const picturesIn2s = () =>
    page.evaluate(async () => {
      const pictures = new Set();
      for (let sample = 0; sample < 40; sample += 1) {
        pictures.add(document.querySelector('[data-shape="0"] canvas').toDataURL());
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      return pictures.size;
    });
  const speed = page.getByLabel('Speed', { exact: true });

  await speed.press('Home');
  await expect(speed).toHaveAttribute('aria-valuetext', '16 bars');
  await expect.poll(picturesIn2s, '16 bars at 120 BPM over 32 frames is 1 s a frame').toBeLessThanOrEqual(3);
  await speed.press('End');
  await expect(speed).toHaveAttribute('aria-valuetext', '0.25 bars');
  await expect.poll(picturesIn2s, 'a 0.5 s cycle shows a new picture every 15.6 ms').toBeGreaterThanOrEqual(5);
});

test("a sparkle's width sizes its particles and its repeat makes them denser", async ({ page, show, startEngine }) => {
  mkdirSync(path.join(show, 'songs'));
  const songFile = path.join(show, 'songs/demo.yaml');
  writeFileSync(songFile, 'program: 1\ncues:\n  40:\n    name: twinkle\n    type: pixel\n    layers:\n      - { shape: fill, effect: sparkle, params: { speed: 1 bar } }\n');
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await page.getByRole('button', { name: 'white sparkle everywhere', exact: true }).click();
  const width = page.getByLabel('Width', { exact: true });
  await expect(width.locator('xpath=following-sibling::output'), 'each pixel its own particle until set').toHaveText('pixel');
  await width.fill('0.25');
  await page.getByLabel('Repeat').fill('4');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Saved scene 40 in demo.')).toBeVisible();
  expect(readFileSync(songFile, 'utf8')).toContain('params: { width: 0.25, repeat: 4, speed: 1 bar }');

  await width.press('Home');
  await expect(width.locator('xpath=following-sibling::output'), 'the slider minimum goes back to a pixel each').toHaveText('pixel');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(() => readFileSync(songFile, 'utf8')).toContain('params: { repeat: 4, speed: 1 bar }');
});

test('dragging the speed slider steadily to the right only ever makes it faster', async ({ page, show, startEngine }) => {
  mkdirSync(path.join(show, 'songs'));
  writeFileSync(path.join(show, 'songs/demo.yaml'), 'program: 1\ncues:\n  40:\n    name: run\n    type: pixel\n    layers:\n      - { effect: chase, params: { speed: 16 bars } }\n');
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);

  await pad(page, 40).click();
  await page.getByRole('button', { name: 'white chase everywhere', exact: true }).click();
  const speed = page.getByLabel('Speed', { exact: true });
  const box = await speed.boundingBox();
  const y = box.y + box.height / 2;
  await page.mouse.move(box.x + 1, y);
  await page.mouse.down();
  const periods = [];
  for (let x = box.x + 1; x < box.x + box.width; x += 2) {
    await page.mouse.move(x, y);
    periods.push(parseFloat(await speed.getAttribute('aria-valuetext')));
  }
  await page.mouse.up();

  const slowerAgain = periods.findIndex((period, index) => index > 0 && period > periods[index - 1]);
  expect(slowerAgain, `the period went back up during the drag: ${periods.join(', ')}`).toBe(-1);
  expect(periods.at(-1)).toBeLessThan(1);
});

test("an area keeps its own saved rhythm to audition after a reorder, and a new one has none", async ({ page, show, startEngine }) => {
  mkdirSync(path.join(show, 'songs'));
  const layers =
    '      - { shape: fill, swell: { duration: 1 bar } }\n' +
    '      - { shape: fill, params: { color: deep-blue }, pattern: { length: 2 bars, hits: [{ at: 1, shape: hold, length: 1 beat, level: 100% }] } }\n';
  writeFileSync(path.join(show, 'songs/demo.yaml'), `program: 1\ncues:\n  40:\n    name: two\n    type: pixel\n    layers:\n${layers}`);
  const { url } = await startEngine();
  await page.goto(`${url}#builder`);
  const audition = page.getByRole('button', { name: 'Audition loop' });

  await pad(page, 40).click();
  await page.getByRole('button', { name: 'deep-blue wash everywhere', exact: true }).click();
  await expect(audition).toBeEnabled();
  await page.getByRole('button', { name: 'Send back: deep-blue wash everywhere' }).click();
  await expect(audition, "the pattern's own saved loop, not the swell now below it").toBeEnabled();

  await ingredient(page, 'Hits').click();
  await expect(audition, 'not saved yet').toBeDisabled();
});

test.describe('on the demo show', () => {
  test.use({ showSource: 'tests/fixtures/shows/demo' });

  test("each scene of demo-song, converted, plays alone as the scene within a DMX step", async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/demo-song.yaml');
    // Program 3 selects demo-song; held past verse-blue's 2 s fade-in, then through its 1 s release.
    const fire = (note) =>
      framesOf(replay(show, [[0, [0xc1, 3]], [0, [0x91, Number(note), 127]], [3000, [0x81, Number(note), 0]], runTo(4500)]));
    const scenes = { 36: 'Deep-blue wash everywhere.', 37: 'Magenta wash everywhere except inner-wash, amber wash everywhere except outer-wash.' };
    const before = Object.fromEntries(Object.keys(scenes).map((note) => [note, fire(note)]));
    const { url } = await startEngine();
    await page.goto(`${url}#builder`);

    for (const [note, sentence] of Object.entries(scenes)) {
      await pad(page, note).click();
      await expect(page.getByText(sentence)).toBeVisible();
      await page.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(page.getByText(`Saved scene ${note} in demo-song.`)).toBeVisible();
      await page.getByRole('link', { name: '← Scenes' }).click();
    }
    expect(readFileSync(songFile, 'utf8')).toContain(
      '  37:\n    name: chorus-amber\n    type: pixel\n    fade_in: 500ms\n    release: 500ms\n    layers:\n' +
        '      -\n        shape: fill\n        params: { color: magenta }\n        blend: over\n        mute: [inner-wash]\n' +
        '      -\n        shape: fill\n        params: { color: amber }\n        blend: over\n        mute: [outer-wash]\n',
    );

    for (const note of Object.keys(scenes)) {
      const after = fire(note);
      expect(after.map(([ms]) => ms), `cue ${note}: frame times`).toEqual(before[note].map(([ms]) => ms));
      const furthest = Math.max(...after.flatMap(([, values], frame) => values.map((value, channel) => Math.abs(value - before[note][frame][1][channel]))));
      expect(furthest, `cue ${note}: largest difference on any channel`).toBeLessThanOrEqual(1);
    }
  });

  test('the blackout, a dimmer over the scenes below, stays a fixed look that can still be auditioned, duplicated and renumbered', async ({ page, show, startEngine }) => {
    const songFile = path.join(show, 'songs/demo-song.yaml');
    const before = readFileSync(songFile, 'utf8');
    const { url } = await startEngine();
    await page.goto(`${url}#builder`);

    await pad(page, 38).click();
    await expect(page.getByRole('status')).toContainText(
      'This fixed look stays as it is, read-only on the stage: it gives wash-1 only a dimmer, which keeps the colour of the scenes below',
    );
    await expect(ingredient(page, 'Wash')).toBeDisabled();
    await page.getByRole('button', { name: 'Audition', exact: true }).click();
    await page.getByRole('button', { name: 'Release', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Audition', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Duplicate', exact: true }).click();
    await expect(page.getByText('Duplicated scene 38 as scene 41.')).toBeVisible();
    await page.getByLabel('Number').fill('50');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Saved scene 50 in demo-song.')).toBeVisible();

    const blackout = before.slice(before.indexOf('  38:\n'), before.indexOf('  39:\n'));
    const after = readFileSync(songFile, 'utf8');
    expect(after, 'moved').not.toContain('  38:\n');
    expect(after, 'still a fixed look').toContain(blackout.replace('  38:', '  50:'));
    expect(after).toContain(blackout.replace('  38:', '  41:').replace('name: blackout', 'name: blackout-copy'));
  });
});

test.describe('on three rows of washes, a scene of nine colours', () => {
  test.use({ showSource: 'tests/fixtures/shows/three-rows' });

  test('needs more areas than a look holds, so it stays a fixed look', async ({ page, show, startEngine }) => {
    const colours = ['#ff0000', '#00ff00', '#0000ff', '#ffff00', '#00ffff', '#ff00ff', '#ffffff', '#808080', '#ff8000'];
    const fixtures = ['low-1', 'low-2', 'low-3', 'low-4', 'mid-1', 'mid-2', 'mid-3', 'mid-4', 'high-1'];
    const values = fixtures.map((fixture, index) => `      ${fixture}: { color: "${colours[index]}" }\n`).join('');
    writeFileSync(path.join(show, 'songs/rainbow.yaml'), `program: 9\ncues:\n  36:\n    name: rainbow\n    type: scene\n    values:\n${values}`);
    const { url } = await startEngine({ look: ['--set', 'all-wash', 'dimmer=0%'] });
    await page.goto(`${url}#builder`);
    await page.getByLabel('Song').selectOption('rainbow');

    await pad(page, 36).click();
    await expect(page.getByRole('status')).toContainText('it needs 9 areas, more than the 8 a look holds');
    await expect(ingredient(page, 'Wash')).toBeDisabled();
  });
});
