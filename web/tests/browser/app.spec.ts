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
  await expect(page.locator('#reset-confirm')).toBeVisible();
  await page.click('#reset-now');
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
  await page.click('#open-solver');
  await expect(page.locator('#solver')).toBeVisible();
  await page.click('#solve');
  await expect(page.locator('#search-status')).toContainText('1 remaining moves');
  await expect(page.locator('#search-status')).toContainText('Search complete.');
  await expect(page.locator('#play')).toBeEnabled();
  await expect(page.locator('#copy')).toBeEnabled();
  await expect(page.locator('#expanded')).not.toHaveText('—');
});

// A modal dialog owns the keyboard: movement and undo must not act behind it,
// which the ported app's input gate ensured and this port's movement layer missed.
test('movement keys stay with an open dialog', async ({ page }) => {
  await loaded(page);
  await page.click('#open-solver');
  await expect(page.locator('#solver')).toBeVisible();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('u');
  await expect(page.locator('#moves')).toHaveText('0');
  await expect(page.locator('#board')).toHaveAttribute('aria-label', FIRST_BOARD);
  await page.click('#solver-close');
  await expect(page.locator('#solver')).not.toBeVisible();
  await page.keyboard.press('ArrowLeft');
  await expect(page.locator('#moves')).toHaveText('1');
});

