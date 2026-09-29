// Temporary factorial measurement of the Phase 5 experiments, each behind an
// off-by-default sokomind-search feature: o2 (5.2), o3b (5.6), pea0/pea1/pea2
// (5.3, C = 0/1/2) and o6 (5.5); 5.4 has no switch. Deleted with the features
// once the adopted experiments are switched on for good.
//   node scripts/factorial.mjs [--quick] [--fresh] [--pre <commit before 5.4>]
// Every build compiles the catalog example with one feature set and runs all
// boards and modes at 20k/64 MiB and at 1M/64 MiB (three samples, timings are
// medians; --quick runs 20k only). Raw records go to
// target/bench/factorial/<build>-<config>.jsonl and are reused by a rerun at
// the same clean HEAD with the same configs unless --fresh. The report (also
// report.txt there) ends with the adoption decisions made by the rule fixed below.
// --pre builds that commit in a temporary git worktree (target/bench/factorial/pre,
// its own target directory pre-target beside it) and runs it at 20k and on the
// memory-bound boards next to the base build: 5.4's identity and capacity
// criteria (it has no switch, so only an older build shows them).
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { invariants, key, toCase, upgrade } from './bench-gate.mjs';
import { sourceRevision } from './corpus.mjs';
import { env, root } from './toolchain.mjs';

const flags = parseArgs({ options: { quick: { type: 'boolean' }, fresh: { type: 'boolean' }, pre: { type: 'string' } },
  strict: true }).values;
const ORDER = ['o2', 'o3b', 'pea0', 'pea1', 'pea2', 'o6'];
const PEA = ['pea0', 'pea1', 'pea2'];
const label = features => [...new Set(features)].sort((a, b) => ORDER.indexOf(a) - ORDER.indexOf(b)).join('+') || 'base';
// PEA* changes only Exact and o6 only Fast/Quality, so {pea0,o6} measures
// both at once. The lone pea0 build checks that claim (see independence()).
const BUILDS = [];
for (const o2 of [[], ['o2']]) {
  for (const o3b of [[], ['o3b']]) {
    for (const variant of [[], ['pea0', 'o6'], ['pea1'], ['pea2']]) BUILDS.push([...o2, ...o3b, ...variant]);
  }
}
BUILDS.push(['pea0']);
const CONFIGS = [
  { name: '20k', states: 20_000, memory: 64, repeat: 1 },
  { name: '1M', states: 1_000_000, memory: 64, repeat: 3 },
].slice(0, flags.quick ? 1 : 2);
// 5.4's capacity boards: memory binds on them at 1M states, and huge is still
// memory-bound after 5.4 only at 48 MiB (5.4 spec, section 8).
const CAPACITY = [['huge', 48], ['gen-v2-370002-8ea0b852', 64], ['gen-v2-360313-16158b3b', 64], ['gen-v2-350001-a996dcbc', 64]]
  .map(([puzzle, memory]) => ({ name: `cap-${puzzle}`, puzzle, states: 1_000_000, memory, repeat: 1 }));
const MODES_OF = { o2: ['fast', 'quality', 'optimal'], o3b: ['fast', 'quality', 'optimal'], o6: ['fast', 'quality'],
  pea0: ['optimal'], pea1: ['optimal'], pea2: ['optimal'] };

const dir = resolve(root, 'target/bench/factorial');
const source = sourceRevision();
const TIMINGS = new Set(['sample', 'setup_us', 'first_route_us', 'search_us', 'reconstruct_us']);
const PROVEN = new Set(['optimal', 'unsolvable']);
const median = list => list.length ? [...list].sort((a, b) => a - b)[Math.floor(list.length / 2)] : null;
mkdirSync(dir, { recursive: true });

/** Per-key cases with the route and median search time; counts keys whose samples disagree. */
function collapse(text) {
  const groups = new Map();
  for (const line of text.trim().split(/\r?\n/)) {
    const r = JSON.parse(line);
    groups.set(key(r), [...(groups.get(key(r)) ?? []), r]);
  }
  const strip = r => JSON.stringify(Object.entries(r).filter(([name]) => !TIMINGS.has(name)));
  const cases = new Map();
  let unstable = 0;
  for (const [k, runs] of groups) {
    if (runs.some(r => strip(r) !== strip(runs[0]))) unstable++;
    // toCase keeps the committed fields; pea builds also report reexpanded (5.3).
    const extra = runs[0].reexpanded === undefined ? {} : { reexpanded: runs[0].reexpanded };
    cases.set(k, { ...toCase(runs[0]), ...extra, route: runs[0].route, search_us: median(runs.map(r => r.search_us)) });
  }
  return { cases, unstable };
}

