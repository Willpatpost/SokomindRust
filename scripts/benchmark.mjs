// Native catalog benchmark. Timings are observational; the gates judge only
// deterministic fields (see scripts/bench-gate.mjs for the rules).
//   npm run bench [-- <catalog example options>]   raw run, no gate
//   npm run bench:check                             gate against benchmarks/catalog-baseline.json
//   npm run bench:update [-- --states N --memory M --accept-regressions]
//                                                   rewrite the baseline after review
//   npm run bench:observe [-- --states N --memory M --repeat K --update]
//                                                   hard boards at OBSERVE's fixed size, median of K;
//                                                   --update rewrites benchmarks/observe-reference.json
//   npm run bench:appscale [-- --full]              the app's own declaration as a gate; --full, every board
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import {
  MODES,
  SCHEMA_VERSION,
  compare,
  formatReport,
  invariants,
  key,
  liveness,
  load,
  median,
  mustRecord,
  pairReference,
  scoreboard,
  scoreboardDelta,
  serialize,
  toCase,
} from './bench-gate.mjs';
import { MIB, catalog, catalogHash, nativeCorpus, sourceRevision } from './corpus.mjs';
import { root } from './toolchain.mjs';

/** @typedef {import('./corpus.mjs').CorpusRecord} CorpusRecord */
/** @typedef {import('./bench-gate.mjs').Case} Case */
/**
 * A committed observe-reference case: a case plus its median timings.
 * @typedef {Case & { search_us: number, first_route_us: number | null }} ObserveCase
 */

const BASELINE = resolve(root, 'benchmarks/catalog-baseline.json');
const REFERENCE = resolve(root, 'benchmarks/observe-reference.json');
// The command that rewrites each committed file, for load()'s errors.
const REGENERATE = { [BASELINE]: 'npm run bench:update', [REFERENCE]: 'npm run bench:observe -- --update' };
const DEFAULTS = { maxStates: 20_000, memoryMiB: 64 };
// bench:observe's fixed size, which observe-reference.json records: far below sokomind_search::MAX_STATES
// on purpose, so a run stays short. It is an observation size, not a cap; --states and --memory override it.
const OBSERVE = { maxStates: 1_000_000, memoryMiB: 64, repeat: 3, puzzles: ['huge', 'large', 'expert-maze', 'gen-v2-310081-a2088508'] };
// bench:appscale's declaration: the limits the page's every search runs under —
// protocol.ts's MAX_STATES ceiling, the memory and time selects' defaults. CI's
// fixed-size tiers cannot see behavior that appears only here: the walk term's
// old unsliced LP wedged every Optimal run at exactly these limits while both
// stayed green. Machine speed changes which routes and bounds a time-capped run
// reaches, so the tier judges only what speed cannot (liveness, in
// bench-gate.mjs) beside the false-proof invariants; the board set reaches
// every part of the engine — quick solves, the room bound and its deferred
// walk term at 64 MiB, the stage ladder, a board with no room structure, and
// the generated giants. --full runs every board, as the weekly CI job does.
const APPSCALE = {
  maxStates: 256_000_000,
  memoryMiB: 64,
  timeMs: 5_000,
  puzzles: ['tiny', 'medium', 'large', 'huge', 'expert-maze', 'open-field', 'gen-v2-340272-fd819d0e', 'gen-v2-370002-8ea0b852'],
};
const TIMINGS = ['sample', 'setup_us', 'first_route_us', 'search_us', 'reconstruct_us'];
const order = catalog.map(puzzle => puzzle.id);

const [action, ...rest] = process.argv.slice(2);
/**
 * A committed gate file, parsed and checked by load().
 * @param {string} path BASELINE or REFERENCE.
 */
const committed = path => load(JSON.parse(readFileSync(path, 'utf8')), relative(root, path), REGENERATE[path]);
/**
 * @param {string} text
 * @param {string} name
 */
const count = (text, name) => {
  const value = Number(text);
  assert(Number.isSafeInteger(value) && value > 0, `--${name} needs a positive integer`);
  return value;
};
/**
 * @template {import('node:util').ParseArgsOptionsConfig} T
 * @param {T} spec
 */
const options = spec => parseArgs({ args: rest, options: spec, strict: true, allowPositionals: false }).values;
/**
 * @param {{ maxStates: number, memoryMiB: number }} config
 * @param {string[]} [extra]
 */