// Quality shows no route on "Grand Hall" for about a second natively, and longer in
// WASM, until its stage search finds one, far longer than the gap between the two
// clicks, so Stop always lands while the search runs and no route is on show.
test('Stop ends a browser search and frees the controls', async ({ page }) => {
  await loaded(page);
  await page.goto('/#/play/huge');
  await expect(page.locator('#title')).toHaveText('Grand Hall');
  await page.click('#open-solver');
  await expect(page.locator('#solver')).toBeVisible();
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
  await page.click('#open-solver');
  await expect(page.locator('#solver')).toBeVisible();
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
  // The board must lay out, not just exist: its sheet targets the host's
  // class, and a board that lost it collapses into a column of its pieces.
  const box = (await page.locator('#board').boundingBox())!;
  expect(box.width).toBeGreaterThan(100);
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
  await expect(page.locator('#reset-confirm')).toBeVisible();
  await page.click('#reset-now');
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

// The clock runs only while attempting: a fresh board holds it, the first
// move starts it, and a restart's fresh clock waits for that attempt's first
// move too, so it can never freeze on a restart again.
test('the clock starts on the first move and starts over after a restart', async ({ page }) => {
  await loaded(page);
  await page.waitForTimeout(1500);
  await expect(page.locator('#timer')).toHaveText('0:00');
  await page.keyboard.press('ArrowLeft');
  await expect.poll(() => page.locator('#timer').textContent(), { timeout: 4000 }).not.toBe('0:00');
  await page.click('#reset');
  await expect(page.locator('#reset-confirm')).toBeVisible();
  await page.click('#reset-now');
  await expect(page.locator('#timer')).toHaveText('0:00');
  await page.waitForTimeout(1500);
  await expect(page.locator('#timer')).toHaveText('0:00');
  await page.keyboard.press('ArrowLeft');
  await expect.poll(() => page.locator('#timer').textContent(), { timeout: 4000 }).not.toBe('0:00');
});

// A restart with moves on the board asks before it throws the attempt away,
// as the ported app's guard did; an empty board restarts at once.
test('Restart asks before discarding an attempt', async ({ page }) => {
  await loaded(page);
  await page.click('#reset');
  await expect(page.locator('#reset-confirm')).not.toBeVisible();
  await expect(page.locator('#moves')).toHaveText('0');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowRight');
  await page.click('#reset');
  await expect(page.locator('#reset-confirm')).toBeVisible();
  await expect(page.locator('#reset-message')).toHaveText('Restarting removes 2 moves in this attempt. Your best route is not affected.');
  await page.click('#reset-keep');
  await expect(page.locator('#reset-confirm')).not.toBeVisible();
  await expect(page.locator('#moves')).toHaveText('2');
  await page.click('#reset');
  await page.click('#reset-now');
  await expect(page.locator('#moves')).toHaveText('0');
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

test('the home dashboard shows progress, tiers and the daily challenge', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/');
  await expect(page.locator('#home-dash > .muted')).toHaveText('0 of 57 rooms cleared');
  await expect(page.locator('.progress-fill')).toHaveCSS('width', '0px');
  await expect(page.locator('.tier-row a')).toHaveCount(6);
  await expect(page.locator('.tier-row a').first()).toContainText('tutorial');
  await expect(page.locator('.tier-row a').first()).toContainText('0/4');
  // The daily card names a catalog puzzle and plays it.
  await expect(page.locator('.daily h2')).not.toHaveText('');
  await expect(page.locator('.daily .button')).toHaveText('Play today’s room');
  const href = await page.locator('.daily .button').getAttribute('href');
  expect(href).toMatch(/^#\/play\//);
  // A fresh journal leaves the run open: seven days of history, today pending, no streak.
  await expect(page.locator('.daily-top span').last()).toHaveText('New room daily');
  await expect(page.locator('.daily-history li')).toHaveCount(7);
  await expect(page.locator('.daily-history li').last()).toHaveAttribute('data-outcome', 'today');
  await expect(page.locator('.daily-history li').last()).toContainText('Today');
  await expect(page.locator('.streak-banner')).toHaveCount(0);
});

test('a solve joins the journal as the streak and the recently cleared', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await page.click('#complete-close');
  await page.goto('/#/');
  await expect(page.locator('.streak-banner strong')).toHaveText('1-day streak');
  await expect(page.locator('.recent-list a strong')).toHaveText('First Steps');
  await expect(page.locator('.recent-list a span')).toHaveText('1m / 1p · just now');
});

test('today’s daily room, once cleared, records the day and the run', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/');
  await expect(page.locator('.daily .button')).toHaveText('Play today’s room');
  const id = (await page.locator('.daily .button').getAttribute('href'))!.replace('#/play/', '');
  // Seed the journal as though today’s room was cleared today, then read the card off it.
  await page.addInitScript(room => {
    const now = new Date();
    const day = now.toISOString().slice(0, 10);
    localStorage.setItem(
      'sokomind-rust.v1.journal',
      JSON.stringify({
        cleared: { [room]: { moves: 5, pushes: 2, clearedAt: now.toISOString() } },
        daily: { [day]: { id: room, at: now.toISOString() } },
        activity: { [day]: [room] },
      }),
    );
  }, id);
  await page.reload();
  await expect(page.locator('.daily')).toHaveAttribute('data-state', 'cleared');
  await expect(page.locator('.daily .button')).toHaveText('Replay today’s room');
  await expect(page.locator('.daily-top span').last()).toHaveText('1-day run');
  await expect(page.locator('.daily .muted')).toContainText('Today is recorded');
  await expect(page.locator('.daily-history li').last()).toHaveAttribute('data-outcome', 'cleared');
  await expect(page.locator('.daily-history li').last()).toContainText('✓');
  await expect(page.locator('.streak-banner strong')).toHaveText('1-day streak');
  await expect(page.locator('.recent-list a span')).toHaveText('5m / 2p · just now');
});

test('a solve can be rated from the completion dialog', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#complete')).toBeVisible();
  await page.click('#rate-right');
  await expect(page.locator('#rate-right')).toHaveClass(/on/);
  await expect(page.locator('#message')).toHaveText('Rating saved.');
  await page.click('#complete-close');
  // The rating persists into the next solve's dialog.
  await page.click('#reset');
  await expect(page.locator('#reset-confirm')).toBeVisible();
  await page.click('#reset-now');
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#rate-right')).toHaveClass(/on/);
});

test('the stats page reads the journal', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.addInitScript(() => {
    const now = new Date();
    localStorage.setItem(
      'sokomind-rust.v1.journal',
      JSON.stringify({
        cleared: { 'ultra-tiny': { moves: 5, pushes: 2, clearedAt: now.toISOString(), elapsedMs: 90000 } },
        daily: {},
        activity: { [now.toISOString().slice(0, 10)]: ['ultra-tiny'] },
      }),
    );
  });
  await page.goto('/#/stats');
  await expect(page.locator('#stats-solved')).toHaveText('1');
  await expect(page.locator('.stat-card:has(#stats-solved) .stat-sub')).toHaveText('of 57 (2%)');
  await expect(page.locator('.donut-center')).toHaveText('1');
  await expect(page.locator('.legend-item')).toHaveCount(1);
  await expect(page.locator('.legend-item')).toContainText('tutorial');
  await expect(page.locator('.bar-row')).toHaveCount(6);
  await expect(page.locator('.bar-row').first()).toContainText('1/4');
  await expect(page.locator('.heat-grid .heat-cell')).toHaveCount(90);
  await expect(page.locator(".heat-grid .heat-cell[data-level='1']")).toHaveCount(1);
  await expect(page.locator('#stats-copy')).toBeVisible();
  await expect(page.locator('#stats-export')).toBeVisible();
  await expect(page.locator('#stats-file')).toBeHidden();
});

