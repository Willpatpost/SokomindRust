import { expect, test } from '@playwright/test';

const MOVE_HINT = 'Arrow keys or WASD to move. Z to undo.';
const FIRST_BOARD = '5 by 5 puzzle, 1 boxes. 0 moves, 0 pushes. Use arrow keys or WASD to move, Z to undo.';
// "First Steps": the robot stands directly above the box, whose goal is one
// cell below it, so a single Down push solves the puzzle.
const SOLVED = 'Solved in 1 moves and 1 pushes.';

async function loaded(page: import('@playwright/test').Page) {
  // vite preview proxies /api to 127.0.0.1:3000, where the suite runs no API, so
  // the health probe would retry every proxy 500 with backoff. A 404 is a host
  // without the API, which the probe accepts once and never asks again.
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
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
    '5 by 5 puzzle, 1 boxes. 1 moves, 1 pushes. Solved. Use arrow keys or WASD to move, Z to undo.',
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

// Restart takes back every move, where Undo takes back one: Left and Right walk
// the robot off and back, and Down solves the puzzle, so three moves go to none.
test('Restart returns a solved puzzle to its start', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#message')).toHaveText('Solved in 3 moves and 1 pushes.');
  await expect(page.locator('#moves')).toHaveText('3');
  await page.click('#reset');
  await expect(page.locator('#moves')).toHaveText('0');
  await expect(page.locator('#pushes')).toHaveText('0');
  await expect(page.locator('#message')).toHaveText(MOVE_HINT);
  await expect(page.locator('#board')).toHaveAttribute('aria-label', FIRST_BOARD);
  await expect(page.locator('#solve')).toBeEnabled();
  await expect(page.locator('#undo')).toBeDisabled();
});

// The keyboard shortcuts skip keys typed into a form field, so S types an s
// there instead of pushing the robot down.
test('keys typed into a text field stay in it', async ({ page }) => {
  await loaded(page);
  await page.locator('summary', { hasText: 'Import / edit a puzzle' }).click();
  await page.fill('#rows', '');
  await page.locator('#rows').press('s');
  await expect(page.locator('#rows')).toHaveValue('s');
  await expect(page.locator('#moves')).toHaveText('0');
  await expect(page.locator('#message')).toHaveText(MOVE_HINT);
});

test('rows that do not parse report why and leave the current puzzle in play', async ({ page }) => {
  await loaded(page);
  await page.locator('summary', { hasText: 'Import / edit a puzzle' }).click();
  for (const [rows, error] of [
    ['OOOOO\nO   O\nO A O\nO a O\nOOOOO', 'Exactly one robot R is required'],
    ['OOOOO\nO R O\nO A#O\nO a O\nOOOOO', 'Unsupported symbol at row 3, column 4'],
    ['OOOOO\nO R O\nO A O\nO b O\nOOOOO', 'Each box label must have the same number of matching goals'],
  ]) {
    await page.fill('#rows', rows);
    await page.click('#load-custom');
    await expect(page.locator('#message')).toHaveText(error);
    await expect(page.locator('#title')).toHaveText('First Steps');
  }
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#message')).toHaveText(SOLVED);
});

// Apply route replays from the puzzle's start and changes nothing unless the
// whole route is valid, so a route that goes wrong after a legal push leaves 0 moves.
test('a route that does not replay reports why and keeps the position', async ({ page }) => {
  await loaded(page);
  await page.locator('summary', { hasText: 'Import a route' }).click();
  for (const [route, error] of [
    ['', 'Route is empty.'],
    ['dx', 'Routes must use only U/D/L/R'],
    ['u', 'Blocked action at index 0'],
    ['d d', 'Blocked action at index 1'],
  ]) {
    await page.fill('#route-input', route);
    await page.click('#load-route');
    await expect(page.locator('#message')).toHaveText(error);
    await expect(page.locator('#moves')).toHaveText('0');
  }
  await page.fill('#route-input', 'd');
  await page.click('#load-route');
  await expect(page.locator('#message')).toHaveText(SOLVED);
  await expect(page.locator('#moves')).toHaveText('1');
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

// Quality finds no route on "Grand Hall" and searches it for hundreds of milliseconds
// before a limit ends it, far longer than the gap between the two clicks, so Stop
// always lands while the search runs and no route is on show.
test('Stop ends a browser search and frees the controls', async ({ page }) => {
  await loaded(page);
  await page.selectOption('#puzzles', 'huge');
  await expect(page.locator('#title')).toHaveText('Grand Hall');
  await page.selectOption('#mode', 'quality');
  await page.selectOption('#seconds', '30');
  await page.click('#solve');
  await page.click('#cancel');
  await expect(page.locator('#search-status')).toHaveText('Stopped.');
  await expect(page.locator('#cancel')).toBeDisabled();
  await expect(page.locator('#solve')).toBeEnabled();
});

// "The Detour" has a 24-move optimum (a move BFS agrees). The worker decodes
// the WASM metrics tuple, so a misread proof kind would change this label.
test('Optimal mode proves the route move-optimal in the browser', async ({ page }) => {
  await loaded(page);
  await page.selectOption('#puzzles', 'beginner-detour');
  await expect(page.locator('#title')).toHaveText('The Detour');
  await page.selectOption('#mode', 'optimal');
  await page.click('#solve');
  await expect(page.locator('#search-status')).toHaveText('24 remaining moves · proven move-optimal from this position. Search complete.');
  await expect(page.locator('#play')).toBeEnabled();
});

test('restores the solved session after a reload', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#message')).toHaveText(SOLVED);
  await expect.poll(() => page.evaluate(() => localStorage.getItem('sokomind-rust.v1.session'))).toContain('"actions":"D"');
  await page.reload();
  await expect(page.locator('#message')).toHaveText('Session restored by replaying your moves.');
  await expect(page.locator('#moves')).toHaveText('1');
  await expect(page.locator('#best-score')).toHaveText('Best: 1 moves · 1 pushes');
});
