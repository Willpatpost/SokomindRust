import { expect, test } from '@playwright/test';

const MOVE_HINT = 'Arrow keys or WASD to move. Z to undo.';
const FIRST_BOARD = '5 by 5 puzzle, 1 boxes. 0 moves, 0 pushes. Use arrow keys or WASD.';
// "First Steps": the robot stands directly above the box, whose goal is one
// cell below it, so a single Down push solves the puzzle.
const SOLVED = 'Solved in 1 moves and 1 pushes.';

async function loaded(page: import('@playwright/test').Page) {
  await page.goto('/');
  await expect(page.locator('#title')).toHaveText('First Steps');
}

test('renders the first catalog puzzle', async ({ page }) => {
  await loaded(page);
  await expect(page.locator('#difficulty')).toHaveText('tutorial');
  await expect(page.locator('#message')).toHaveText(MOVE_HINT);
  await expect(page.locator('#board')).toHaveAttribute('aria-label', FIRST_BOARD);
  await expect(page.locator('#solve')).toBeEnabled();
  await expect(page.locator('#undo')).toBeDisabled();
});

test('solves the tutorial with one keyboard push', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#message')).toHaveText(SOLVED);
  await expect(page.locator('#moves')).toHaveText('1');
  await expect(page.locator('#pushes')).toHaveText('1');
  await expect(page.locator('#board')).toHaveAttribute(
    'aria-label',
    '5 by 5 puzzle, 1 boxes. 1 moves, 1 pushes. Solved. Use arrow keys or WASD.',
  );
  await expect(page.locator('#solve')).toBeDisabled();
  await expect(page.locator('#undo')).toBeEnabled();
});

test('undo restores the previous position', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#moves')).toHaveText('1');
  await page.click('#undo');
  await expect(page.locator('#moves')).toHaveText('0');
  await expect(page.locator('#pushes')).toHaveText('0');
  await expect(page.locator('#message')).toHaveText(MOVE_HINT);
  await expect(page.locator('#undo')).toBeDisabled();
});

test('finds a route through the in-browser engine', async ({ page }) => {
  await loaded(page);
  await page.click('#solve');
  await expect(page.locator('#search-status')).toContainText('1 remaining moves');
  await expect(page.locator('#search-status')).toContainText('Search complete.');
  await expect(page.locator('#play')).toBeEnabled();
  await expect(page.locator('#copy')).toBeEnabled();
  await expect(page.locator('#expanded')).not.toHaveText('—');
});

// "The Detour" has a 24-move optimum (a move BFS agrees). The worker decodes
// the WASM metrics tuple, so a misread proof kind would change this label.
test('Optimal mode proves the route move-optimal in the browser', async ({ page }) => {
  await loaded(page);
  await page.selectOption('#puzzles', 'beginner-detour');
  await expect(page.locator('#title')).toHaveText('The Detour');
  await page.selectOption('#mode', 'optimal');
  await page.click('#solve');
  await expect(page.locator('#search-status'))
    .toHaveText('24 remaining moves · proven move-optimal from this position. Search complete.');
  await expect(page.locator('#play')).toBeEnabled();
});

test('restores the solved session after a reload', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#message')).toHaveText(SOLVED);
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem('sokomind-rust.v1.session')))
    .toContain('"actions":"D"');
  await page.reload();
  await expect(page.locator('#message')).toHaveText('Session restored by replaying your moves.');
  await expect(page.locator('#moves')).toHaveText('1');
  await expect(page.locator('#best-score')).toHaveText('Best: 1 moves · 1 pushes');
});