test('an import merges with a summary', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  // The init script runs on every load, so it seeds only while nothing is stored:
  // the reload below must read the journal the import wrote, not the seed again.
  await page.addInitScript(() => {
    if (localStorage.getItem('sokomind-rust.v1.journal') !== null) return;
    const now = new Date();
    localStorage.setItem(
      'sokomind-rust.v1.journal',
      JSON.stringify({ cleared: { 'ultra-tiny': { moves: 5, pushes: 2, clearedAt: now.toISOString() } }, daily: {}, activity: {} }),
    );
  });
  await page.goto('/#/stats');
  await expect(page.locator('#stats-solved')).toHaveText('1');
  await page.setInputFiles('#stats-file', {
    name: 'progress.json',
    mimeType: 'application/json',
    buffer: Buffer.from(
      JSON.stringify({
        cleared: {
          'ultra-tiny': { moves: 4, pushes: 1, clearedAt: new Date().toISOString() },
          'ghost-room': { moves: 9, pushes: 4, clearedAt: new Date().toISOString() },
        },
        daily: {},
        activity: {},
      }),
    ),
  });
  await expect(page.locator('#stats-status')).toHaveText('Imported: 0 added, 1 improved, 0 unchanged, 1 rejected, 0 invalid.');
  // The merged record persists: the moves card carries the imported route.
  await page.reload();
  await expect(page.locator('#stats-solved')).toHaveText('1');
  await expect(page.locator('.stat-card:has-text("Total moves") .stat-big')).toHaveText('4');
});

test('the type-RESET reset clears progress', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.addInitScript(() => {
    const now = new Date();
    localStorage.setItem(
      'sokomind-rust.v1.journal',
      JSON.stringify({
        cleared: { 'ultra-tiny': { moves: 5, pushes: 2, clearedAt: now.toISOString() } },
        daily: {},
        activity: { [now.toISOString().slice(0, 10)]: ['ultra-tiny'] },
      }),
    );
  });
  await page.goto('/#/stats');
  await expect(page.locator('#stats-solved')).toHaveText('1');
  await page.click('#stats-reset');
  await page.getByRole('button', { name: 'I understand, continue' }).click();
  await expect(page.locator('#stats-reset')).toBeDisabled();
  await page.fill('#stats-reset-input', 'RESE');
  await expect(page.locator('#stats-reset')).toBeDisabled();
  await page.fill('#stats-reset-input', 'RESET');
  await expect(page.locator('#stats-reset')).toBeEnabled();
  await page.click('#stats-reset');
  await expect(page).toHaveURL(/#\/$/);
  await expect(page.locator('#home-dash > .muted')).toHaveText('0 of 57 rooms cleared');
});

test('the guided journey recommends the next room and pauses', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/');
  await expect(page.locator('.journey .eyebrow')).toHaveText('Optional journey · 0/19');
  // A fresh journal points at the first room of the first chapter.
  await expect(page.locator('.journey-next strong')).toHaveText('First Steps');
  await expect(page.locator('.journey-next .button')).toHaveAttribute('href', '#/play/ultra-tiny');
  await expect(page.locator('.journey-map .chapter')).toHaveCount(5);
  await expect(page.locator('.chapter-nodes .node')).toHaveCount(19);
  // Pause folds the card to its resume state; resume brings it back.
  await page.click('.journey-pause');
  await expect(page.locator('.journey h2')).toHaveText('Explore your own way');
  await page.getByRole('button', { name: 'Resume guided path' }).click();
  await expect(page.locator('.journey .eyebrow')).toHaveText('Optional journey · 0/19');
});

