import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execute, formatDuration, plan, summary } from './validate.mjs';

/** @typedef {import('./validate.mjs').Step} Step */

/** @param {readonly Step[]} steps */
const commands = steps => steps.map(s => s.command);
/** @param {readonly Step[]} steps */
const skipped = steps => steps.filter(s => s.skip);

/** @type {Step[]} */
const STEPS = [
  { command: 'npm run a' },
  { command: 'npm run b' },
  { command: 'npm run c', skip: 'skipped by --quick' },
  { command: 'npm run d' },
];

/**
 * Executes STEPS with a runner that fails the `failing` commands with exit code 3
 * and a clock that advances 1.5 s per reading.
 * @param {boolean} keepGoing
 * @param {readonly string[]} failing
 */
function fake(keepGoing, failing) {
  /** @type {string[]} */
  const ran = [];
  let clock = 0;
  const outcomes = execute(STEPS, {
    keepGoing,
    run: command => {
      ran.push(command);
      return failing.includes(command) ? 3 : 0;
    },
    now: () => (clock += 1500),
  });
  return { ran, outcomes };
}

test('the steps are the npm and node scripts steps of ci.yml, in its order', () => {
  const workflow = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const ci = workflow.split(/\r?\n/).flatMap(line => {
    const match = /^\s*(?:- )?run: ((?:npm run |node scripts\/).+?)\s*$/.exec(line);
    return match ? [match[1]] : [];
  });
  assert.ok(ci.length > 0, 'ci.yml has no npm run or node scripts steps');
  assert.deepEqual(commands(plan({ quick: false, database: true })), ci);
});

test('--quick skips exactly bench:observe, test:db and test:browser', () => {
  const steps = plan({ quick: true, database: true });
  assert.deepEqual(commands(skipped(steps)), ['npm run bench:observe', 'npm run test:db', 'npm run test:browser']);
  assert.ok(skipped(steps).every(s => s.skip === 'skipped by --quick'));
});

test('test:db is skipped, never run, without SOKOMIND_TEST_DATABASE_URL', () => {
  assert.deepEqual(skipped(plan({ quick: false, database: false })), [
    { command: 'npm run test:db', skip: 'SOKOMIND_TEST_DATABASE_URL is not set' },
  ]);
  assert.deepEqual(skipped(plan({ quick: false, database: true })), []);
});

test('by default the steps after a failure are skipped, not run', () => {
  const { ran, outcomes } = fake(false, ['npm run a']);
  assert.deepEqual(ran, ['npm run a']);
  const lines = outcomes.map(o => `${o.result} ${o.command} ${o.note}`);
  assert.deepEqual(lines, [
    'FAIL npm run a exit code 3',
    'SKIP npm run b an earlier step failed; --keep-going runs every step',
    'SKIP npm run c skipped by --quick',
    'SKIP npm run d an earlier step failed; --keep-going runs every step',
  ]);
});

test('--keep-going runs every step that is not skipped', () => {
  const { ran, outcomes } = fake(true, ['npm run a']);
  assert.deepEqual(ran, ['npm run a', 'npm run b', 'npm run d']);
  assert.deepEqual(outcomes.map(o => o.result), ['FAIL', 'PASS', 'SKIP', 'PASS']);
});

test('the summary shows SKIP, never PASS, for a skipped step and counts every result', () => {
  const text = summary(fake(false, []).outcomes);
  assert.match(text, /^ {2}PASS {2}npm run a {2}1\.5s$/m);
  assert.match(text, /^ {2}SKIP {2}npm run c {2}skipped by --quick$/m);
  assert.match(text, /^Result: PASS - 3 passed, 0 failed, 1 skipped in 4\.5s$/m);
  assert.ok(/^[\x20-\x7e\n]*$/.test(text), 'ASCII only');
  const failed = summary(fake(false, ['npm run b']).outcomes);
  assert.match(failed, /^ {2}FAIL {2}npm run b {2}1\.5s, exit code 3$/m);
  assert.match(failed, /^Result: FAIL - 1 passed, 1 failed, 2 skipped in 3\.0s$/m);
});

test('durations read as seconds under a minute, then as minutes and seconds', () => {
  assert.equal(formatDuration(12_300), '12.3s');
  assert.equal(formatDuration(59_400), '59.4s');
  assert.equal(formatDuration(59_500), '1m 00s');
  assert.equal(formatDuration(185_000), '3m 05s');
});