const corpus = (config, extra = []) => nativeCorpus(['--states', String(config.maxStates), '--memory', String(config.memoryMiB), ...extra]);
/**
 * @param {string} name
 * @param {string} text
 */
function save(name, text) {
  mkdirSync(resolve(root, 'target/bench'), { recursive: true });
  writeFileSync(resolve(root, 'target/bench', name), text);
}
/**
 * Saves the records to target/bench/catalog.json and prints each mode's median search time, setup included.
 * @param {CorpusRecord[]} records
 */
function raw(records) {
  save('catalog.json', JSON.stringify({ catalogHash, records }, null, 2) + '\n');
  const medians = MODES.flatMap(mode => {
    const times = records.filter(r => r.mode === mode).map(r => r.search_us);
    return times.length ? [`${mode} ${(median(times) / 1000).toFixed(1)} ms`] : [];
  });
  console.log(`Median search time, setup included (observational): ${medians.join(', ') || 'no runs'}.`);
}
/** @param {import('./bench-gate.mjs').ReportFields} fields */
function report(fields) {
  save('report.txt', formatReport(fields).text + '\n');
  const result = formatReport({ ...fields, limit: 25 });
  console.log(result.text);
  return result.hard;
}

// Each action returns whether its gate passed; exitCode (not process.exit)
// lets Windows terminals flush the report first.
function check() {
  assert.equal(rest.length, 0, 'Baseline checks always use the reviewed configuration');
  const baseline = committed(BASELINE);
  const config = { maxStates: baseline.maxStates, memoryMiB: baseline.memoryMiB };
  // Before the build and the corpus run, so a changed catalog fails in seconds.
  assert.equal(catalogHash, baseline.catalogHash, 'Catalog changed: review, then run npm run bench:update');
  const records = corpus(config);
  raw(records);
  const cases = records.map(toCase);
  const failures = invariants(cases, baseline.cases);
  const diff = compare(baseline.cases, cases, { sameConfig: true });
  const stale = mustRecord(diff);
  console.table(scoreboard(cases));
  const hard = report({
    action: 'check',
    cases,
    config,
    diff,
    failures,
    baseline: baseline.sourceRevision,
    source: sourceRevision(),
    catalogChangesAreHard: true,
  });
  console.log('Raw measurements: target/bench/catalog.json; full report: target/bench/report.txt.');
  // Soft findings and unrecorded gains are reported, not failed: only a false
  // proof or bound, a hard regression or a changed catalog fails the gate.
  if (diff.soft.length) console.log(`Soft findings (${diff.soft.length}) are reported only; review them above.`);
  if (stale.length) {
    console.log(
      `Route, proof, status or bound improvements not in the baseline (${stale.length}): run npm run bench:update to hold later runs to them`,
    );
  }
  return !failures.length && !hard;
}

function update() {
  const values = options({ states: { type: 'string' }, memory: { type: 'string' }, 'accept-regressions': { type: 'boolean' } });
  const previous = existsSync(BASELINE) ? committed(BASELINE) : null;
  const config = {
    maxStates: values.states ? count(values.states, 'states') : (previous?.maxStates ?? DEFAULTS.maxStates),
    memoryMiB: values.memory ? count(values.memory, 'memory') : (previous?.memoryMiB ?? DEFAULTS.memoryMiB),
  };
  const records = corpus(config);
  raw(records);
  const cases = records.map(toCase);
  const failures = invariants(cases, previous?.cases);
  if (previous && previous.catalogHash !== catalogHash) {
    console.log(`Catalog changed since ${previous.sourceRevision}; new and removed cases are listed below.`);
  }
  const sameConfig = previous?.maxStates === config.maxStates && previous?.memoryMiB === config.memoryMiB;
  const diff = previous
    ? compare(previous.cases, cases, { sameConfig })
    : { hard: [], soft: [], improved: [], changed: [], missing: [], extra: [] };
  console.table(scoreboard(cases));
  if (previous) {
    console.log('Scoreboard change vs baseline:');
    console.table(scoreboardDelta(scoreboard(previous.cases), scoreboard(cases)));
  }
  const source = sourceRevision();
  report({ action: 'update', cases, config, diff, failures, baseline: previous?.sourceRevision, source });
  if (failures.length) {
    console.error('Invariant failures are soundness bugs and cannot be recorded.');
    return false;
  }
  if (diff.hard.length && !values['accept-regressions']) {
    console.error(`Refusing to record ${diff.hard.length} hard regressions; review them, then rerun with -- --accept-regressions.`);
    return false;
  }
  if (diff.hard.length) console.log(`*** ACCEPTED HARD REGRESSIONS (${diff.hard.length}); explain them in the commit message ***`);
  writeFileSync(BASELINE, serialize({ version: SCHEMA_VERSION, sourceRevision: source, catalogHash, ...config, cases }, order));
  console.log(`Wrote benchmarks/catalog-baseline.json (${cases.length} cases, ${config.maxStates}/${config.memoryMiB}, source ${source}).`);
  return true;
}