test('achievements and keepsakes render from the journal', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.addInitScript(() => {
    const now = new Date();
    const day = now.toISOString().slice(0, 10);
    localStorage.setItem(
      'sokomind-rust.v1.journal',
      JSON.stringify({
        cleared: {
          'ultra-tiny': { moves: 5, pushes: 2, clearedAt: now.toISOString() },
          'tutorial-push': { moves: 6, pushes: 3, clearedAt: now.toISOString() },
          tiny: { moves: 7, pushes: 4, clearedAt: now.toISOString() },
        },
        daily: {},
        activity: { [day]: ['ultra-tiny'] },
      }),
    );
  });
  await page.goto('/#/stats');
  await expect(page.locator('.showcase-intro strong')).toHaveText('1/17 earned');
  await expect(page.locator('.milestone-list li')).toHaveCount(1);
  await expect(page.locator('.milestone-list li')).toContainText('First Steps');
  await expect(page.locator('.milestone-list li')).toContainText('Room by room');
  await expect(page.locator(".achievement-list li[data-earned='true']")).toHaveCount(1);
  // The classic frame wears; the earned ones name their requirements.
  await expect(page.locator(".keepsake[data-frame='classic']")).toHaveAttribute('data-active', 'true');
  await expect(page.locator(".keepsake[data-frame='classic'] em")).toHaveText('Active');
  await expect(page.locator(".keepsake[data-frame='sage-thread']")).toBeDisabled();
  await expect(page.locator(".keepsake[data-frame='sage-thread'] em")).toHaveText('Earn Getting Started');
});

test('an earned frame wears on the board, and ratings mark their rows', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.addInitScript(() => {
    const now = new Date();
    const rooms = [
      'ultra-tiny',
      'tutorial-push',
      'tiny',
      'tutorial-around',
      'beginner-detour',
      'box-5x5-a',
      'beginner-three',
      'beginner-typed-line',
      'workshop-1',
      'classic-1',
    ];
    const cleared: Record<string, { moves: number; pushes: number; clearedAt: string }> = {};
    for (const room of rooms) cleared[room] = { moves: 6, pushes: 2, clearedAt: now.toISOString() };
    localStorage.setItem('sokomind-rust.v1.journal', JSON.stringify({ cleared, daily: {}, activity: {} }));
    localStorage.setItem('sokomind-rust.v1.ratings', JSON.stringify({ 'ultra-tiny': 'easy' }));
  });
  await page.goto('/#/stats');
  // Ten cleared rooms earn the sage thread; choosing it makes it wear.
  await expect(page.locator(".keepsake[data-frame='sage-thread']")).toBeEnabled();
  await expect(page.locator(".keepsake[data-frame='sage-thread'] em")).toHaveText('Available');
  await page.click(".keepsake[data-frame='sage-thread']");
  await expect(page.locator(".keepsake[data-frame='sage-thread']")).toHaveAttribute('data-active', 'true');
  await page.goto(PLAY_FIRST);
  await expect(page.locator('#title')).toHaveText('First Steps');
  await expect(page.locator('html')).toHaveAttribute('data-board-frame', 'sage-thread');
  // The felt-difficulty rating marks its row in the catalog's list.
  await page.goto('/#/puzzles/tutorial');
  await expect(page.locator('.puzzle-row').first().locator('.rating-dot')).toHaveAttribute('data-rating', 'easy');
  await expect(page.locator('.puzzle-row').first().locator('.rating-dot')).toHaveAttribute('title', 'Felt easy');
});

test('a proven optimum earns its star', async ({ page }) => {
  await loaded(page);
  // An optimal search from the start certifies the room's optimum.
  await page.click('#open-solver');
  await expect(page.locator('#solver')).toBeVisible();
  await page.selectOption('#mode', 'optimal');
  await page.click('#solve');
  await expect(page.locator('#search-status')).toHaveText('1 remaining moves · proven move-optimal from this position. Search complete.');
  // The laboratory folds before the board's own controls answer.
  await page.click('#solver-close');
  await expect(page.locator('#solver')).not.toBeVisible();
  // Solving to exactly the certified count earns the star.
  await page.click('#reset');
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#complete-milestones li')).toHaveText(['First clear', '★ Proven move-optimal']);
});

