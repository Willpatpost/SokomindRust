import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compare, formatReport, invariants, load, mustRecord, scoreboard, scoreboardDelta, serialize, toCase } from './bench-gate.mjs';

/** @typedef {import('./bench-gate.mjs').Case} Case */
/** @typedef {import('./bench-gate.mjs').Finding} Finding */

const full = (overrides = {}) => ({
  id: 'a', mode: 'optimal', fingerprint: 'fa', status: 'solved', moves: 10, pushes: 4, proof: 'optimal', lower_bound: 10,
  expanded: 100, generated: 200, reserved_bytes: 4096, first_route_expanded: 90, first_route_generated: 180,
  stats: { unique_states: 150, duplicate_improvements: 3 }, ...overrides,
});
const fast = (overrides = {}) => full({ mode: 'fast', proof: 'none', lower_bound: null, ...overrides });
/**
 * @param {Case} before
 * @param {Case} after
 */
const diff = (before, after, sameConfig = true) => compare([before], [after], { sameConfig });
/** @param {readonly Finding[]} findings */
const rules = findings => findings.map(f => `${f.rule}:${f.field}`);
/** @param {readonly Finding[]} findings */
const fields = findings => findings.map(f => f.field);
const CONFIG = { maxStates: 20000, memoryMiB: 64 };

test('serialize is deterministic: sorted keys, catalog then mode order, one case per line, LF', () => {
  const cases = [full({ id: 'b', mode: 'fast' }), full({ id: 'a' }), full({ id: 'b' }), fast({ id: 'a' })];
  const text = serialize({ version: 2, sourceRevision: 'abc', catalogHash: 'h', maxStates: 20000, memoryMiB: 64, cases }, ['b', 'a']);
  const shuffled = cases.map(c => Object.fromEntries(Object.entries(c).reverse())).reverse();
  assert.equal(serialize({ cases: shuffled, memoryMiB: 64, maxStates: 20000, catalogHash: 'h', sourceRevision: 'abc', version: 2 }, ['b', 'a']), text);
  assert.ok(text.endsWith('}\n') && !text.includes('\r'));
  const lines = text.split('\n');
  assert.equal(lines.length, 1 + 5 + 1 + cases.length + 2 + 1);
  assert.deepEqual(lines.slice(1, 6).map(line => line.trim().split('"')[1]),
    ['catalogHash', 'maxStates', 'memoryMiB', 'sourceRevision', 'version']);
  /** @type {{ cases: Record<string, unknown>[] }} */
  const parsed = JSON.parse(text);
  assert.deepEqual(parsed.cases.map(c => `${c.id}:${c.mode}`), ['b:fast', 'b:optimal', 'a:fast', 'a:optimal']);
  assert.deepEqual(Object.keys(parsed.cases[0]), Object.keys(parsed.cases[0]).sort());
});

test('R1 and R2: a worse or lost route and a lost proof are hard', () => {
  assert.deepEqual(rules(diff(fast(), fast({ moves: 12 })).hard), ['R1:moves']);
  const lost = diff(fast(), fast({ moves: null, pushes: null, status: 'state_limit' }));
  assert.deepEqual(rules(lost.hard), ['R1:moves']);
  assert.deepEqual(rules(lost.soft), ['S3:status']);
  assert.deepEqual(rules(diff(full(), full({ status: 'state_limit', proof: 'bounded', lower_bound: 9 })).hard),
    ['R2:proof', 'R3:status', 'R4:lower_bound']);
  const unsolvable = full({ status: 'exhausted', moves: null, pushes: null, proof: 'unsolvable', lower_bound: null });
  assert.deepEqual(rules(diff(unsolvable, { ...unsolvable, status: 'state_limit', proof: 'none', lower_bound: 30 }).hard),
    ['R2:proof', 'R3:status']);
});

test('R4: a capped Optimal lower bound never falls unless the run is now proven unsolvable', () => {
  const capped = full({ status: 'state_limit', moves: null, pushes: null, proof: 'none', lower_bound: 8 });
  assert.deepEqual(rules(diff(capped, { ...capped, lower_bound: 7 }).hard), ['R4:lower_bound']);
  assert.deepEqual(rules(diff(capped, { ...capped, lower_bound: null }).hard), ['R4:lower_bound']);
  const raised = diff(capped, { ...capped, lower_bound: 9 });
  assert.deepEqual(raised.hard, []);
  assert.deepEqual(rules(raised.improved), ['R4:lower_bound']);
  const proven = diff(capped, { ...capped, status: 'exhausted', proof: 'unsolvable', lower_bound: null });
  assert.deepEqual(proven.hard, []);
  assert.deepEqual(rules(proven.improved), ['R2:proof', 'R3:status']);
});

