import { expect, test } from '@playwright/test';

const MOVE_HINT = 'Arrow keys or WASD to move. U to undo.';
const FIRST_BOARD = '5 by 5 puzzle, 1 boxes. 0 moves, 0 pushes. Use arrow keys or WASD to move, U to undo.';
// "First Steps": the robot stands directly above the box, whose goal is one
// cell below it, so a single Down push solves the puzzle.
const SOLVED = 'Solved in 1 moves and 1 pushes.';
const PLAY_FIRST = '/#/play/ultra-tiny';

async function loaded(page: import('@playwright/test').Page) {
  // vite preview proxies /api to 127.0.0.1:3000, where the suite runs no API;
  // a 404 stands in for a host without one and keeps proxy errors out of the log.
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto(PLAY_FIRST);
  await expect(page.locator('#title')).toHaveText('First Steps');
}

test('renders the first catalog puzzle', async ({ page }) => {
  await loaded(page);
  await expect(page.locator('#difficulty')).toHaveText('tutorial');
  await expect(page.locator('#position')).toHaveText('1 / 57');
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
    '5 by 5 puzzle, 1 boxes. 1 moves, 1 pushes. Solved. Use arrow keys or WASD to move, U to undo.',
  );
  await expect(page.locator('#solve')).toBeDisabled();
  await expect(page.locator('#undo')).toBeEnabled();
});

test('undo restores the previous position', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#moves')).toHaveText('1');
  // The solve opened the completion dialog; the board's controls need it closed.
  await page.click('#complete-close');
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
  await page.click('#complete-close');
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

// Quality shows no route on "Grand Hall" for about a second natively, and longer in
// WASM, until its stage search finds one, far longer than the gap between the two
// clicks, so Stop always lands while the search runs and no route is on show.
test('Stop ends a browser search and frees the controls', async ({ page }) => {
  await loaded(page);
  await page.goto('/#/play/huge');
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
  await page.goto('/#/play/beginner-detour');
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

test('the home page leads into play, and Escape comes back', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/');
  await expect(page.locator('[data-page="home"] h1')).toHaveText('Sokomind');
  await expect(page.locator('#announcer')).toHaveText('Navigated to Home');
  await page.click('#start');
  await expect(page.locator('#title')).toHaveText('First Steps');
  await expect(page.locator('#announcer')).toHaveText('Navigated to Play');
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-page="home"] h1')).toBeVisible();
  await expect(page.locator('#announcer')).toHaveText('Navigated to Home');
});

test('a route naming no catalog puzzle lands home', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/play/nope');
  await expect(page.locator('[data-page="home"] h1')).toHaveText('Sokomind');
  await expect(page).toHaveURL(/#\/$/);
});

test('the picker and the arrows navigate play routes', async ({ page }) => {
  await loaded(page);
  await page.click('#next');
  await expect(page).toHaveURL(/#\/play\/tiny$/);
  await expect(page.locator('#title')).toHaveText("Two's Company");
  await expect(page.locator('#position')).toHaveText('2 / 57');
  await page.click('#prev');
  await expect(page.locator('#title')).toHaveText('First Steps');
  await page.selectOption('#puzzles', 'huge');
  await expect(page).toHaveURL(/#\/play\/huge/);
  await expect(page.locator('#title')).toHaveText('Grand Hall');
});

test('the DOM board renders pieces that move and celebrate', async ({ page }) => {
  await loaded(page);
  await expect(page.locator('#board .cell')).toHaveCount(25);
  await expect(page.locator('#board .robot')).toHaveCount(1);
  await expect(page.locator('#board .crate')).toHaveCount(1);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#board')).toHaveAttribute('data-solved', '');
  await expect(page.locator('#board .crate[data-home]')).toHaveCount(1);
  await expect(page.locator('#board .trail-dot')).toHaveCount(1);
});

test('a tap on a reachable cell walks the robot there', async ({ page }) => {
  await loaded(page);
  const box = (await page.locator('#board').boundingBox())!;
  // First Steps is a 5 by 5 board; the robot stands at row 1, column 2.
  const at = (row: number, column: number) => ({
    x: box.x + ((column + 0.5) / 5) * box.width,
    y: box.y + ((row + 0.5) / 5) * box.height,
  });
  await page.mouse.click(at(1, 1).x, at(1, 1).y);
  await expect(page.locator('#board')).toHaveAttribute('aria-label', /1 moves, 0 pushes\./);
  await page.mouse.click(at(1, 3).x, at(1, 3).y);
  await expect(page.locator('#board')).toHaveAttribute('aria-label', /3 moves, 0 pushes\./);
  // The box's own cell and a wall are not walk targets.
  await page.mouse.click(at(2, 2).x, at(2, 2).y);
  await page.mouse.click(at(0, 2).x, at(0, 2).y);
  await expect(page.locator('#board')).toHaveAttribute('aria-label', /3 moves, 0 pushes\./);
});

test('the score card and move strips follow the attempt', async ({ page }) => {
  await loaded(page);
  await expect(page.locator('#timer')).toHaveText('0:00');
  await expect(page.locator('#placed')).toHaveText('0 / 1');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#notation')).toHaveText('←→↓');
  await expect(page.locator('#push-share')).toHaveText('33% pushes');
  await expect(page.locator('#efficiency')).toHaveText('33%');
  await expect(page.locator('#placed')).toHaveText('1 / 1');
  await expect(page.locator('#undo-depth')).toHaveText('(3)');
  await expect(page.locator('#undo-5')).toBeHidden();
  await page.click('#complete-close');
  await page.click('#undo');
  await expect(page.locator('#notation')).toHaveText('←→');
  await expect(page.locator('#undo-depth')).toHaveText('(2)');
});

test('a solve opens the completion dialog with its tier and celebration', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#complete')).toBeVisible();
  await expect(page.locator('#complete-eyebrow')).toHaveText('FIRST CLEAR');
  await expect(page.locator('#complete-title')).toHaveText('Room cleared');
  await expect(page.locator('#complete-milestones li')).toHaveCount(1);
  await expect(page.locator('#celebrate .particle')).toHaveCount(20);
  await page.click('#complete-close');
  await expect(page.locator('#complete')).not.toBeVisible();
  // Solving again at the same count is a plain clear, with fewer particles.
  await page.click('#reset');
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#complete-eyebrow')).toHaveText('ROOM CLEARED');
  await expect(page.locator('#celebrate .particle')).toHaveCount(8);
  await page.click('#complete-close');
});

test('zen mode folds the chrome away and back', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('z');
  await expect(page.locator('[data-page="play"]')).toHaveClass(/zen/);
  await expect(page.locator('.play-grid > aside')).toBeHidden();
  await expect(page.locator('#zen-dock')).toBeVisible();
  // The board keeps taking input in zen.
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#moves')).toHaveText('1');
  await page.click('#complete-close');
  await page.click('#exit-zen');
  await expect(page.locator('.play-grid > aside')).toBeVisible();
});

test('pause locks input and the clock until P resumes', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('p');
  await expect(page.locator('#pause')).toBeVisible();
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#moves')).toHaveText('0');
  await page.keyboard.press('p');
  await expect(page.locator('#pause')).toBeHidden();
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#moves')).toHaveText('1');
});