test('the replay shelf lists and plays a best route', async ({ page }) => {
  await loaded(page);
  await page.keyboard.press('ArrowDown');
  await page.click('#complete-close');
  await page.goto('/#/stats');
  await expect(page.locator('.shelf-list li')).toHaveCount(1);
  await expect(page.locator('.shelf-list li')).toContainText('First Steps');
  await expect(page.locator('.shelf-list li')).toContainText('1 moves · 1 pushes · 1 route');
  // The replay loads the room fresh and plays the route as a playback.
  await page.getByRole('button', { name: 'Replay' }).click();
  await expect(page.locator('#title')).toHaveText('First Steps');
  await expect(page.locator('#moves')).toHaveText('1');
  await expect(page.locator('#message')).toHaveText(SOLVED);
});

test('the replay study compares two routes', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await loaded(page);
  // A first three-move solve, then the one-move improvement: two routes kept.
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await page.click('#complete-close');
  await page.click('#reset');
  await expect(page.locator('#reset-confirm')).toBeVisible();
  await page.click('#reset-now');
  await page.keyboard.press('ArrowDown');
  await page.click('#complete-close');
  await page.goto('/#/stats');
  await expect(page.locator('.shelf-list li')).toHaveCount(1);
  await expect(page.locator('.shelf-list li')).toContainText('2 routes');
  await page.getByRole('button', { name: 'Study', exact: true }).click();
  await expect(page.locator('#replay')).toBeVisible();
  await expect(page.locator('#replay-title')).toHaveText('First Steps');
  await expect(page.locator('#replay-watch')).toHaveValue('0');
  await expect(page.locator('#replay-position')).toHaveText('Move 0 / 1');
  // The comparison names where the two routes separate.
  await expect(page.locator('#replay-summary')).toContainText('match for 0 moves');
  await expect(page.locator('.replay-marker')).toHaveCount(2);
  await expect(page.locator('.replay-marker').first()).toHaveAttribute('data-kind', 'direction');
  // Seeking the end solves the board; the close button folds the study.
  await page.click('#replay-last');
  await expect(page.locator('#replay-position')).toHaveText('Move 1 / 1');
  await expect(page.locator('#replay-board')).toHaveAttribute('data-solved', '');
  await page.click('#replay-close');
  await expect(page.locator('#replay')).not.toBeVisible();
});

test('the solver lab runs and compares two searches', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/solver-lab/ultra-tiny');
  await expect(page.locator('#lab-note')).toHaveText('Initial position');
  await expect(page.locator('#lab-puzzle')).toHaveValue('ultra-tiny');
  await expect(page.locator('#lab-play')).toHaveAttribute('href', '#/play/ultra-tiny');
  // A fast search returns a route the lab can watch and step through.
  await page.click('#lab-run');
  await expect(page.locator('#lab-watch option')).toHaveCount(1);
  await expect(page.locator('#lab-watched-label')).toContainText('fast · browser');
  await expect(page.locator('#lab-position')).toContainText('Move 0 of ');
  await page.click('#lab-forward');
  await expect(page.locator('#lab-position')).toContainText('Move 1 of ');
  // A second run under another mode gives the comparison its two sides.
  await page.selectOption('#lab-mode', 'optimal');
  await page.click('#lab-run');
  await expect(page.locator('#lab-watch option')).toHaveCount(2);
  await expect(page.locator('#lab-runs')).toHaveText('2 of 6 runs');
  await expect(page.locator('.lab-delta strong')).toHaveText('Same puzzle and starting state');
  await expect(page.locator('.lab-delta span')).toHaveText('Limits or mode differ');
  await expect(page.locator('.lab-compare-grid article')).toHaveCount(2);
});

test('the search laboratory logs its search and links the lab', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await loaded(page);
  await page.click('#open-solver');
  await expect(page.locator('#solver')).toBeVisible();
  await page.click('#solve');
  await expect(page.locator('#search-status')).toContainText('Search complete.');
  // The status log lists what the solver said, newest first.
  await expect(page.locator('#solver-log li')).not.toHaveCount(0);
  await expect(page.locator('#solver-log li').first()).toContainText('Search complete.');
  // The result panel names the route and its guarantee.
  await expect(page.locator('#solver-result-title')).toHaveText('Route found');
  await expect(page.locator('#solver-result-moves')).toHaveText('1');
  await expect(page.locator('#solver-result-guarantee')).toHaveText('First route found');
  // The laboratory's door to the lab carries the room and its position.
  await page.click('#solver-lab-link');
  await expect(page).toHaveURL(/#\/solver-lab\/ultra-tiny$/);
  await expect(page.locator('#lab-note')).toHaveText('Initial position');
});