const git = (args, cwd = root) => spawnSync('git', args, { cwd, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
const signature = c => `${c.states}/${c.memory}x${c.repeat}${c.puzzle ? ` ${c.puzzle}` : ''}`;
/** Whether `name` has records of every config, all measured at `revision` (a clean commit). */
function reusable(name, revision, configs) {
  if (flags.fresh || revision === 'unknown' || revision.endsWith('+dirty')) return false;
  try {
    const meta = JSON.parse(readFileSync(resolve(dir, `${name}.meta.json`), 'utf8'));
    return meta.source === revision
      && configs.every(c => meta.configs?.[c.name] === signature(c) && existsSync(resolve(dir, `${name}-${c.name}.jsonl`)));
  } catch {
    return false;
  }
}
/** Build one feature set (in `tree`) and run every config: { [config]: run } or { error }. */
function measure(features, { name = label(features), tree = root, revision = source, configs = CONFIGS, target } = {}) {
  const meta = resolve(dir, `${name}.meta.json`), files = configs.map(c => resolve(dir, `${name}-${c.name}.jsonl`));
  if (!reusable(name, revision, configs)) {
    console.log(`=== ${name}: build ===`);
    rmSync(meta, { force: true });
    // `target` (the pre build's own directory) keeps it from replacing this tree's binary.
    const exe = resolve(tree, target ?? env.CARGO_TARGET_DIR ?? 'target',
      'release/examples/catalog' + (process.platform === 'win32' ? '.exe' : ''));
    const built = spawnSync('cargo', ['build', '--locked', '--release', '-p', 'sokomind-search', '--example', 'catalog',
      ...(features.length ? ['--features', features.join(',')] : [])],
    { cwd: tree, env: target ? { ...env, CARGO_TARGET_DIR: target } : env, stdio: 'inherit' });
    if (built.error || built.status !== 0) return { error: `build failed${built.error ? `: ${built.error.message}` : ''}` };
    for (const [i, c] of configs.entries()) {
      console.log(`=== ${name}: ${c.name} x${c.repeat} ===`);
      const run = spawnSync(exe, ['--states', String(c.states), '--memory', String(c.memory), '--repeat', String(c.repeat),
        ...(c.puzzle ? ['--puzzle', c.puzzle] : [])], { cwd: tree, env, encoding: 'utf8', maxBuffer: 512 * 1024 * 1024 });
      if (run.error || run.status !== 0) {
        return { error: `${c.name} run failed: ${(run.error?.message ?? run.stderr ?? '').trim().split(/\r?\n/).pop()}` };
      }
      writeFileSync(files[i], run.stdout);
    }
    const configsMeta = Object.fromEntries(configs.map(c => [c.name, signature(c)]));
    writeFileSync(meta, JSON.stringify({ source: revision, features, configs: configsMeta }) + '\n');
  }
  return Object.fromEntries(configs.map((c, i) => [c.name, collapse(readFileSync(files[i], 'utf8'))]));
}
/** Removes the pre worktree even when a stopped run left it locked, half added, or deleted but registered. */
function dropWorktree(tree) {
  git(['worktree', 'remove', '--force', '--force', tree]);
  try {
    rmSync(tree, { recursive: true, force: true, maxRetries: 3 });
  } catch (error) {
    console.warn(`factorial: could not delete ${tree}: ${error.message}`);
  }
}
/** The --pre commit's 20k and capacity runs, built in a worktree that is removed afterwards. */
function measurePre(revision) {
  const sha = git(['rev-parse', '--short=12', `${revision}^{commit}`]);
  if (sha.error || sha.status !== 0) return { error: `--pre ${revision} is not a commit` };
  const commit = sha.stdout.trim(), tree = resolve(dir, 'pre'), configs = [CONFIGS[0], ...CAPACITY];
  const options = { name: 'pre', tree, revision: commit, configs, target: resolve(dir, 'pre-target') };
  if (reusable('pre', commit, configs)) return measure([], options);
  dropWorktree(tree);
  // --force twice also takes over a path still registered to a missing (or locked) worktree.
  const added = git(['worktree', 'add', '--force', '--force', '--detach', tree, commit]);
  if (added.error || added.status !== 0) return { error: `git worktree add failed: ${(added.stderr ?? '').trim()}` };
  try {
    return measure([], options);
  } finally {
    dropWorktree(tree);
  }
}

const results = new Map(BUILDS.map(features => [label(features),
  measure(features, features.length || !flags.pre ? {} : { configs: [...CONFIGS, ...CAPACITY] })]));
const pre = flags.pre ? measurePre(flags.pre) : null;
const lines = [];
const say = (...parts) => lines.push(parts.join(' '));
const read = path => JSON.parse(readFileSync(resolve(root, path), 'utf8'));
const baseline = read('benchmarks/catalog-baseline.json'), reference = read('benchmarks/observe-reference.json');
const runs = [...results.values(), ...(pre ? [pre] : [])].filter(r => !r.error).flatMap(r => Object.values(r));
const fingerprints = new Map(runs.flatMap(r => [...r.cases.values()]).map(c => [c.id, c.fingerprint]));
// Best replay-verified moves per board over every build, config and committed
// file: the fixed reference for capped lower-bound gaps and the evidence for I3.
const evidence = [...baseline.cases, ...reference.cases, ...runs.flatMap(r => [...r.cases.values()])]
  .filter(c => fingerprints.get(c.id) === c.fingerprint);
const best = new Map();
for (const c of evidence) if (c.moves !== null && !(best.get(c.id) <= c.moves)) best.set(c.id, c.moves);

const get = (name, config) => { const r = results.get(name); return r && !r.error ? r[config] ?? null : null; };
/** The build that measures `features` in `mode`: PEA* never touches Fast/Quality, o6 never touches Optimal. */
function resolveBuild(features, mode) {
  const set = new Set(features);
  if (mode === 'optimal') {
    set.delete('o6');
    if (set.has('pea0')) set.add('o6');
  } else {
    for (const p of PEA) set.delete(p);
    if (set.has('o6')) set.add('pea0');
  }
  return label([...set]);
}
const pairs = (a, b, mode) => [...a.cases.values()].filter(c => c.mode === mode).map(c => [c, b.cases.get(key(c))]).filter(([, t]) => t);
/** Equal apart from time and layout; reexpanded counts only when both builds report it (both pea). */
const same = (c, t) => {
  const both = 'reexpanded' in c && 'reexpanded' in t;
  const norm = x => ({ ...x, search_us: 0, reserved_bytes: 0, reexpanded: both ? x.reexpanded : undefined });
  return JSON.stringify(norm(c)) === JSON.stringify(norm(t));
};
/** Nodes a build expanded, re-expansions included (pea builds report them). */
const work = c => c.expanded + (c.reexpanded ?? 0);
const count = x => x === null ? '-' : x >= 1e6 ? `${(x / 1e6).toFixed(1)}M` : x >= 1e4 ? `${(x / 1e3).toFixed(0)}k` : String(x);

function optimalStats(a, b) {
  const rows = pairs(a, b, 'optimal');
  const capped = rows.filter(([c]) => !PROVEN.has(c.proof) && best.has(c.id));
  const gap = side => median(capped.map(pair => best.get(pair[0].id) - (pair[side].lower_bound ?? 0)));
  const both = rows.filter(([c, t]) => c.proof === 'optimal' && t.proof === 'optimal');
  const timed = rows.filter(([c, t]) => c.generated >= 1000 && t.generated >= 1000);
  const solved = rows.filter(([c, t]) => c.status === 'solved' && t.status === 'solved');
  const reexpanded = side => rows.some(pair => 'reexpanded' in pair[side])
    ? rows.reduce((sum, pair) => sum + (pair[side].reexpanded ?? 0), 0) : null;
  return {
    // expanded + reexpanded over rows both builds solve; reexpanded summed over all rows (null: not a pea build).
    work: [solved.reduce((sum, [c]) => sum + work(c), 0), solved.reduce((sum, [, t]) => sum + work(t), 0)], n_work: solved.length,
    reexpanded: [reexpanded(0), reexpanded(1)],
    proofs: [rows.filter(([c]) => PROVEN.has(c.proof)).length, rows.filter(([, t]) => PROVEN.has(t.proof)).length],
    gap: [gap(0), gap(1)], n: capped.length,
    fell: rows.filter(([c, t]) => t.proof !== 'unsolvable' && (t.lower_bound ?? 0) < (c.lower_bound ?? 0))
      .map(([c, t]) => `${c.id} ${c.lower_bound}->${t.lower_bound}`),
    lost: rows.filter(([c, t]) => PROVEN.has(c.proof) && !PROVEN.has(t.proof)).map(([c]) => c.id),
    mismatch: both.filter(([c, t]) => c.moves !== t.moves).map(([c, t]) => `${c.id} ${c.moves}->${t.moves}`),
    rpp: median(both.map(([c, t]) => t.generated / c.generated)),
    tpr: median(timed.map(([c, t]) => (t.search_us / t.generated) / (c.search_us / c.generated))),
    identical: rows.filter(([c, t]) => same(c, t)).length, n_all: rows.length,
  };
}
function routeStats(a, b, mode) {
  const rows = pairs(a, b, mode);
  const both = rows.filter(([c, t]) => c.moves !== null && t.moves !== null);
  return {
    routes: [rows.filter(([c]) => c.moves !== null).length, rows.filter(([, t]) => t.moves !== null).length],
    worse: both.filter(([c, t]) => t.moves > c.moves).map(([c, t]) => `${c.id} ${c.moves}->${t.moves}`),
    better: both.filter(([c, t]) => t.moves < c.moves).length,
    lost: rows.filter(([c, t]) => c.moves !== null && t.moves === null).map(([c]) => c.id),
    gained: rows.filter(([c, t]) => c.moves === null && t.moves !== null).length,
    records: median(rows.filter(([c, t]) => c.status === 'solved' && t.status === 'solved').map(([c, t]) => t.generated / c.generated)),
    identical: rows.filter(([c, t]) => same(c, t)).length, n_all: rows.length,
  };
}
const f2 = x => x === null || x === undefined ? '-' : x.toFixed(2);
const list = (items, limit = 8) => items.length > limit ? `${items.slice(0, limit).join(', ')}, +${items.length - limit}` : items.join(', ');

say(`FACTORIAL v1 source=${source} builds=${BUILDS.length} configs=${CONFIGS.map(c => `${c.states}/${c.memory}x${c.repeat}`).join(',')}`);
const failed = [...results, ...(pre ? [['pre', pre]] : [])].filter(([, r]) => r.error);
say(`failures: ${failed.length ? failed.map(([name, r]) => `${name} (${r.error})`).join('; ') : 'none'}`);

// Per build vs base at the same config.
// rexp: Optimal re-expansions (pea builds only); work: expanded + reexpanded vs base over Optimal rows both solve.
say('build'.padEnd(20), 'cfg ', 'inv ', 'uns', 'prf', 'gap', 'fell', 'lost', 'mism', 'rpp ', 'tpr ', ' rexp', 'work',
  '| fastR', 'fW', 'fB', 'fRec', '| qualR', 'qW', 'qB');
const soundness = new Map();
for (const [name, r] of results) {
  if (r.error) continue;
  for (const c of CONFIGS) {
    const run = r[c.name], base = get('base', c.name);
    const failures = invariants([...run.cases.values()], evidence);
    soundness.set(`${name}@${c.name}`, { failures, unstable: run.unstable });
    for (const f of failures) say(`  INVARIANT FAILURE ${name}@${c.name} ${f.key} ${f.rule} ${f.message}`);
    if (!base) continue;
    const o = optimalStats(base, run), fa = routeStats(base, run, 'fast'), q = routeStats(base, run, 'quality');
    say(name.padEnd(20), c.name.padEnd(4), (failures.length ? 'FAIL' : 'ok').padEnd(4), String(run.unstable).padStart(3),
      String(o.proofs[1]).padStart(3), String(o.gap[1] ?? '-').padStart(3), String(o.fell.length).padStart(4),
      String(o.lost.length).padStart(4), String(o.mismatch.length).padStart(4), f2(o.rpp).padStart(4), f2(o.tpr).padStart(4),
      count(o.reexpanded[1]).padStart(5), f2(o.work[0] ? o.work[1] / o.work[0] : null).padStart(4),
      '|', String(fa.routes[1]).padStart(5), String(fa.worse.length).padStart(2), String(fa.better).padStart(2), f2(fa.records),
      '|', String(q.routes[1]).padStart(5), String(q.worse.length).padStart(2), String(q.better).padStart(2));
  }
}
// The capacity and --pre runs feed no contrast, but they must be sound too.
for (const [name, r] of [['base', results.get('base')], ...(pre ? [['pre', pre]] : [])]) {
  if (!r || r.error) continue;
  for (const [config, run] of Object.entries(r)) {
    if (soundness.has(`${name}@${config}`)) continue;
    const failures = invariants([...run.cases.values()], evidence);
    soundness.set(`${name}@${config}`, { failures, unstable: run.unstable });
    for (const f of failures) say(`  INVARIANT FAILURE ${name}@${config} ${f.key} ${f.rule} ${f.message}`);
    if (run.unstable) say(`  NONDETERMINISTIC ${name}@${config}: ${run.unstable} records`);
  }
}

// Independence: the substitutions resolveBuild relies on.
const independence = [];
function expectSame(what, a, b, modes) {
  for (const c of CONFIGS) {
    const x = get(a, c.name), y = get(b, c.name);
    if (!x || !y) { independence.push(`${what}@${c.name}: missing build`); continue; }
    for (const mode of modes) {
      const rows = pairs(x, y, mode), diff = rows.filter(([p, t]) => !same(p, t)).map(([p]) => p.id);
      if (diff.length) independence.push(`${what} ${mode}@${c.name}: ${diff.length}/${rows.length} differ (${list(diff, 4)})`);
    }
  }
}
expectSame('o6 on Optimal (pea0 vs pea0+o6)', 'pea0', 'pea0+o6', ['optimal']);
expectSame('pea0 on Fast/Quality', 'base', 'pea0', ['fast', 'quality']);
for (const ctx of [[], ['o2'], ['o3b'], ['o2', 'o3b']]) {
  for (const p of ['pea1', 'pea2']) expectSame(`${p} on Fast/Quality in {${ctx}}`, label(ctx), label([...ctx, p]), ['fast', 'quality']);
}
say(`independence: ${independence.length ? 'FAILED' : 'ok'}`);
for (const line of independence) say(`  ${line}`);

// 5.4 S3: state-limited runs identical apart from reserved_bytes; more unique states when memory binds.
// Committed files keep no routes; two measured builds compare them too.
const LAYOUT = new Set(['reserved_bytes', 'search_us', 'first_route_us', 'route']);
const MEASURED = new Set(['reserved_bytes', 'search_us']);
/** Fields of a reference case (committed: sorted keys) whose values differ in a measured one. */
const differs = (committed, c, skip = LAYOUT) => Object.keys(committed)
  .filter(f => !skip.has(f) && (f !== 'reexpanded' || f in c) && JSON.stringify(committed[f]) !== JSON.stringify(c[f]));
const base20 = get('base', '20k');
if (base20) {
  const diffs = [], smaller = [];
  for (const b of baseline.cases) {
    const c = base20.cases.get(key(b));
    if (!c) { diffs.push(`${key(b)} missing`); continue; }
    const fields = differs(b, c);
    if (fields.length) diffs.push(`${key(b)} ${fields.join('/')}`);
    if (c.reserved_bytes < b.reserved_bytes) smaller.push(key(b));
  }
  say(`S3 identity base@20k vs baseline ${baseline.sourceRevision}: ${diffs.length} of ${baseline.cases.length} differ`
    + ` beyond reserved_bytes${diffs.length ? ` (${list(diffs)})` : ''}; reserved_bytes smaller on ${smaller.length}`);
}
// Spec 8: a non-memory-limited observe row that differs beyond reserved_bytes,
// or whose reserved_bytes did not fall by 12 B per record slot, blocks S3.
const observeDiffs = [];
const base1m = get('base', '1M');
if (base1m) {
  const slot = -12 * (CONFIGS[1].states + 1);
  for (const r of reference.cases) {
    const c = base1m.cases.get(key(r));
    if (!c) continue;
    const u0 = r.stats.unique_states, u1 = c.stats.unique_states;
    const fields = differs(r, c), delta = c.reserved_bytes - r.reserved_bytes;
    const bound = r.status === 'memory_limit';
    if (!bound && (fields.length || delta !== slot)) observeDiffs.push(key(r));
    const tag = bound ? `unique ${u0}->${u1} (${f2(u1 / u0)}x)` : fields.length ? `DIFFERS ${fields.join('/')}` : 'identical';
    say(`S3 1M ${key(r).padEnd(34)} ${r.status}->${c.status} ${tag} reserved_bytes ${delta}`
      + (bound || delta === slot ? '' : ` (expect ${slot})`));
  }
}

// 5.4 against the --pre build. Identity: every 20k row equal apart from
// reserved_bytes, which falls by 12 B per record slot (-240,012 at 20k).
// Capacity: median unique_states ratio >= 1.15 over rows memory-bound in both.
let s3 = null;
/** The committed 20k/64 MiB baseline at `commit`, or null. */
function baselineAt(commit) {
  const shown = git(['show', `${commit}:benchmarks/catalog-baseline.json`]);
  try {
    const b = shown.status === 0 ? upgrade(JSON.parse(shown.stdout)) : null;
    return b && b.maxStates === CONFIGS[0].states && b.memoryMiB === CONFIGS[0].memory ? b : null;
  } catch {
    return null;
  }
}
if (pre?.error) say(`S3 pre ${flags.pre}: ${pre.error}`);
if (pre && !pre.error && base20) {
  const revision = JSON.parse(readFileSync(resolve(dir, 'pre.meta.json'), 'utf8')).source;
  const diffs = [], deltas = {}, drift = [];
  for (const [k, p] of pre['20k'].cases) {
    const c = base20.cases.get(k);
    if (!c) { diffs.push(`${k} missing`); continue; }
    const fields = differs(p, c, MEASURED);
    if (fields.length) diffs.push(`${k} ${fields.join('/')}`);
    deltas[c.reserved_bytes - p.reserved_bytes] = (deltas[c.reserved_bytes - p.reserved_bytes] ?? 0) + 1;
  }
  // The pre commit's own baseline: HEAD's may already be refreshed for 5.4's reserved_bytes.
  const own = baselineAt(revision), held = own ?? baseline;
  for (const b of held.cases) {
    const p = pre['20k'].cases.get(key(b));
    if (!p || differs(b, p).length || p.reserved_bytes !== b.reserved_bytes) drift.push(key(b));
  }
  say(`S3 pre ${revision}@20k vs ${own ? 'its' : "HEAD's (it has no 20k/64 MiB)"} baseline ${held.sourceRevision}:`
    + ` ${drift.length} of ${held.cases.length} differ (the refactors before 5.4 must change nothing)`
    + (drift.length ? ` (${list(drift)})` : ''));
  const deltaText = JSON.stringify(deltas), rows = pre['20k'].cases.size;
  say(`S3 identity base@20k vs pre: ${diffs.length} of ${rows} differ beyond reserved_bytes${diffs.length ? ` (${list(diffs)})` : ''};`
    + ` reserved_bytes deltas ${deltaText} (expect {"-240012":${rows}})`);
  const ratios = [], overBudget = [];
  for (const cap of CAPACITY) {
    const before = pre[cap.name], after = get('base', cap.name);
    if (!before || !after) { say(`S3 capacity ${cap.name}: missing`); continue; }
    for (const [k, p] of before.cases) {
      const c = after.cases.get(k);
      if (!c) { say(`S3 capacity ${k}: missing after`); continue; }
      const [u0, u1] = [p.stats.unique_states, c.stats.unique_states];
      const inserts = r => r.stats.unique_states + r.stats.duplicate_improvements;
      const bound = p.status === 'memory_limit' && c.status === 'memory_limit';
      if (bound) ratios.push(u1 / u0);
      if (c.reserved_bytes > cap.memory * 1048576) overBudget.push(`${k}@${cap.memory}MiB`);
      say(`S3 capacity ${`${k}@${cap.memory}MiB`.padEnd(40)} ${p.status}->${c.status} unique ${u0}->${u1} (${f2(u1 / u0)}x)`
        + ` inserts ${inserts(p)}->${inserts(c)}${bound ? '' : ' (not memory-bound in both)'}`
        + (c.reserved_bytes > cap.memory * 1048576 ? ' OVER BUDGET' : ''));
    }
  }
  const identical = !diffs.length && deltaText === JSON.stringify({ '-240012': rows });
  // Spec 8: median >= 1.15, but any single row below 1.10 needs a look first.
  const least = ratios.length ? Math.min(...ratios) : null;
  const capacity = ratios.length > 0 && median(ratios) >= 1.15 && least >= 1.10;
  s3 = identical && capacity && !drift.length && !observeDiffs.length && !overBudget.length;
  if (observeDiffs.length) say(`S3 1M observe rows off spec (not memory-bound): ${list(observeDiffs)}`);
  if (overBudget.length) say(`S3 OVER BUDGET: ${list(overBudget)}`);
  say(`S3 capacity: median unique ratio ${f2(median(ratios))} (min ${f2(least)}${least !== null && least < 1.10 ? ' < 1.10' : ''})`
    + ` over ${ratios.length} memory-bound rows; S3 ${s3 ? 'PASS' : 'REVIEW'}`);
}

/** One experiment switched on against the same context with it off. */
function contrast(experiment, context) {
  const others = context.filter(f => f !== experiment && !(PEA.includes(experiment) && PEA.includes(f)));
  const treat = [...others, experiment], per = {}, veto = [], builds = {};
  for (const c of CONFIGS) {
    per[c.name] = {};
    for (const mode of MODES_OF[experiment]) {
      const [ca, tb] = builds[mode] = [resolveBuild(others, mode), resolveBuild(treat, mode)];
      const a = get(ca, c.name), b = get(tb, c.name);
      if (!a || !b) { veto.push(`${mode}@${c.name}: build ${a ? tb : ca} missing`); continue; }
      // Only this mode's failures: {pea0,o6} is one build measuring two experiments.
      const s = soundness.get(`${tb}@${c.name}`), own = s.failures.filter(f => f.key.endsWith(`:${mode}`));
      if (own.length) veto.push(`${tb}@${c.name}: ${own.length} ${mode} invariant failures`);
      if (s.unstable) veto.push(`${tb}@${c.name}: ${s.unstable} nondeterministic records`);
      per[c.name][mode] = mode === 'optimal' ? optimalStats(a, b) : routeStats(a, b, mode);
      if (mode === 'optimal' && per[c.name][mode].mismatch.length) veto.push(`${c.name}: uncapped Optimal moves changed`);
    }
  }
  const substituted = [experiment, ...others].some(f => f === 'o6' || PEA.includes(f));
  if (independence.length && substituted) veto.push('independence check failed');
  return { experiment, control: label(others), treat: label(treat), builds, per, veto: [...new Set(veto)] };
}

// Adoption rule, fixed before the first measurement (PLAN 5.2-5.6). An
// experiment passes when its criterion holds switched on against the same
// build with it off, the other adopted experiments on; any soundness veto
// (invariant failure, nondeterminism, changed uncapped Optimal moves, failed
// build or run) rejects it. "Improves" means +1 Optimal proof or a median
// capped lower-bound gap (to the best known route, over boards the control
// leaves unproven) at most 0.95x the control's, at either config. Judged in
// plan order o2, o3b, pea (best passing C: most proofs summed over the
// configs, then smallest gap at the last config, then fewest expanded +
// reexpanded nodes at the last config over the Optimal rows the control and
// every passing C solve, then smaller C), o6; then every adopted experiment
// is re-judged against the final set. R4 falls and worse Fast/Quality moves of adopted
// experiments are listed for --accept-regressions (decisions 2 and 3).
const cfgs = CONFIGS.map(c => c.name), last = cfgs[cfgs.length - 1];
const opt = (v, cfg) => v.per[cfg]?.optimal;
const improves = v => cfgs.some(cfg => {
  const o = opt(v, cfg);
  return o && (o.proofs[1] >= o.proofs[0] + 1 || (o.gap[0] > 0 && o.gap[1] <= 0.95 * o.gap[0]));
});
const noFewerProofs = v => cfgs.every(cfg => opt(v, cfg) && opt(v, cfg).proofs[1] >= opt(v, cfg).proofs[0]);
const CRITERIA = {
  // 5.2: improves; uncapped moves identical (veto); time per record +<=15% (1M medians).
  o2: v => improves(v) && noFewerProofs(v) && (opt(v, last)?.tpr ?? 1) <= 1.15,
  // 5.6: proofs, capped bounds and records per proof no worse; no Fast/Quality move regression.
  o3b: v => noFewerProofs(v) && cfgs.every(cfg => {
    const p = v.per[cfg];
    return p.optimal && !p.optimal.fell.length && (p.optimal.rpp ?? 1) <= 1
      && ['fast', 'quality'].every(m => p[m] && !p[m].worse.length && !p[m].lost.length);
  }),
  // 5.3: improves (WASM equality is the parity run with the feature on).
  pea: v => improves(v) && noFewerProofs(v),
  // 5.5: Fast routes found >= before at both configs; median Fast records -30% at 1M; routes replay (catalog.rs).
  o6: v => cfgs.every(cfg => v.per[cfg].fast && v.per[cfg].fast.routes[1] >= v.per[cfg].fast.routes[0])
    && (v.per[last].fast?.records ?? 1) <= 0.7,
};
/** Pea tie-break: each passing C's expanded + reexpanded at the last config over the same rows (solved by all). */
function peaWork(passing) {
  const control = get(passing[0].builds.optimal[0], last), runs = passing.map(v => get(v.builds.optimal[1], last));
  const rows = [...control.cases.values()].filter(c => c.mode === 'optimal' && c.status === 'solved'
    && runs.every(r => r.cases.get(key(c))?.status === 'solved')).map(key);
  const cost = new Map(passing.map((v, i) => [v, rows.reduce((sum, k) => sum + work(runs[i].cases.get(k)), 0)]));
  cost.rows = rows.length;
  return cost;
}
const judge = (experiment, context) => {
  const v = contrast(experiment, context);
  v.pass = !v.veto.length && CRITERIA[PEA.includes(experiment) ? 'pea' : experiment](v);
  return v;
};
function describe(v) {
  say(`${v.experiment} (${v.control} -> ${v.treat}): ${v.pass ? 'PASS' : 'fail'}${v.veto.length ? ` VETO ${v.veto.join('; ')}` : ''}`);
  for (const cfg of cfgs) {
    const p = v.per[cfg], parts = [];
    if (p.optimal) {
      const o = p.optimal;
      parts.push(`opt proofs ${o.proofs.join('->')} gap ${o.gap.map(g => g ?? '-').join('->')} (n=${o.n}) fell ${o.fell.length}`
        + ` lost ${o.lost.length} mism ${o.mismatch.length} rpp ${f2(o.rpp)} tpr ${f2(o.tpr)}`
        + ` work ${o.work.map(count).join('->')} (n=${o.n_work})`
        + (o.reexpanded[1] === null ? '' : ` rexp ${o.reexpanded.map(x => x === null ? 'none' : count(x)).join('->')}`)
        + ` same ${o.identical}/${o.n_all}`);
    }
    for (const m of ['fast', 'quality']) {
      if (!p[m]) continue;
      const s = p[m];
      parts.push(`${m} routes ${s.routes.join('->')} worse ${s.worse.length} better ${s.better} lost ${s.lost.length}`
        + ` gained ${s.gained} rec ${f2(s.records)} same ${s.identical}/${s.n_all}`);
    }
    say(`  ${cfg.padEnd(3)} ${parts.join(' | ')}`);
    const o = p.optimal;
    if (o?.fell.length) say(`      fell: ${list(o.fell)}`);
    if (o?.lost.length) say(`      lost proofs: ${list(o.lost)}`);
    if (o?.mismatch.length) say(`      MISMATCH: ${list(o.mismatch)}`);
    for (const m of ['fast', 'quality']) {
      if (p[m]?.worse.length) say(`      ${m} worse: ${list(p[m].worse)}`);
      if (p[m]?.lost.length) say(`      ${m} lost: ${list(p[m].lost)}`);
    }
  }
}

say('--- contrasts in plan order (control -> treat) ---');
const adopted = [];
for (const step of [['o2'], ['o3b'], PEA, ['o6']]) {
  const verdicts = step.map(e => judge(e, adopted));
  verdicts.forEach(describe);
  const passing = verdicts.filter(v => v.pass);
  const total = v => cfgs.reduce((sum, cfg) => sum + (opt(v, cfg)?.proofs[1] ?? 0), 0);
  const gap = v => opt(v, last)?.gap[1] ?? 0;
  const cost = passing.length > 1 ? peaWork(passing) : new Map();
  if (cost.size) {
    say(`pea tie-break at ${last}: expanded+reexpanded ${passing.map(v => `${v.experiment} ${cost.get(v)}`).join(', ')}`
      + ` over the ${cost.rows} Optimal rows ${passing[0].control} and every passing C solve`);
  }
  passing.sort((x, y) => total(y) - total(x) || gap(x) - gap(y) || (cost.get(x) ?? 0) - (cost.get(y) ?? 0));
  if (passing.length) adopted.push(passing[0].experiment);
}
say('--- final check: each adopted experiment against the final set ---');
const final = adopted.map(e => judge(e, adopted));
final.forEach(describe);
const accepted = final.filter(v => v.pass).map(v => v.experiment);
say(`DECISION: adopt {${accepted.join(', ')}}; reject {${['o2', 'o3b', 'pea*', 'o6']
  .filter(e => !accepted.some(a => a === e || (e === 'pea*' && PEA.includes(a)))).join(', ')}}`
  + (accepted.length < adopted.length ? ` (greedy picked {${adopted.join(', ')}}; the final check dropped some: review)` : '')
  + `; S3 ${s3 === null ? `not judged (${pre?.error ? '--pre run failed' : !flags.pre ? 'no --pre' : 'base build failed'})`
    : s3 ? 'keep' : 'REVIEW'}`);
for (const v of final.filter(x => x.pass)) {
  for (const cfg of cfgs) {
    const p = v.per[cfg];
    if (p.optimal?.fell.length) say(`  accept R4 (${v.experiment}@${cfg}): ${list(p.optimal.fell, 20)}`);
    for (const m of ['fast', 'quality']) if (p[m]?.worse.length) say(`  accept worse ${m} (${v.experiment}@${cfg}): ${list(p[m].worse, 20)}`);
  }
}

const text = lines.join('\n') + '\n';
writeFileSync(resolve(dir, 'report.txt'), text);
console.log('\n===== FACTORIAL REPORT (also target/bench/factorial/report.txt) =====');
console.log(text);
if (failed.length || independence.length || [...soundness.values()].some(s => s.failures.length || s.unstable)) process.exitCode = 1;