test('R3 is hard for Optimal while the same status drop is soft (S3) for Fast and Quality', () => {
  const bounded = full({ proof: 'bounded', lower_bound: 9 });
  const optimal = diff(bounded, { ...bounded, status: 'state_limit' });
  assert.deepEqual(rules(optimal.hard), ['R3:status']);
  assert.deepEqual(optimal.soft, []);
  for (const mode of ['fast', 'quality']) {
    const result = diff(fast({ mode }), fast({ mode, status: 'state_limit' }));
    assert.deepEqual(result.hard, [], mode);
    assert.deepEqual(rules(result.soft), ['S3:status'], mode);
  }
  const capped = full({ status: 'state_limit', moves: null, pushes: null, proof: 'none', lower_bound: 5 });
  const sameRank = diff(capped, { ...capped, status: 'memory_limit' });
  assert.deepEqual([...sameRank.hard, ...sameRank.soft], []);
  assert.deepEqual(fields(sameRank.changed), ['status']);
});

test('S1 judges node counts only when both runs finished', () => {
  // expanded may grow to max(100 + 32, 120) = 132 and generated to max(232, 240) = 240.
  const grown = diff(fast(), fast({ expanded: 133, generated: 240 }));
  assert.deepEqual(rules(grown.soft), ['S1:expanded']);
  assert.deepEqual(fields(grown.changed), ['generated']);
  assert.deepEqual(diff(fast({ status: 'state_limit' }), fast({ status: 'state_limit', expanded: 1000 })).soft, []);
});

test('S2 flags accounted-memory growth only at the same config', () => {
  assert.deepEqual(rules(diff(fast(), fast({ reserved_bytes: 8192 })).soft), ['S2:reserved_bytes']);
  const otherConfig = diff(fast(), fast({ reserved_bytes: 8192 }), false);
  assert.deepEqual(otherConfig.soft, []);
  assert.deepEqual(fields(otherConfig.changed), ['reserved_bytes']);
  assert.deepEqual(rules(diff(fast(), fast({ reserved_bytes: 2048 })).improved), ['S2:reserved_bytes']);
});

test('toCase keeps every field present', () => {
  // Deliberately partial: toCase must still set every field, to null when the record lacks it.
  const partial = { id: 'a', mode: 'fast', sample: 0, route: 'DD', search_us: 5, proof: { kind: 'none' }, moves: 2,
    stats: { unique_states: 1 } };
  const record = toCase(/** @type {any} */ (partial));
  assert.equal(record.proof, 'none');
  assert.equal(record.first_route_expanded, null);
  assert.ok(!('route' in record) && !('search_us' in record) && !('sample' in record));
  assert.ok(Object.values(record).every(value => value !== undefined));
  // The fixtures carry exactly the keys toCase writes, so the load test below covers every one.
  assert.deepEqual(Object.keys(record).sort(), Object.keys(full()).sort());
});

test('load accepts only the current schema, with every case key and a string fingerprint', () => {
  /** @param {unknown[]} cases */
  const file = (cases, version = 2) => ({ version, sourceRevision: 'abc', catalogHash: 'h', maxStates: 20000, memoryMiB: 64, cases });
  /** @param {unknown} parsed */
  const loaded = parsed => load(parsed, 'baseline.json', 'npm run bench:update');
  const current = file([full(), fast()]);
  assert.equal(loaded(current), current);
  assert.throws(() => loaded(file([full()], 1)), {
    message: 'baseline.json has schema version 1, not 2. Regenerate the file: delete it, then run npm run bench:update.',
  });
  assert.throws(() => loaded(file([full()], 3)), /schema version 3, not 2/);
  assert.throws(() => loaded({ version: 2 }), /has no cases array/);
  /** @param {string} name */
  const without = name => Object.fromEntries(Object.entries(fast()).filter(([field]) => field !== name));
  assert.throws(() => loaded(file([full(), without('fingerprint')])), {
    message: 'baseline.json: case a:fast lacks fingerprint. Regenerate the file: delete it, then run npm run bench:update.',
  });
  assert.throws(() => loaded(file([fast({ fingerprint: null })])), /case a:fast lacks fingerprint\./);
  for (const name of Object.keys(fast())) assert.throws(() => loaded(file([without(name)])), new RegExp(`lacks ${name}\\.`), name);
});

test('invariants I1-I3 catch false proofs and bounds, using only matching evidence', () => {
  /**
   * @param {readonly Case[]} cases
   * @param {readonly Case[]} [evidence]
   */
  const found = (cases, evidence) => invariants(cases, evidence).map(f => `${f.key} ${f.rule}`);
  assert.deepEqual(found([full(), fast(), fast({ mode: 'quality' })]), []);
  assert.deepEqual(found([fast({ proof: 'optimal' })]), ['a:fast I1']);
  assert.deepEqual(found([fast({ lower_bound: 3 })]), ['a:fast I1']);
  assert.deepEqual(found([full({ lower_bound: 9 })]), ['a:optimal I2']);
  assert.deepEqual(found([full({ proof: 'bounded' })]), ['a:optimal I2']);
  assert.deepEqual(found([full({ proof: 'none' })]), ['a:optimal I2']);
  assert.deepEqual(found([full({ status: 'state_limit', moves: null, pushes: null, proof: 'unsolvable', lower_bound: null })]), ['a:optimal I2']);
  // A shorter Fast route refutes both the bound and the optimality claim.
  assert.deepEqual(found([full(), fast({ moves: 9 })]), ['a:optimal I3', 'a:optimal I3']);
  const capped = full({ status: 'state_limit', moves: null, pushes: null, proof: 'none', lower_bound: 13 });
  assert.deepEqual(found([capped], [fast({ mode: 'quality', moves: 12 })]), ['a:optimal I3']);
  const proven = full({ status: 'exhausted', moves: null, pushes: null, proof: 'unsolvable', lower_bound: null });
  assert.deepEqual(found([proven]), []);
  assert.deepEqual(found([proven], [fast({ moves: 12 })]), ['a:optimal I3']);
  assert.deepEqual(found([proven], [fast({ fingerprint: 'changed', moves: 12 })]), []);
  assert.deepEqual(found([proven], [fast({ id: 'b', moves: 12 })]), []);
  assert.deepEqual(found([proven], [fast({ moves: null })]), []);
});

