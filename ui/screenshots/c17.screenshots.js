import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

// The rig with height that plan §16's example look is built for, with that look on note 43.
test.use({ showSource: 'tests/fixtures/shows/three-rows' });

const shoot = (page, state) => capture(page, 'c17', state);
const pad = (page, note) => page.locator(`[data-pad="${note}"]`);

async function stagePoint(page, [x, y]) {
  const stage = await page.getByRole('img', { name: 'Front view' }).locator('rect.stage').boundingBox();
  return { x: stage.x + x * stage.width, y: stage.y + (1 - y) * stage.height };
}

test('C17 stage composer', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);

  await pad(page, 43).click();
  await expect(page.getByText('Warm-white wash on the left, amber chase on the right')).toBeVisible();
  await expect(page.locator('[data-fixture="low-1"] circle').first()).not.toHaveAttribute('fill', '#000000');
  await shoot(page, 'composer-example-look');

  const chase = await stagePoint(page, [0.75, 0.3]);
  await page.mouse.click(chase.x, chase.y);
  await expect(page.locator('circle.caught')).not.toHaveCount(0);
  await shoot(page, 'area-selected-caught-pixels');

  await page.getByRole('button', { name: /steel-blue sparkle/ }).first().click();
  await expect(page.locator('[data-crossed-out]')).toHaveCount(2);
  await shoot(page, 'mute-toggles-crossed-out');

  await pad(page, 44).click();
  await page.getByLabel('Type').selectOption('pixel');
  const sweep = await page.getByRole('group', { name: 'Ingredients' }).getByRole('button', { name: 'Sweep' }).boundingBox();
  await page.mouse.move(sweep.x + sweep.width / 2, sweep.y + sweep.height / 2);
  await page.mouse.down();
  const drop = await stagePoint(page, [0.5, 0.5]);
  await page.mouse.move(drop.x, drop.y, { steps: 5 });
  await page.waitForTimeout(500);
  await shoot(page, 'ingredient-tray');
  await page.mouse.up();
});
