// The npm and node steps of CI's rust and integration jobs (.github/workflows/ci.yml),
// in the same order and with the same step env:, as one local command. npm ci, the
// node-floor job and the deploy job's Docker and version-copy checks run only in CI.
// It ends with a PASS/FAIL/SKIP line per step.
//   npm run validate                      stops at the first failure
//   npm run validate -- --keep-going      runs every step, then fails if any failed
//   npm run validate -- --quick           skips bench:observe, test:db and test:browser
// Windows PowerShell 5.1 drops a bare --, so quote it there: npm run validate '--' --quick
// test:db needs SOKOMIND_TEST_DATABASE_URL (README.md "Validation") and is skipped,
// never passed, without it. test:browser needs Playwright's Chromium once:
// npx playwright install chromium.
import { parseArgs } from 'node:util';
import { attempt } from './toolchain.mjs';

/**
 * One step: an npm script or node command line (split on spaces, so no quoting),
 * with the reason it is skipped, if it is, and the variables it runs with on top
 * of this process's environment (a step env: in ci.yml).
 * @typedef {{ command: string, skip?: string, env?: Readonly<Record<string, string>> }} Step
 */

/**
 * What happened to a step. `ms` is set for a step that ran; `note` holds a skip's
 * reason or a failure's exit code.
 * @typedef {{ command: string, result: 'PASS' | 'FAIL' | 'SKIP', ms?: number, note?: string }} Outcome
 */

const QUICK = 'skipped by --quick';
const STOPPED = 'an earlier step failed; --keep-going runs every step';

/**
 * The steps in ci.yml's order; validate.test.mjs checks that they match its run lines
 * and env: blocks.
 * @param {{ quick: boolean, database: boolean }} options database: SOKOMIND_TEST_DATABASE_URL is set.
 * @returns {Step[]}
 */
export function plan({ quick, database }) {
  /**
   * @param {string} command
   * @param {string} [skip]
   * @returns {Step}
   */
  const step = (command, skip) => (skip ? { command, skip } : { command });
  const slow = quick ? QUICK : undefined;
  return [
    step('npm run fmt:check'),
    step('npm run lint:rust'),
    // Without -D warnings, rustdoc reports a broken doc link and still passes.
    { command: 'npm run doc:rust', env: { RUSTDOCFLAGS: '-D warnings' } },
    step('npm run test:rust'),
    step('npm run test:release'),
    // The integration job's first step after npm ci.
    step('npm run format:check'),
    step('npm run test:web'),
    step('npm run test:scripts'),
    step('npm run check:scripts'),
    step('npm run bench:check'),
    // The only WASM build: `npm run build` and `npm run test:parity` would each
    // rebuild it, so their second halves run directly.
    step('npm run wasm'),
    step('npm run build:web'),
    // Reuses the native records bench:check just wrote instead of rerunning the corpus.
    step('node scripts/parity.mjs --native target/bench/catalog.json'),
    step('npm run bench:observe', slow),
    step('npm run test:db', slow ?? (database ? undefined : 'SOKOMIND_TEST_DATABASE_URL is not set')),
    step('npm run test:browser', slow),
  ];
}

/**
 * Runs the steps in order and times each one. Without keepGoing, every step after
 * a failure is skipped.
 * @param {readonly Step[]} steps
 * @param {{ keepGoing: boolean, run: (command: string, env?: Step['env']) => number, now: () => number }} options
 *   run starts a command with a step's env and returns its exit code; now reads a
 *   clock in milliseconds.
 * @returns {Outcome[]}
 */
export function execute(steps, { keepGoing, run, now }) {
  /** @type {Outcome[]} */
  const outcomes = [];
  let failed = false;
  for (const { command, skip, env } of steps) {
    if (skip || (failed && !keepGoing)) {
      outcomes.push({ command, result: 'SKIP', note: skip || STOPPED });
      continue;
    }
    const start = now();
    const code = run(command, env);
    const ms = now() - start;
    if (code === 0) {
      outcomes.push({ command, result: 'PASS', ms });
    } else {
      failed = true;
      outcomes.push({ command, result: 'FAIL', ms, note: `exit code ${code}` });
    }
  }
  return outcomes;
}

/**
 * 12.3s under a minute, otherwise 3m 05s.
 * @param {number} ms
 */
export function formatDuration(ms) {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, '0')}s`;
}

/**
 * The closing report: one line per step, then the overall result.
 * @param {readonly Outcome[]} outcomes
 * @returns {string}
 */
export function summary(outcomes) {
  const width = Math.max(...outcomes.map(o => o.command.length));
  /** @param {Outcome['result']} result */
  const count = result => outcomes.filter(o => o.result === result).length;
  /** @param {Outcome} o */
  const detail = o => [o.ms === undefined ? '' : formatDuration(o.ms), o.note ?? ''].filter(Boolean).join(', ');
  const total = outcomes.reduce((sum, o) => sum + (o.ms ?? 0), 0);
  const failed = count('FAIL');
  return [
    'Validation summary:',
    ...outcomes.map(o => `  ${o.result}  ${o.command.padEnd(width)}  ${detail(o)}`),
    `Result: ${failed ? 'FAIL' : 'PASS'} - ${count('PASS')} passed, ${failed} failed, ${count('SKIP')} skipped in ${formatDuration(total)}`,
  ].join('\n');
}

const USAGE = 'Usage: npm run validate [-- [--quick] [--keep-going]]';

/**
 * Runs the plan for this command line and prints the summary.
 * @returns {number} The exit code.
 */
function main() {
  /** @type {{ quick?: boolean, 'keep-going'?: boolean }} */
  let options;
  try {
    options = parseArgs({
      options: { quick: { type: 'boolean' }, 'keep-going': { type: 'boolean' } },
      strict: true,
      allowPositionals: false,
    }).values;
  } catch (error) {
    console.error(`${/** @type {Error} */ (error).message}\n${USAGE}`);
    return 2;
  }
  // npm scripts run through the npm that started this, as node <npm-cli.js>:
  // Windows cannot spawn npm.cmd without a shell.
  const npm = process.env.npm_execpath;
  if (!npm) {
    console.error(`npm_execpath is not set; run this through npm.\n${USAGE}`);
    return 2;
  }
  /**
   * @param {string} command
   * @param {Step['env']} env
   */
  const run = (command, env) => {
    const set = Object.entries(env ?? {}).map(([key, value]) => `${key}=${value}`);
    console.log(`\n> ${command}${set.length ? ` (with ${set.join(', ')})` : ''}`);
    const [program, ...args] = command.split(' ');
    if (program !== 'npm' && program !== 'node') throw new Error(`Step ${command} must start with npm or node`);
    return attempt(process.execPath, program === 'npm' ? [npm, ...args] : args, env);
  };
  const steps = plan({ quick: !!options.quick, database: !!process.env.SOKOMIND_TEST_DATABASE_URL });
  const outcomes = execute(steps, { keepGoing: !!options['keep-going'], run, now: () => performance.now() });
  console.log(`\n${summary(outcomes)}`);
  return outcomes.some(o => o.result === 'FAIL') ? 1 : 0;
}

// Only when run directly: validate.test.mjs imports the functions above. Node
// releases older than package.json's engines lack import.meta.main, and would
// otherwise skip main() and exit 0 as if every step had passed.
if (import.meta.main === undefined) throw new Error(`Node ${process.version} is older than package.json's engines (>=22.18.0)`);
if (import.meta.main) process.exitCode = main();
