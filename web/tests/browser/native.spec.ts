import { expect, test } from '@playwright/test';

// Stubs the server API inside the browser, so these run through the real
// fetch calls without a Rust backend. The solve reply mirrors ResultBody in
// crates/server/src/solve.rs.
async function connected(page: import('@playwright/test').Page) {
  await page.route('**/api/health', route => route.fulfill({ json: { status: 'ok' } }));
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
  await page.goto('/');
  await expect(page.locator('#title')).toHaveText('First Steps');
  // The health probe starts once start() has bound every control, so waiting
  // for its label keeps clicks from racing start().
  await expect(page.locator('#connection')).toHaveText('Native solver connected');
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
