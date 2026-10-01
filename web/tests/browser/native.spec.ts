import { expect, test } from '@playwright/test';

const SAVED = 'Verified best route saved in PostgreSQL for this browser profile.';

// Stubs the server API inside the browser, so these run through the real
// fetch calls without a Rust backend or database. The solve reply mirrors
// ResultBody in crates/server/src/solve.rs.
async function connected(page: import('@playwright/test').Page, stored: object | null = null) {
  await page.route('**/api/health', route => route.fulfill({ json: { status: 'ok', persistence: true } }));
  await page.route('**/api/solve', route =>
    route.fulfill({
      json: {
        status: 'solved',
        route: 'D',
        moves: 1,
        pushes: 1,
        expanded: 1,
        generated: 2,
        reserved_bytes: 1208114,
        elapsed_ms: 3,
        proof: { kind: 'optimal', lower_bound: 1, upper_bound: 1 },
        stats: {},
      },
    }),
  );
  await page.route('**/api/progress/**', route => {
    if (route.request().method() === 'POST') return route.fulfill({ json: { saved: true, improved: true } });
    return stored ? route.fulfill({ json: stored }) : route.fulfill({ status: 404, json: { error: 'No saved route' } });
  });
  await page.goto('/');
  await expect(page.locator('#title')).toHaveText('First Steps');
  // The health check ends start(); waiting for it keeps key presses from racing it.
  await expect(page.locator('#connection')).toHaveText('PostgreSQL connected');
}

test('native engine solves through the server API', async ({ page }) => {
  await connected(page);
  await page.selectOption('#mode', 'optimal');
  await page.selectOption('#engine', 'native');
  const [request] = await Promise.all([page.waitForRequest('**/api/solve'), page.click('#solve')]);
  const body = request.postDataJSON();
  expect(body.mode).toBe('optimal');
  expect(body.actions).toBe('');
  expect(body.rows).toHaveLength(5);
  expect(body.max_states).toBeGreaterThan(0);
  expect(body.memory_mib).toBe(64);
  await expect(page.locator('#search-status')).toContainText('1 remaining moves · proven move-optimal from this position');
  await expect(page.locator('#search-status')).toContainText('Search complete.');
  await expect(page.locator('#play')).toBeEnabled();
  await expect(page.locator('#expanded')).toHaveText('1');
});

test('a solved puzzle is saved to PostgreSQL progress', async ({ page }) => {
  await connected(page);
  const [save] = await Promise.all([
    page.waitForRequest(r => r.method() === 'POST' && r.url().endsWith('/api/progress/ultra-tiny')),
    page.keyboard.press('ArrowDown'),
  ]);
  expect(save.postDataJSON()).toEqual({ route: 'D' });
  expect(save.headers()['x-profile-id']).toMatch(/^[a-f0-9]{32}$/);
  await expect(page.locator('#storage')).toHaveText(SAVED);
});

test('Replay best solves again without saving the same route twice', async ({ page }) => {
  const posts: string[] = [];
  page.on('request', request => {
    if (request.method() === 'POST' && request.url().includes('/api/progress/')) posts.push(new URL(request.url()).pathname);
  });
  await connected(page);
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('#storage')).toHaveText(SAVED);
  await page.click('#undo');
  await expect(page.locator('#moves')).toHaveText('0');
  await page.click('#best');
  await expect(page.locator('#message')).toHaveText('Solved in 1 moves and 1 pushes.');
  // A save would start at the replay's solving step; Stop turns off one tick after it.
  await expect(page.locator('#cancel')).toBeDisabled();
  // Requests are reported in the order they start, so once the next puzzle's
  // progress lookup is seen, a save from the replay would have been seen too.
  await Promise.all([page.waitForRequest('**/api/progress/tiny'), page.click('#next')]);
  expect(posts).toEqual(['/api/progress/ultra-tiny']);
});

test('a verified server best is pulled on load', async ({ page }) => {
  await connected(page, { puzzle_id: 'ultra-tiny', route: 'D', moves: 1, pushes: 1 });
  await expect(page.locator('#best-score')).toHaveText('Best: 1 moves · 1 pushes');
  await expect(page.locator('#best')).toBeEnabled();
});

test('a server that first reports no persistence is asked again', async ({ page }) => {
  let persistence = false;
  await page.route('**/api/health', route => {
    const reply = route.fulfill({ json: { status: 'ok', persistence } });
    persistence = true;
    return reply;
  });
  await page.route('**/api/progress/**', route =>
    route.fulfill({
      json: {
        puzzle_id: 'ultra-tiny',
        route: 'D',
        moves: 1,
        pushes: 1,
      },
    }),
  );
  await page.goto('/');
  await expect(page.locator('#connection')).toHaveText('Native solver connected');
  // The first re-probe runs 2 s later, inside the default 5 s expect timeout.
  await expect(page.locator('#connection')).toHaveText('PostgreSQL connected');
  await expect(page.locator('#best-score')).toHaveText('Best: 1 moves · 1 pushes');
});