test('R1-R4 and S3 improvements must be recorded, S1 and S2 ones are informational; catalog changes are hard only for check', () => {
  const before = full({ status: 'state_limit', moves: 12, proof: 'bounded', lower_bound: 8, expanded: 20000 });
  const after = full({ expanded: 900, generated: 150, reserved_bytes: 2048 });
  const result = compare([before], [after], { sameConfig: true });
  assert.deepEqual([...result.hard, ...result.soft], []);
  assert.deepEqual(rules(result.improved), ['R1:moves', 'R2:proof', 'R3:status', 'R4:lower_bound', 'S2:reserved_bytes']);
  assert.deepEqual(fields(result.changed), ['expanded', 'generated']);
  assert.deepEqual(rules(mustRecord(result)), ['R1:moves', 'R2:proof', 'R3:status', 'R4:lower_bound']);
  const earlier = [fast({ status: 'state_limit' }), fast({ id: 'b', expanded: 500 })];
  const gains = compare(earlier, [fast(), fast({ id: 'b' })], { sameConfig: true });
  assert.deepEqual(rules(gains.improved), ['S3:status', 'S1:expanded']);
  assert.deepEqual(rules(mustRecord(gains)), ['S3:status']);
  /** @param {import('./bench-gate.mjs').Diff} diff */
  const report = diff => formatReport({ action: 'check', cases: [after], config: CONFIG, diff, failures: [],
    baseline: 'abc', source: 'def', catalogChangesAreHard: true });
  const { text, hard } = report(result);
  assert.equal(hard, 0);
  assert.match(
    text,
    /^BENCH check v2: cases=1 config=20000\/64 hard=0 soft=0 improved=5 changed=2 invariants=ok baseline=abc source=def$/m,
  );
  const removed = compare([before], [], { sameConfig: true });
  assert.deepEqual(removed.missing, ['a:optimal']);
  assert.equal(report(removed).hard, 1);
  assert.equal(formatReport({ action: 'update', cases: [], config: CONFIG, diff: removed, failures: [], source: 'def' }).hard, 0);
});

test('scoreboard counts routes, proofs, capped lower bounds and records per proof', () => {
  /**
   * @param {string} id
   * @param {string} mode
   */
  const at = (id, mode, overrides = {}) => full({ id, mode, proof: 'none', lower_bound: null,
    stats: { unique_states: 10, duplicate_improvements: 1 }, ...overrides });
  const cases = [
    at('a', 'fast'), at('a', 'quality'), at('a', 'optimal', { proof: 'optimal', lower_bound: 10 }),
    at('b', 'fast', { moves: 20, expanded: 50, generated: 80 }),
    at('b', 'quality', { status: 'state_limit', moves: 22, expanded: 20000, generated: 30000 }),
    at('b', 'optimal', { status: 'state_limit', moves: null, lower_bound: 15, expanded: 20000, generated: 30000 }),
    at('c', 'fast', { status: 'exhausted', moves: null, expanded: 5, generated: 6 }),
    at('c', 'quality', { status: 'exhausted', moves: null, expanded: 5, generated: 6 }),
    at('c', 'optimal', { status: 'exhausted', moves: null, proof: 'unsolvable', expanded: 5, generated: 6 }),
  ];
  const board = scoreboard(cases);
  const common = { runs: 3, uniqueStates: 30, duplicateImprovements: 3 };
  assert.deepEqual(board, [
    { mode: 'fast', ...common, routes: 2, finished: 3, capped: 0, proofs: 0, movesSum: 30, cappedLowerBoundSum: 0,
      uncappedExpanded: 155, uncappedGenerated: 286, recordsPerProof: null },
    { mode: 'quality', ...common, routes: 2, finished: 2, capped: 1, proofs: 0, movesSum: 32, cappedLowerBoundSum: 0,
      uncappedExpanded: 105, uncappedGenerated: 206, recordsPerProof: null },
    { mode: 'optimal', ...common, routes: 1, finished: 2, capped: 1, proofs: 2, movesSum: 10, cappedLowerBoundSum: 15,
      uncappedExpanded: 105, uncappedGenerated: 206, recordsPerProof: 103 },
  ]);
  const delta = scoreboardDelta(board, board);
  assert.equal(delta[2].mode, 'optimal');
  assert.equal(delta[2].movesSum, 0);
  assert.equal(delta[0].recordsPerProof, null);
});
