import { readFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect } from './engine.js';

const hex = (levels) => '#' + levels.map((level) => Math.min(255, level).toString(16).padStart(2, '0')).join('');

/** Stage-plan fill of every pixel, by fixture. */
const previewColours = (page) =>
  page.$$eval('[data-fixture]', (fixtures) =>
    Object.fromEntries(
      fixtures.map((fixture) => [
        fixture.dataset.fixture,
        [...fixture.querySelectorAll('circle')].map((circle) => circle.getAttribute('fill')),
      ]),
    ),
  );

const waitForFrames = (page) => expect(page.getByTestId('engine-time')).not.toHaveText('—');

test('the front view shows the colours on the DMX output', async ({ page, startEngine }) => {
  const { url } = await startEngine();

  await page.goto(`${url}#monitor`);
  await waitForFrames(page);
  // `fire` selects no song and takes no MIDI.
  await expect(page.getByTestId('song')).toHaveText('None selected');
  await expect(page.getByTestId('master')).toHaveText('100%');
  await expect(page.getByTestId('cues')).toHaveText('None');
  await expect(page.getByTestId('midi-log')).toContainText('No MIDI received');
  const dmx = await page.$$eval('[data-channel]', (cells) => cells.map((cell) => Number(cell.textContent)));
  const rig = await (await page.request.get(`${url}api/show/rig.yaml`)).json();
  await page.getByRole('link', { name: 'Front view' }).click();
  await expect(page.locator('[data-fixture] circle')).toHaveCount(16);
  const preview = await previewColours(page);

  // Wild Wash 16ch: per pixel red, green, blue, white; white shows as added to all three.
  for (const { name, address } of rig.fixtures) {
    const expected = [0, 1, 2, 3].map((pixel) => {
      const [red, green, blue, white] = dmx.slice(address - 1 + 4 * pixel, address + 3 + 4 * pixel);
      return hex([red + white, green + white, blue + white]);
    });
    expect(preview[name], name).toEqual(expected);
  }
  expect(preview['wash-1'][0]).toBe('#ff0000');
  expect(preview['wash-2'][0]).toBe('#000000');
  expect(dmx.slice(100, 104), 'wash-4 uses its white channel').toEqual([115, 38, 0, 38]);
});

const centre = (box) => ({ x: box.x + box.width / 2, y: box.y + box.height / 2 });

test('dragging a fixture moves it in rig.yaml', async ({ page, show, startEngine }) => {
  const { url } = await startEngine();
  const rigFile = path.join(show, 'rig.yaml');
  await page.goto(`${url}#front`);
  const stage = await page.getByRole('img', { name: 'Front view' }).locator('rect.stage').boundingBox();
  const pixel = page.locator('[data-fixture="wash-2"] circle').first();
  const neighbour = page.locator('[data-fixture="wash-1"] circle').first();
  const from = centre(await pixel.boundingBox());

  // Right by a tenth of the stage, and up by three tenths: y grows upwards.
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + 0.1 * stage.width, from.y - 0.3 * stage.height, { steps: 5 });
  await page.mouse.up();

  const wash2Canvas = /- name: wash-2\n(?:.*\n){3}  canvas:\n    x: ([\d.]+)\n    y: ([\d.]+)\n/;
  await expect.poll(() => readFileSync(rigFile, 'utf8')).toMatch(wash2Canvas);
  const saved = readFileSync(rigFile, 'utf8');
  const [, x, y] = saved.match(wash2Canvas).map(Number);
  expect(x).toBeCloseTo(0.325 + 0.1, 2);
  expect(y).toBeCloseTo(0.2 + 0.3, 2);
  expect(saved, 'the header comment is kept').toMatch(/^# Test copy of the Fossile rig/);
  expect(saved, 'other fixtures stay put').toContain('  canvas:\n    x: 0.55\n    y: 0.2\n');

  // The engine sends the saved rig to the open page, and serves it again after a reload.
  const firstPixelX = x + 0.125 / 8;
  for (const reload of [false, true]) {
    if (reload) await page.reload();
    await expect.poll(async () => Number(await pixel.getAttribute('cx'))).toBeCloseTo(firstPixelX * 1600, 1);
    await expect.poll(async () => Number(await pixel.getAttribute('cy'))).toBeCloseTo((1 - y) * 900, 1);
  }
  const [raised, below] = [centre(await pixel.boundingBox()), centre(await neighbour.boundingBox())];
  expect(raised.y, 'wash-2 (y 0.5) is drawn above wash-1 (y 0.2)').toBeLessThan(below.y - 0.25 * stage.height);
});

