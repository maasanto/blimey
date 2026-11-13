import { test, expect } from '../tests/engine.js';
import { DARK, capture } from './capture.js';

const shoot = (page, state) => capture(page, 'c7', state);

test('C7 builder', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK });
  await page.goto(`${url}#builder`);
  await expect(page.locator('tr[data-note="36"]')).toBeVisible();
  await shoot(page, 'cue-list');

  await page.getByLabel('Fixture or group to add').selectOption('all-wash');
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await page.locator('[data-target="all-wash"]').getByRole('button', { name: 'deep-blue' }).click();
  await page.getByLabel('all-wash dimmer').fill('60');
  await page.getByLabel('Fixture or group to add').selectOption('outer-wash');
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await page.locator('[data-target="outer-wash"]').getByRole('button', { name: 'amber' }).click();
  await expect(page.locator('[data-fixture="wash-1"] circle').first()).not.toHaveAttribute('fill', '#000000');
  await shoot(page, 'look-in-progress');

  await page.getByRole('link', { name: 'Front view' }).click();
  await shoot(page, 'front-view-live');
  await page.goBack();

  await page.getByLabel('Note').fill('36');
  await page.getByLabel('Name').fill('verse-amber');
  await page.getByRole('button', { name: 'Save cue' }).click();
  await expect(page.getByRole('alert')).toContainText('note 36 already fires cue');
  await shoot(page, 'duplicate-note');

  await page.getByLabel('Note').fill('41');
  await page.getByRole('button', { name: 'Save cue' }).click();
  await page.locator('tr[data-note="41"]').getByRole('button', { name: 'Audition' }).click();
  await expect(page.locator('tr[data-note="41"]').getByRole('button', { name: 'Release' })).toBeVisible();
  await shoot(page, 'saved-and-auditioning');

  await page.getByRole('button', { name: 'New song' }).click();
  await page.getByLabel('Song name').fill('encore');
  await page.getByLabel('Tempo').fill('128');
  await shoot(page, 'new-song-form');

  await page.getByRole('button', { name: 'Create song' }).click();
  await expect(page.getByRole('button', { name: 'Undo (2)' })).toBeEnabled();
  await shoot(page, 'undo-steps');
});

test('C7 builder in show mode', async ({ page, startEngine }) => {
  const { url } = await startEngine({ look: DARK, edit: false });
  await page.goto(`${url}#builder`);
  await expect(page.getByText('Show mode: the builder is read-only.')).toBeVisible();
  await shoot(page, 'show-mode-locked');
});
