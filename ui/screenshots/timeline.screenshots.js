import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c21', state);
const pad = (page, number) => page.locator(`[data-pad="${number}"]`);
const block = (page, scene) => page.locator(`.section-editor [data-block="${scene}"]`);
/** The engine-rendered thumbnail of a scene pad, lit. */
const lit = (page, number) => expect(pad(page, number).locator('circle').first()).not.toHaveAttribute('fill', '#000000');

// On the config's scratch copy of tests/fixtures/shows/demo: demo-song has no arrangement yet.
test('Demo show: an empty timeline, then a first section', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);
  await lit(page, 36);
  await page.getByRole('button', { name: 'Timeline' }).click();
  await expect(page.getByText('Create a section, then drag scenes onto it.')).toBeVisible();
  await shoot(page, 'empty-timeline');

  await page.getByLabel('New section name').fill('intro');
  await page.getByRole('button', { name: 'New section' }).click();
  await lit(page, 36);
  await shoot(page, 'new-section');
});

test.describe('on the arranged fixture', () => {
  test.use({ showSource: 'tests/fixtures/shows/arranged' });
  const FIXTURE_DARK = ['--set', 'both', 'dimmer=0%'];

  test('sections, arrangement and overview', async ({ page, startEngine }) => {
    const { url } = await startEngine({ look: FIXTURE_DARK });
    await page.goto(`${url}#builder/arranged/timeline`);
    await lit(page, 36);
    await shoot(page, 'verse-open');

    await block(page, 37).click();
    await expect(page.getByLabel('Level')).toHaveValue('50');
    await shoot(page, 'block-selected');

    await block(page, 36).hover();
    await page.mouse.down();
    await expect(block(page, 36)).toHaveClass(/playing/);
    await shoot(page, 'block-held');
    await page.mouse.up();

    await page.locator('[data-placement="waltz"]').first().click();
    await expect(page.getByLabel('Name', { exact: true })).toHaveValue('waltz');
    await lit(page, 36);
    await shoot(page, 'waltz-open');

    await page.getByLabel('Bars', { exact: true }).fill('1');
    await page.getByLabel('Bars', { exact: true }).press('Tab');
    await expect(page.getByRole('alert')).toBeVisible();
    await shoot(page, 'refused-edit');
  });

  test('show mode is read-only', async ({ page, startEngine }) => {
    const { url } = await startEngine({ look: FIXTURE_DARK, edit: false });
    await page.goto(`${url}#builder/arranged/timeline`);
    await lit(page, 36);
    await shoot(page, 'show-mode');
  });
});