test('a drag that saves nothing leaves fixtures draggable', async ({ page, startEngine }) => {
  const { url } = await startEngine();
  await page.goto(`${url}#front`);
  const stage = await page.getByRole('img', { name: 'Front view' }).locator('rect.stage').boundingBox();
  // wash-4 is the rightmost fixture: at the edge no other fixture's pixels sit over it.
  const pixel = page.locator('[data-fixture="wash-4"] circle').first();
  const dragRight = async (fraction) => {
    const from = centre(await pixel.boundingBox());
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + fraction * stage.width, from.y, { steps: 5 });
    await page.mouse.up();
  };
  /** wash-4's saved x, as the engine reads rig.yaml back. */
  const wash4X = async () => {
    const rig = await (await page.request.get(`${url}api/show/rig.yaml`)).json();
    return rig.fixtures.find((fixture) => fixture.name === 'wash-4').canvas.x;
  };

  // Against the right edge: x is 1 - width.
  await dragRight(0.9);
  await expect.poll(wash4X).toBe(0.875);
  await expect.poll(async () => Number(await pixel.getAttribute('cx'))).toBeCloseTo((0.875 + 0.125 / 8) * 1600, 1);
  // Further right clamps back to the same x, so nothing is saved.
  await dragRight(0.05);
  await dragRight(-0.1);

  await expect.poll(wash4X).toBe(0.775);
});

test('a body that is not JSON gets a JSON error', async ({ page, startEngine }) => {
  const { url } = await startEngine();

  const response = await page.request.put(`${url}api/show/rig.yaml`, { data: 'not json' });

  expect(response.status()).toBe(422);
  expect((await response.json()).error).toBeTruthy();
});

test('the page reconnects after an engine restart', async ({ page, startEngine }) => {
  const first = await startEngine();
  await page.goto(`${first.url}#monitor`);
  await waitForFrames(page);
  await expect(page.locator('header .connection')).toHaveText('Connected');

  await first.stop();
  await expect(page.getByRole('status')).toContainText('Reconnecting');

  await startEngine({ listen: first.address });
  await expect(page.locator('header .connection')).toHaveText('Connected');
  await expect(page.getByRole('status')).toHaveCount(0);
  await expect(page.getByText(/^[1-9]\d* \/ s$/)).toBeVisible();
});

test('the patch test button flashes a fixture through the engine', async ({ page, startEngine }) => {
  const { url } = await startEngine();
  await page.goto(`${url}#patch`);
  await expect(page.locator('header .mode')).toHaveText('edit mode');

  await page.locator('[data-fixture="wash-2"]').getByRole('button', { name: 'Test' }).click();
  await page.getByRole('link', { name: 'Front view' }).click();

  const wash2 = page.locator('[data-fixture="wash-2"] circle').first();
  await expect(wash2).toHaveAttribute('fill', '#ffffff');
  await expect(wash2).toHaveAttribute('fill', '#000000');
});

test('show mode locks positions and fixture tests', async ({ page, show, startEngine }) => {
  const { url } = await startEngine({ edit: false });
  const before = readFileSync(path.join(show, 'rig.yaml'), 'utf8');
  await page.goto(`${url}#patch`);

  await expect(page.locator('header .mode')).toHaveText('show mode');
  await expect(page.getByRole('button', { name: 'Test' }).first()).toBeDisabled();
  const rig = await (await page.request.get(`${url}api/show/rig.yaml`)).json();
  const refused = await page.request.put(`${url}api/show/rig.yaml`, { data: rig });
  expect(refused.status()).toBe(403);
  for (const [method, endpoint] of [['put', 'show/rig.yaml'], ['put', 'preview/pixel'], ['post', 'midi'], ['post', 'songs']]) {
    const malformed = await page.request[method](`${url}api/${endpoint}`, { data: 'not json' });
    expect(malformed.status(), `${endpoint}: show mode refuses before reading the body`).toBe(403);
  }
  expect(readFileSync(path.join(show, 'rig.yaml'), 'utf8')).toBe(before);
});