test('the bracket keys walk puzzles and ? lists the shortcuts', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press(']');
  await expect(page).toHaveURL(/#\/play\/tiny$/);
  // The URL changes before the puzzle loads; the title waits out that race,
  // or the next bracket would step from the stale session and wrap.
  await expect(page.locator('#title')).toHaveText("Two's Company");
  await page.keyboard.press('[');
  await expect(page).toHaveURL(/#\/play\/ultra-tiny$/);
  await page.keyboard.press('?');
  await expect(page.locator('#shortcuts')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('#shortcuts')).not.toBeVisible();
});

test('a push that strands a box opens the deadlock dialog and reddens it', async ({ page }) => {
  await loaded(page);
  await page.locator('summary', { hasText: 'Import / edit a puzzle' }).click();
  await page.fill('#rows', 'OOOOOOO\nO  R  O\nO  A  O\nOa  B O\nO  OO O\nO  b  O\nOOOOOOO');
  await page.click('#load-custom');
  // Pushing A down corners it with B against the walls: the 2x2 rule strands them both.
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#deadlock')).toBeVisible();
  await expect(page.locator('#board .crate[data-deadlocked]')).toHaveCount(2);
  await page.click('#deadlock-undo');
  await expect(page.locator('#deadlock')).not.toBeVisible();
  await expect(page.locator('#board .crate[data-deadlocked]')).toHaveCount(0);
});

test('the hint plays the first steps of a fast route', async ({ page }) => {
  await loaded(page);
  await page.click('#hint');
  await expect(page.locator('#board')).toHaveAttribute('data-solved', '');
  await expect(page.locator('#moves')).toHaveText('1');
});

test('share copies the attempt as a play link', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowLeft');
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.click('#share');
  await expect(page.locator('#message')).toHaveText('Share link copied.');
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toContain('#/play/ultra-tiny?play=L');
});

test('the puzzle selector drills from tiers to a playable row', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/puzzles');
  await expect(page.locator('.grid-card')).toHaveCount(6);
  await expect(page.locator('.grid-card').first()).toContainText('tutorial');
  await expect(page.locator('.grid-card').first()).toContainText('0 of 4 cleared');
  // The tutorial tier holds one collection, so its route is that collection's list already.
  await page.click('.grid-card:first-child');
  await expect(page).toHaveURL(/#\/puzzles\/tutorial$/);
  await expect(page.locator('.puzzle-row')).toHaveCount(4);
  await expect(page.locator('.list-summary')).toHaveText('Showing 1–4 of 4 puzzles');
  // The search narrows the list, and a row plays its puzzle.
  await page.fill('#puzzle-search', 'first steps');
  await expect(page.locator('.puzzle-row')).toHaveCount(1);
  await page.click('.puzzle-row');
  await expect(page).toHaveURL(/#\/play\/ultra-tiny$/);
  await expect(page.locator('#title')).toHaveText('First Steps');
});

test('the collection grid leads to its list, and favorites shape it', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/puzzles/beginner');
  await expect(page.locator('.grid-card')).toHaveCount(2);
  await page.click('.grid-card:first-child');
  await expect(page).toHaveURL(/#\/puzzles\/beginner\/Sokomind%20Originals$/);
  await expect(page.locator('.puzzle-row')).toHaveCount(4);
  await expect(page.locator('.list-summary')).toHaveText('Showing 1–4 of 4 puzzles');
  // The heart favorites a puzzle; the Favorites chip keeps only favorites.
  await page.click('.puzzle-row:first-child .heart');
  await expect(page.locator('.puzzle-row:first-child .heart')).toHaveClass(/on/);
  await page.click('.chips button:nth-child(4)');
  await expect(page.locator('.puzzle-row')).toHaveCount(1);
  await expect(page.locator('.puzzle-row .heart')).toHaveClass(/on/);
});