// Hard boards at OBSERVE's size, each searched --repeat times; the table
// shows the median timings. It fails on an invariant failure (the catalog
// baseline's and the committed reference's routes count as evidence), a crash,
// a board missing from the catalog, or repeats that differ in anything but
// timings. The deltas against the committed reference are for review and never
// fail it; a board edited since the reference was recorded gets none, and the
// summary line names it. --update records a new reference only when the
// invariants hold.
function observe() {
  const values = options({
    states: { type: 'string' },
    memory: { type: 'string' },
    repeat: { type: 'string' },
    update: { type: 'boolean' },
  });
  const config = {
    maxStates: values.states ? count(values.states, 'states') : OBSERVE.maxStates,
    memoryMiB: values.memory ? count(values.memory, 'memory') : OBSERVE.memoryMiB,
  };
  const repeat = values.repeat ? count(values.repeat, 'repeat') : OBSERVE.repeat;
  for (const id of OBSERVE.puzzles) assert(order.includes(id), `Observe board ${id} is not in the catalog`);
  // Before the corpus runs, so a file load() rejects fails in seconds.
  const baseline = committed(BASELINE);
  const reference = existsSync(REFERENCE) ? committed(REFERENCE) : null;
  /** @param {CorpusRecord} r */
  const strip = r => Object.fromEntries(Object.entries(r).filter(([name]) => !TIMINGS.includes(name)));
  /** @type {CorpusRecord[]} */
  const records = [];
  for (const id of OBSERVE.puzzles) {
    const samples = corpus(config, ['--puzzle', id, '--repeat', String(repeat)]);
    for (const mode of MODES) {
      const runs = samples.filter(r => r.mode === mode);
      assert.equal(runs.length, repeat, `${id}:${mode}: expected ${repeat} samples`);
      for (const r of runs.slice(1)) assert.deepEqual(strip(r), strip(runs[0]), `${id}:${mode}: nondeterministic search`);
      const firstRoutes = runs.map(r => r.first_route_us).filter(us => us !== null);
      records.push({
        ...runs[0],
        search_us: median(runs.map(r => r.search_us)),
        first_route_us: firstRoutes.length ? median(firstRoutes) : null,
      });
    }
  }
  const cases = records.map(r => ({ ...toCase(r), search_us: r.search_us, first_route_us: r.first_route_us }));
  const failures = invariants(cases, [...baseline.cases, ...(reference?.cases ?? [])]);
  const comparable = reference?.maxStates === config.maxStates && reference?.memoryMiB === config.memoryMiB;
  const { pairs, changed } = pairReference(cases, comparable ? /** @type {ObserveCase[]} */ (reference.cases) : []);
  /**
   * @param {number | null} now
   * @param {number | null} then
   */
  const delta = (now, then) => (now === null || then === null ? '' : now - then);
  console.table(
    cases.map(c => {
      const p = pairs.get(key(c));
      return {
        id: c.id,
        mode: c.mode,
        status: c.status,
        moves: c.moves,
        lb: c.lower_bound,
        proof: c.proof,
        expanded: c.expanded,
        generated: c.generated,
        firstRouteExpanded: c.first_route_expanded,
        reservedMiB: +(c.reserved_bytes / MIB).toFixed(1),
        searchMs: +(c.search_us / 1000).toFixed(1),
        ...(p
          ? {
              dMoves: delta(c.moves, p.moves),
              dLb: delta(c.lower_bound, p.lower_bound),
              dExpanded: delta(c.expanded, p.expanded),
              dSearchMs: +((c.search_us - p.search_us) / 1000).toFixed(1),
            }
          : {}),
      };
    }),
  );
  console.table(scoreboard(cases));
  for (const f of failures) console.log(`INVARIANT FAILURE  ${f.key}  ${f.rule}  ${f.message}`);
  const state = !reference ? 'none' : comparable ? reference.sourceRevision : 'config-differs';
  console.log(
    `OBSERVE v${SCHEMA_VERSION}: boards=${OBSERVE.puzzles.length} config=${config.maxStates}/${config.memoryMiB}`
      + ` invariants=${failures.length ? `FAIL(${failures.length})` : 'ok'} reference=${state}`
      + (changed.length ? ` changed-boards=${changed.join(',')}` : ''),
  );
  save('observe.json', JSON.stringify({ catalogHash, ...config, repeat, records }, null, 2) + '\n');
  if (failures.length) return false;
  if (values.update) {
    const source = sourceRevision();
    writeFileSync(
      REFERENCE,
      serialize(
        { version: SCHEMA_VERSION, sourceRevision: source, catalogHash, ...config, repeat, puzzles: OBSERVE.puzzles, cases },
        order,
      ),
    );
    console.log(`Wrote benchmarks/observe-reference.json (${cases.length} rows, source ${source}).`);
  }
  return true;
}

