import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

// The app invests in its accessibility patterns — focus management, live
// regions, dialogs, labeled controls — and this sweep locks them on every
// page, the same ruleset each run. Whatever axe cannot judge fairly on this
// app is excluded here by rule with its reason, never disabled wholesale.
const PAGES: [string, string][] = [
  ['/', 'Sokomind'],
  ['/#/puzzles', 'Puzzles'],
  ['/#/play/ultra-tiny', 'First Steps'],
  ['/#/stats', 'Statistics'],
  ['/#/solver-lab', 'Solver Lab'],
  ['/#/editor', 'Puzzle editor'],
];

for (const [hash, title] of PAGES) {
  test(`axe raises nothing on ${title}`, async ({ page }) => {
    await page.route('**/api/health', health => health.fulfill({ status: 404 }));
    await page.goto(hash);
    // The shell keeps every page's section in the DOM, hidden; the sweep
    // judges the page on show.
    await expect(page.locator('[data-page]:not([hidden]) h1')).toHaveText(title);
    const results = await new AxeBuilder({ page }).analyze();
    expect(results.violations).toEqual([]);
  });
}
