import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c8', state);

test('C8 swell editor and sequencer grid', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);
  await page.locator('.bar select').selectOption('tell-me');
  const edit = (note) => page.locator(`tr[data-note="${note}"]`).getByRole('button', { name: 'Edit' }).click();
  // A scene underneath, as in the song, so the swell and the pattern show on the washes.
  await page.locator('tr[data-note="36"]').getByRole('button', { name: 'Audition' }).click();

  await edit(39);
  await expect(page.locator('.curve polyline')).toBeVisible();
  await shoot(page, 'swell-editor');

  await page.getByRole('button', { name: 'Audition loop' }).click();
  await page.waitForTimeout(3000);
  await shoot(page, 'swell-auditioning');
  await page.getByRole('button', { name: 'Stop audition' }).click();

  await page.getByLabel('Curve', { exact: true }).selectOption('custom');
  await page.locator('.curve').scrollIntoViewIfNeeded();
  const point = await page.locator('.curve .point[data-point="3"]').boundingBox();
  await page.mouse.move(point.x + point.width / 2, point.y + point.height / 2);
  await page.mouse.down();
  await page.mouse.move(point.x + point.width / 2, point.y - 60, { steps: 5 });
  await page.mouse.up();
  await shoot(page, 'swell-custom-points');

  await edit(42);
  await page.getByRole('button', { name: 'at 2', exact: true }).click();
  await shoot(page, 'pattern-grid');

  await page.getByRole('button', { name: 'Audition loop' }).click();
  await expect(page.locator('.lane .playhead')).toHaveCount(1);
  await page.waitForTimeout(700);
  await shoot(page, 'pattern-auditioning');
  await page.getByRole('button', { name: 'Stop audition' }).click();

  await page.getByRole('button', { name: 'New cue' }).click();
  await page.getByLabel('Note').fill('44');
  await page.getByLabel('Name').fill('outro-pulse');
  await page.getByLabel('Type').selectOption('pattern');
  await page.getByLabel('Bars').selectOption('2');
  await page.getByLabel('Drives').selectOption('colour');
  for (const at of [1, 1.75, 2.5, 3, 4.25]) await page.getByRole('button', { name: `at ${at}`, exact: true }).click();
  await page.getByLabel('Hit shape').selectOption('sine');
  await page.getByRole('button', { name: 'Copy', exact: true }).click();
  await shoot(page, 'new-two-bar-pattern');
});