test('the editor paints a starter and playtests it', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/editor');
  // A never-painted draft meets the starter templates.
  await expect(page.locator('#editor-starters')).toBeVisible();
  await page.click('#editor-starter-single');
  await expect(page.locator('#editor-starters')).not.toBeVisible();
  // The starter room parses, so it is ready to test and share.
  await expect(page.locator('#editor-validation')).toHaveText('Puzzle is valid');
  await expect(page.locator('.editor-cell[data-row="2"][data-column="3"]')).toHaveAttribute('data-symbol', 'X');
  await page.click('#editor-test');
  await expect(page.locator('#editor-playtest')).toBeVisible();
  await expect(page.locator('#editor-moves')).toHaveText('0');
  // The box sits directly left of its goal: one push solves the room.
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('#editor-moves')).toHaveText('1');
  await expect(page.locator('#editor-pushes')).toHaveText('1');
  await expect(page.locator('#editor-playtest-status')).toHaveText('Solved in 1 move!');
  await page.click('#editor-playtest-exit');
  await expect(page.locator('#editor-playtest')).not.toBeVisible();
  await expect(page.locator('#editor-test')).toBeEnabled();
});

test('edits autosave into the drafts and survive a reload', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/editor');
  await page.click('#editor-starter-single');
  // The box tool is key 4; a click paints with it.
  await page.keyboard.press('4');
  await page.click('.editor-cell[data-row="1"][data-column="1"]');
  await expect(page.locator('.editor-cell[data-row="1"][data-column="1"]')).toHaveAttribute('data-symbol', 'X');
  // Leaving the editor flushes the autosave; the reload proves it stored.
  await page.goto('/#/');
  await page.reload();
  await page.goto('/#/editor');
  await expect(page.locator('.editor-cell[data-row="1"][data-column="1"]')).toHaveAttribute('data-symbol', 'X');
  await expect(page.locator('#editor-title')).toHaveValue('Single push starter');
  await expect(page.locator('#editor-drafts')).toHaveValue('draft-1');
  await expect(page.locator('#editor-draft-saved')).toContainText('Last saved');
});

test('undo and redo walk the edit history', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/editor');
  await page.click('#editor-starter-single');
  await page.keyboard.press('4');
  await page.click('.editor-cell[data-row="1"][data-column="1"]');
  await expect(page.locator('.editor-cell[data-row="1"][data-column="1"]')).toHaveAttribute('data-symbol', 'X');
  await page.keyboard.press('Control+z');
  await expect(page.locator('.editor-cell[data-row="1"][data-column="1"]')).toHaveAttribute('data-symbol', ' ');
  await expect(page.locator('#editor-redo')).toBeEnabled();
  // Undoing past the paint reaches the blank room the templates address.
  await page.keyboard.press('Control+z');
  await expect(page.locator('#editor-starters')).toBeVisible();
  await page.keyboard.press('Control+Shift+z');
  await page.keyboard.press('Control+Shift+z');
  await expect(page.locator('.editor-cell[data-row="1"][data-column="1"]')).toHaveAttribute('data-symbol', 'X');
  await expect(page.locator('#editor-starters')).not.toBeVisible();
});