// The app-scale gate: every board of APPSCALE's set — the whole catalog with
// --full — searched in every mode at the app's declaration, each corpus spawn
// under a wall budget a wedged search cannot hide inside. It fails on a
// liveness or invariant failure or a spawn that does not finish; timings and
// counts stay observational, because machine speed moves them under a time
// budget. The baseline's solved routes stand as the invariants' evidence.
function appscale() {
  const values = options({
    full: { type: 'boolean' },
    states: { type: 'string' },
    memory: { type: 'string' },
    'time-ms': { type: 'string' },
  });
  const config = {
    maxStates: values.states ? count(values.states, 'states') : APPSCALE.maxStates,
    memoryMiB: values.memory ? count(values.memory, 'memory') : APPSCALE.memoryMiB,
    timeMs: values['time-ms'] ? count(values['time-ms'], 'time-ms') : APPSCALE.timeMs,
  };
  const puzzles = values.full ? order : APPSCALE.puzzles;
  for (const id of puzzles) assert(order.includes(id), `App-scale board ${id} is not in the catalog`);
  // Before the corpus runs, so a file load() rejects fails in seconds.
  const baseline = committed(BASELINE);
  assert.equal(catalogHash, baseline.catalogHash, 'Catalog changed: review, then run npm run bench:update');
  /** @type {CorpusRecord[]} */
  const records = [];
  for (const id of puzzles) {
    // One spawn per board, so a wedge is named to its board and cannot hide
    // inside another's wall time: every mode under the budget, the factor
    // liveness allows, and the run's start.
    try {
      const samples = nativeCorpus(
        ['--states', String(config.maxStates), '--memory', String(config.memoryMiB), '--time-ms', String(config.timeMs), '--puzzle', id],
        { timeoutMs: MODES.length * config.timeMs * 4 + 30_000 },
      );
      assert.equal(samples.length, MODES.length, `${id}: expected one record per mode`);
      records.push(...samples);
    } catch (error) {
      console.error(`APPSCALE  ${id}  ${error instanceof Error ? error.message : error}`);
      return false;
    }
  }
  const cases = records.map(toCase);
  const failures = [...liveness(records, config.timeMs), ...invariants(cases, baseline.cases)];
  console.table(
    records.map(r => ({
      id: r.id,
      mode: r.mode,
      status: r.status,
      moves: r.moves,
      lb: r.lower_bound,
      proof: r.proof.kind,
      searchMs: +(r.search_us / 1000).toFixed(1),
    })),
  );
  console.table(scoreboard(cases));
  for (const f of failures) console.log(`FAILURE  ${f.key}  ${f.rule}  ${f.message}`);
  console.log(
    `APPSCALE v${SCHEMA_VERSION}: boards=${puzzles.length} config=${config.maxStates}/${config.memoryMiB}/${config.timeMs}ms`
      + ` liveness+invariants=${failures.length ? `FAIL(${failures.length})` : 'ok'} source=${sourceRevision()}`,
  );
  save('appscale.json', JSON.stringify({ catalogHash, ...config, puzzles, records }, null, 2) + '\n');
  return !failures.length;
}

function passThrough() {
  const records = nativeCorpus(process.argv.slice(2));
  raw(records);
  console.table(scoreboard(records.map(toCase)));
  console.log('Raw measurements: target/bench/catalog.json; timings are observational and exclude compilation.');
  return true;
}

/** @type {Record<string, () => boolean>} */
const actions = { '--check': check, '--update': update, '--observe': observe, '--appscale': appscale };
if (!(actions[action] ?? passThrough)()) process.exitCode = 1;