test('the typed pair paints labels that must match', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/editor');
  await page.click('#editor-starter-single');
  await page.click('#editor-typed-box');
  await expect(page.locator('#editor-typed-box')).toHaveAttribute('aria-pressed', 'true');
  await page.click('.editor-cell[data-row="1"][data-column="1"]');
  await page.click('.editor-cell[data-row="1"][data-column="2"]');
  await expect(page.locator('.editor-cell[data-row="1"][data-column="1"]')).toHaveAttribute('data-symbol', 'A');
  await expect(page.locator('.editor-cell[data-row="1"][data-column="2"]')).toHaveAttribute('data-symbol', 'A');
  // One goal against two boxes: the readiness column names the label that is off.
  await page.click('#editor-typed-goal');
  await page.click('.editor-cell[data-row="3"][data-column="1"]');
  await expect(page.locator('#editor-validation')).toContainText('Label A: 2 boxes but 1 goals.');
  await expect(page.locator('#editor-test')).toBeDisabled();
  // Matching the pair makes the room ready again.
  await page.click('.editor-cell[data-row="3"][data-column="2"]');
  await expect(page.locator('#editor-validation')).toHaveText('Puzzle is valid');
  await expect(page.locator('#editor-test')).toBeEnabled();
});

test('a shared link previews read-only and imports as a draft', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/editor');
  await page.click('#editor-starter-single');
  await page.fill('#editor-title', 'Workshop gift');
  await expect(page.locator('#editor-validation')).toHaveText('Puzzle is valid');
  await page.click('#editor-share');
  // The share field always carries the link, whatever the clipboard does.
  await expect(page.locator('#editor-share-field')).toBeVisible();
  const url = await page.locator('#editor-share-url').inputValue();
  expect(url).toContain('#/editor?custom=');
  await page.goto(url);
  // The preview keeps the saved draft untouched and the grid read-only.
  await expect(page.locator('#editor-shared')).toBeVisible();
  await expect(page.locator('#editor-shared-note')).toContainText('Your saved draft "Untitled" is unchanged.');
  await expect(page.locator('.editor-cell[data-row="2"][data-column="3"]')).toBeDisabled();
  await expect(page.locator('#editor-title')).toHaveValue('Workshop gift');
  // The shared puzzle still plays, privately.
  await page.click('#editor-shared-play');
  await expect(page.locator('#editor-playtest')).toBeVisible();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('#editor-playtest-status')).toHaveText('Solved in 1 move!');
  await page.click('#editor-playtest-exit');
  // Importing folds the preview into the drafts as its own copy.
  await page.click('#editor-shared-import');
  await expect(page).toHaveURL(/#\/editor$/);
  await expect(page.locator('#editor-fields')).toBeEnabled();
  await expect(page.locator('#editor-drafts option')).toHaveCount(2);
  await expect(page.locator('#editor-draft-name')).toHaveValue('Workshop gift');
  await expect(page.locator('#editor-title')).toHaveValue('Workshop gift');
});

test('an unreadable shared link leaves the draft editable', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/editor?custom=abc!def');
  await expect(page.locator('#editor-link-error')).toBeVisible();
  await expect(page.locator('#editor-link-error')).toContainText('invalid or incomplete');
  await expect(page.locator('#editor-starters')).toBeVisible();
  await expect(page.locator('.editor-cell[data-row="0"][data-column="0"]')).toBeEnabled();
});

test('the play page shares a pasted puzzle as an editor link', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await loaded(page);
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: (text: string) => {
          (window as unknown as { __shared: string }).__shared = text;
          return Promise.resolve();
        },
      },
    });
  });
  await page.locator('summary', { hasText: 'Import / edit a puzzle' }).click();
  await page.fill('#rows', 'OOOOO\nO R O\nO X O\nO S O\nOOOOO');
  await page.click('#load-custom');
  await expect(page.locator('#title')).toHaveText('Your puzzle');
  await page.click('#share');
  await expect(page.locator('#message')).toHaveText('Share link copied.');
  const url = await page.evaluate(() => (window as unknown as { __shared: string }).__shared);
  expect(url).toContain('#/editor?custom=');
  await page.goto(url);
  await expect(page.locator('#editor-shared')).toBeVisible();
  await expect(page.locator('#editor-title')).toHaveValue('Your puzzle');
  await expect(page.locator('.editor-cell[data-row="2"][data-column="2"]')).toHaveAttribute('data-symbol', 'X');
});

test('the generator grows a room and loads it into the editor', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/editor');
  await page.click('#editor-generate');
  await expect(page.locator('#generator')).toBeVisible();
  // The dialog's fields carry the default room.
  await expect(page.locator('#generator-difficulty')).toHaveValue('beginner');
  await expect(page.locator('#generator-width')).toHaveValue('7');
  await page.click('#generator-run');
  // The run reports its progress, then the result the engine certified.
  await expect(page.locator('#generator-result')).toBeVisible({ timeout: 15000 });
  await expect(page.locator('#generator-result-title')).toHaveText('Puzzle generated');
  // Loading folds the room into the grid, engine-verified and valid.
  await page.click('#generator-accept');
  await expect(page.locator('#generator')).not.toBeVisible();
  await expect(page.locator('#editor-validation')).toHaveText('Puzzle is valid');
  await expect(page.locator('.editor-cell[data-symbol="R"]')).toHaveCount(1);
  await expect(page.locator('#editor-notice')).toContainText('Generated room loaded');
});

test('the difficulty preset shapes the room the generator grows', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await page.goto('/#/editor');
  await page.click('#editor-generate');
  await page.selectOption('#generator-difficulty', 'advanced');
  await expect(page.locator('#generator-width')).toHaveValue('9');
  await expect(page.locator('#generator-height')).toHaveValue('9');
  await expect(page.locator('#generator-boxes')).toHaveValue('5');
  // A one-box room cannot carry typed pairs.
  await page.selectOption('#generator-difficulty', 'tutorial');
  await expect(page.locator('#generator-boxes')).toHaveValue('1');
  await expect(page.locator('#generator-labels')).toBeDisabled();
});

test('the sound choices persist and M mutes', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await loaded(page);
  // The menu shows the defaults.
  await page.locator('summary', { hasText: 'Theme & sound' }).click();
  await expect(page.locator('#sound-enabled')).toBeChecked();
  await expect(page.locator('#effects-volume-out')).toHaveText('50%');
  // M mutes from wherever the page stands, and the menu follows.
  await page.keyboard.press('m');
  await expect(page.locator('#sound-enabled')).not.toBeChecked();
  await expect(page.locator('#audio-note')).toHaveText('Audio muted.');
  // The music toggle follows the master switch.
  await expect(page.locator('#music-enabled')).toBeDisabled();
  // The choice persists across a reload, and M restores it.
  await page.reload();
  await expect(page.locator('#title')).toHaveText('First Steps');
  await page.locator('summary', { hasText: 'Theme & sound' }).click();
  await expect(page.locator('#sound-enabled')).not.toBeChecked();
  await page.keyboard.press('m');
  await expect(page.locator('#sound-enabled')).toBeChecked();
  await expect(page.locator('#audio-note')).toHaveText('Audio restored.');
});

test('the app installs as an offline-capable PWA', async ({ page }) => {
  await page.route('**/api/health', health => health.fulfill({ status: 404 }));
  await loaded(page);
  // The document links the manifest and the icons.
  await expect(page.locator('link[rel="manifest"]')).toHaveAttribute('href', /manifest\.webmanifest$/);
  await expect(page.locator('link[rel="icon"]')).toHaveAttribute('href', /favicon\.svg$/);
  // The service worker takes control of the page. ready resolves as soon as
  // an active worker exists — including its activating phase, while the
  // cache cleanup still runs — so poll for the settled state.
  await expect
    .poll(async () => page.evaluate(() => navigator.serviceWorker.ready.then(registration => registration.active?.state ?? 'none')))
    .toBe('activated');
  // The shell the worker installed is cached for offline play.
  const cached = await page.evaluate(async () => {
    const names = await caches.keys();
    const shell = names.find(name => name.startsWith('sokomind-shell'));
    if (shell === undefined) return false;
    const cache = await caches.open(shell);
    return (await cache.match('./')) !== undefined;
  });
  expect(cached).toBe(true);
});

// The bottom navigation is the only navigation under 850px: the desktop nav
// hides there, and the port ordered the bar's hiding rule after the
// breakpoint that shows it, so phones had no page navigation at all.
test.describe('phone viewport', () => {
  test.use({ viewport: { width: 375, height: 667 } });

  test('the bottom navigation carries phones between pages', async ({ page }) => {
    await loaded(page);
    await expect(page.locator('.mobile-nav')).toBeVisible();
    await expect(page.locator('.site-nav')).toBeHidden();
    await page.locator('.mobile-nav a[href="#/puzzles"]').click();
    await expect(page).toHaveURL(/#\/puzzles$/);
    await expect(page.locator('#puzzles-body')).toBeVisible();
  });
});
