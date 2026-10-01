// Pure benchmark-gate logic: baseline schema, regression rules, false-proof
// invariants, the scoreboard and bench:observe's review helpers. No filesystem
// or process access, so scripts/bench-gate.test.mjs can exercise every rule
// without cargo.

/**
 * A case of the current run as toCase builds it, or of a committed file as load()
 * checks it: every key is present, and null means the run had no such value.
 * @typedef {object} Case
 * @property {string} id
 * @property {string} mode
 * @property {string} fingerprint
 * @property {string} status
 * @property {number | null} moves
 * @property {number | null} pushes
 * @property {string} proof The proof kind.
 * @property {number | null} lower_bound
 * @property {number} expanded
 * @property {number} generated
 * @property {number} reserved_bytes
 * @property {number | null} first_route_expanded
 * @property {number | null} first_route_generated
 * @property {Record<string, number> | null} stats
 */

/**
 * One difference between two cases; `rule` is null for changed fields.
 * @typedef {{ key: string, rule: string | null, field: string, from: unknown, to: unknown }} Finding
 */

/**
 * The result of compare: findings by bucket, and the keys found on one side only.
 * @typedef {object} Diff
 * @property {Finding[]} hard
 * @property {Finding[]} soft
 * @property {Finding[]} improved
 * @property {Finding[]} changed
 * @property {string[]} missing Keys only in the baseline.
 * @property {string[]} extra Keys only in the new run.
 */

/** @typedef {{ key: string, rule: string, message: string }} Failure */

/**
 * A baseline or observe-reference file as load() returns it.
 * @typedef {object} Baseline
 * @property {number} version
 * @property {string} sourceRevision
 * @property {string} catalogHash
 * @property {number} maxStates
 * @property {number} memoryMiB
 * @property {Case[]} cases
 */

export const SCHEMA_VERSION = 2;
export const MODES = ['fast', 'quality', 'optimal'];
/** @type {ReadonlySet<string>} */
export const FINISHED = new Set(['solved', 'exhausted']);
/** @type {ReadonlySet<string>} */
export const CAPPED = new Set(['state_limit', 'memory_limit', 'time_limit']);
/** @type {ReadonlySet<string>} */
const PROOFS = new Set(['optimal', 'unsolvable']);
// Every key of a Case, and so every key load() requires of a committed case.
const CASE_KEYS = [
  'id',
  'mode',
  'fingerprint',
  'status',
  'moves',
  'pushes',
  'proof',
  'lower_bound',
  'expanded',
  'generated',
  'reserved_bytes',
  'first_route_expanded',
  'first_route_generated',
  'stats',
];

/** @param {{ id: string, mode: string }} c */
export const key = c => `${c.id}:${c.mode}`;
/** @param {string} status */
const rank = status => (FINISHED.has(status) ? 2 : CAPPED.has(status) ? 1 : 0);
/** @param {number} previous */
const tolerance = previous => Math.max(previous + 32, Math.ceil(previous * 1.2));

/**
 * One catalog.rs record as a baseline case; every field is present (null when absent).
 * @param {import('./corpus.mjs').CorpusRecord} r
 * @returns {Case}
 */
export function toCase(r) {
  return {
    id: r.id,
    mode: r.mode,
    fingerprint: r.fingerprint ?? null,
    status: r.status ?? null,
    moves: r.moves ?? null,
    pushes: r.pushes ?? null,
    proof: r.proof?.kind ?? null,
    lower_bound: r.lower_bound ?? null,
    expanded: r.expanded ?? null,
    generated: r.generated ?? null,
    reserved_bytes: r.reserved_bytes ?? null,
    first_route_expanded: r.first_route_expanded ?? null,
    first_route_generated: r.first_route_generated ?? null,
    stats: r.stats ? { ...r.stats } : null,
  };
}

/**
 * A parsed baseline or observe-reference file, checked so that no rule meets a
 * missing value: it must be at SCHEMA_VERSION, and every case must carry every
 * Case key, with a string fingerprint, since invariants trusts a route as
 * evidence only on the board it was found on. Anything else throws, naming the
 * file and the command that regenerates it.
 * @param {any} file A parsed JSON file.
 * @param {string} name The file's path, for the error.
 * @param {string} update The npm command that rewrites the file.
 * @returns {Baseline} The file itself.
 */
export function load(file, name, update) {
  const regenerate = `Regenerate the file: delete it, then run ${update}.`;
  if (file.version !== SCHEMA_VERSION) throw new Error(`${name} has schema version ${file.version}, not ${SCHEMA_VERSION}. ${regenerate}`);
  if (!Array.isArray(file.cases)) throw new Error(`${name} has no cases array. ${regenerate}`);
  for (const c of file.cases) {
    const lacking = CASE_KEYS.filter(field => !Object.hasOwn(c, field) || (field === 'fingerprint' && typeof c.fingerprint !== 'string'));
    if (lacking.length) throw new Error(`${name}: case ${key(c)} lacks ${lacking.join(', ')}. ${regenerate}`);
  }
  return file;
}

/**
 * A copy of a JSON value with every object's keys in sorted order.
 * @param {any} value
 * @returns {any}
 */
export function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map(name => [name, sortKeys(value[name])]),
  );
}

/**
 * Deterministic text: sorted header keys, then one case per line in catalog
 * order (ids from `order`) and mode order, LF endings, trailing newline.
 * @param {{ [name: string]: unknown, cases: readonly Record<string, any>[] }} file
 * @param {readonly string[]} order
 * @returns {string}
 */
export function serialize({ cases, ...header }, order) {
  const index = new Map(order.map((id, i) => [id, i]));
  /** @param {string} id */
  const at = id => index.get(id) ?? order.length;
  const sorted = [...cases].sort(
    (a, b) => at(a.id) - at(b.id) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0) || MODES.indexOf(a.mode) - MODES.indexOf(b.mode),
  );
  return [
    '{',
    ...Object.entries(sortKeys(header)).map(([name, value]) => `  ${JSON.stringify(name)}: ${JSON.stringify(value)},`),
    '  "cases": [',
    ...sorted.map((c, i) => `    ${JSON.stringify(sortKeys(c))}${i + 1 < sorted.length ? ',' : ''}`),
    '  ]',
    '}',
    '',
  ].join('\n');
}

/**
 * The compared fields of a case: all but id and mode, with stats flattened to "stats.<name>".
 * @param {Case} c
 * @returns {Record<string, any>}
 */
function fields(c) {
  /** @type {Record<string, any>} */
  const out = {};
  for (const [name, value] of Object.entries(c)) {
    if (name === 'id' || name === 'mode') continue;
    if (name === 'stats' && value) for (const [stat, count] of Object.entries(value)) out[`stats.${stat}`] = count;
    else out[name] = value;
  }
  return out;
}

/**
 * Diffs a baseline's cases (prev) against a new run's (cur), matched by key.
 * Hard rules:
 *   R1 moves never worsen and a route is never lost;
 *   R2 an optimal or unsolvable proof is retained;
 *   R3 Optimal never drops status rank (finished > capped > other);
 *   R4 an Optimal lower bound never falls, unless the run is now proven unsolvable.
 * Soft rules:
 *   S1 expanded/generated stay within max(+32, 1.2x), judged only when both runs
 *      finished;
 *   S2 reserved_bytes never grows at the same config;
 *   S3 Fast/Quality never drop status rank.
 * A move the other way on a ruled field is improved, and every other difference
 * is changed. Keys only in prev are missing, and keys only in cur are extra.
 *
 * bench:check fails on any hard or soft finding and on the improvements
 * mustRecord selects, and through formatReport counts missing and extra cases
 * as hard. bench:update records soft findings, improvements and missing or
 * extra cases, and refuses hard findings unless run with --accept-regressions.
 * Changed findings and S1 and S2 improvements never fail either. Invariant
 * failures (see invariants) fail every caller, with no override.
 * @param {readonly Case[]} prev
 * @param {readonly Case[]} cur
 * @param {{ sameConfig: boolean }} options sameConfig: both runs used the same state and memory limits.
 * @returns {Diff}
 */
export function compare(prev, cur, { sameConfig }) {
  /** @type {Diff} */
  const out = { hard: [], soft: [], improved: [], changed: [], missing: [], extra: [] };
  const before = new Map(prev.map(c => [key(c), c])),
    now = new Map(cur.map(c => [key(c), c]));
  for (const k of before.keys()) if (!now.has(k)) out.missing.push(k);
  for (const k of now.keys()) if (!before.has(k)) out.extra.push(k);
  for (const [k, p] of before) {
    const c = now.get(k);
    if (!c) continue;
    const P = fields(p),
      C = fields(c),
      flagged = new Set();
    /**
     * @param {'hard' | 'soft' | 'improved' | 'changed'} bucket
     * @param {string | null} rule
     * @param {string} field
     */
    const note = (bucket, rule, field) => {
      flagged.add(field);
      out[bucket].push({ key: k, rule, field, from: P[field], to: C[field] });
    };
    const optimal = c.mode === 'optimal';
    if (P.moves !== null && (C.moves === null || C.moves > P.moves)) note('hard', 'R1', 'moves');
    else if (C.moves !== null && (P.moves === null || C.moves < P.moves)) note('improved', 'R1', 'moves');
    if (P.proof !== C.proof) {
      if (PROOFS.has(P.proof)) note('hard', 'R2', 'proof');
      else if (PROOFS.has(C.proof)) note('improved', 'R2', 'proof');
    }
    if (rank(C.status) !== rank(P.status)) {
      const rule = optimal ? 'R3' : 'S3';
      note(rank(C.status) > rank(P.status) ? 'improved' : optimal ? 'hard' : 'soft', rule, 'status');
    }
    if (optimal && C.proof !== 'unsolvable') {
      if (P.lower_bound !== null && (C.lower_bound === null || C.lower_bound < P.lower_bound)) note('hard', 'R4', 'lower_bound');
      else if (C.lower_bound !== null && (P.lower_bound === null || C.lower_bound > P.lower_bound)) note('improved', 'R4', 'lower_bound');
    }
    if (FINISHED.has(P.status) && FINISHED.has(C.status)) {
      for (const field of ['expanded', 'generated']) {
        if (C[field] > tolerance(P[field])) note('soft', 'S1', field);
        else if (C[field] < P[field]) note('improved', 'S1', field);
      }
    }
    if (sameConfig) {
      if (C.reserved_bytes > P.reserved_bytes) note('soft', 'S2', 'reserved_bytes');
      else if (C.reserved_bytes < P.reserved_bytes) note('improved', 'S2', 'reserved_bytes');
    }
    // A stat only one run reports (SearchStats gained or lost a field) is changed, shown as ? on the other side.
    for (const field of new Set([...Object.keys(P), ...Object.keys(C)])) {
      if (!flagged.has(field) && P[field] !== C[field]) note('changed', null, field);
    }
  }
  return out;
}

/**
 * The rules that judge a run's results (route, proof, status and bound) rather
 * than its cost.
 * @type {ReadonlySet<string | null>}
 */
const RESULT_RULES = new Set(['R1', 'R2', 'R3', 'R4', 'S3']);

/**
 * The improvements bench:check fails on until bench:update records them: those
 * under a result rule (R1-R4, S3). The baseline value is the floor those rules
 * hold later runs to, so a gain left out of it would let a later loss back to
 * the old value pass. S1 and S2 judge cost, not results, so their improvements
 * stay informational.
 * @param {Diff} diff
 * @returns {Finding[]}
 */
export function mustRecord(diff) {
  return diff.improved.filter(f => RESULT_RULES.has(f.rule));
}

/**
 * Absolute rules with no escape hatch; any failure is a soundness bug.
 *   I1 only Optimal carries a proof or a lower bound;
 *   I2 the proof agrees with moves, lower_bound and status;
 *   I3 no Optimal bound or "optimal" proof lies above, and no "unsolvable"
 *      sits alongside, a replay-verified route from any mode or evidence case
 *      of the same id. Evidence counts only when its fingerprint matches the
 *      current board's.
 * @param {readonly Case[]} cases
 * @param {readonly Case[]} [evidence]
 * @returns {Failure[]}
 */
export function invariants(cases, evidence = []) {
  /** @type {Failure[]} */
  const failures = [];
  const best = new Map();
  const fingerprints = new Map(cases.map(c => [c.id, c.fingerprint]));
  /**
   * @param {string} id
   * @param {number | null} moves
   */
  const offer = (id, moves) => {
    if (moves !== null && (!best.has(id) || moves < best.get(id))) best.set(id, moves);
  };
  for (const c of cases) offer(c.id, c.moves);
  for (const e of evidence) if (e.fingerprint === fingerprints.get(e.id)) offer(e.id, e.moves);
  for (const c of cases) {
    /**
     * @param {string} rule
     * @param {string} message
     */
    const fail = (rule, message) => failures.push({ key: key(c), rule, message });
    const { moves, lower_bound: lower, proof } = c;
    if (c.mode !== 'optimal') {
      if (proof !== 'none' || lower !== null) fail('I1', `${c.mode} claims proof=${proof} lower_bound=${lower}`);
      continue;
    }
    const consistent = /** @type {Record<string, boolean>} */ ({
      optimal: moves !== null && lower === moves,
      bounded: moves !== null && lower !== null && lower < moves,
      unsolvable: c.status === 'exhausted' && moves === null,
      none: moves === null,
    })[proof];
    if (!consistent) fail('I2', `proof=${proof} disagrees with moves=${moves} lower_bound=${lower} status=${c.status}`);
    if (!best.has(c.id)) continue;
    const route = best.get(c.id);
    if (lower !== null && lower > route) fail('I3', `lower bound ${lower} exceeds a verified ${route}-move route`);
    if (proof === 'optimal' && moves !== route) fail('I3', `claims ${moves} moves optimal but a ${route}-move route exists`);
    if (proof === 'unsolvable') fail('I3', `claims unsolvable but a ${route}-move route exists`);
  }
  return failures;
}

/**
 * Per-mode totals; the delta between two scoreboards shows a change's net effect.
 * @param {readonly Case[]} cases
 */
export function scoreboard(cases) {
  /**
   * @param {readonly Case[]} list
   * @param {(c: Case) => number | null | undefined} value
   */
  const sum = (list, value) => list.reduce((total, c) => total + (value(c) ?? 0), 0);
  return MODES.map(mode => {
    const all = cases.filter(c => c.mode === mode);
    const finished = all.filter(c => FINISHED.has(c.status)),
      capped = all.filter(c => CAPPED.has(c.status));
    const proven = all.filter(c => PROOFS.has(c.proof));
    const routes = all.filter(c => c.moves !== null);
    return {
      mode,
      runs: all.length,
      routes: routes.length,
      finished: finished.length,
      capped: capped.length,
      proofs: proven.length,
      movesSum: sum(routes, c => c.moves),
      cappedLowerBoundSum: mode === 'optimal' ? sum(capped, c => c.lower_bound) : 0,
      uncappedExpanded: sum(finished, c => c.expanded),
      uncappedGenerated: sum(finished, c => c.generated),
      uniqueStates: sum(all, c => c.stats?.unique_states),
      duplicateImprovements: sum(all, c => c.stats?.duplicate_improvements),
      recordsPerProof: proven.length ? Math.round(sum(proven, c => c.generated) / proven.length) : null,
    };
  });
}

/**
 * Row-wise after - before for numeric scoreboard columns.
 * @param {readonly Record<string, any>[]} before
 * @param {readonly Record<string, any>[]} after
 */
export function scoreboardDelta(before, after) {
  return after.map((row, i) =>
    Object.fromEntries(
      Object.entries(row).map(([name, value]) => [
        name,
        typeof value === 'number' && typeof before[i]?.[name] === 'number' ? value - before[i][name] : value,
      ]),
    ),
  );
}

/**
 * The median of a non-empty list of timings in microseconds. An even-length list
 * has two middle values; their mean, rounded to a whole microsecond, is the median.
 * @param {readonly number[]} list
 * @returns {number}
 */
export function median(list) {
  const sorted = [...list].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : Math.round((sorted[middle - 1] + sorted[middle]) / 2);
}

/**
 * Pairs each case with the reference case of the same key for bench:observe's
 * review deltas, but only on the same board: when the fingerprints differ, the
 * board was edited after the reference was recorded, and a delta would compare
 * two different boards.
 * @template {Case} T
 * @param {readonly Case[]} cases
 * @param {readonly T[]} reference
 * @returns {{ pairs: Map<string, T>, changed: string[] }} pairs: the reference case by key; changed: the ids
 *   whose board differs from the reference's, each once.
 */
export function pairReference(cases, reference) {
  const prior = new Map(reference.map(c => [key(c), c]));
  /** @type {Map<string, T>} */
  const pairs = new Map();
  /** @type {string[]} */
  const changed = [];
  for (const c of cases) {
    const p = prior.get(key(c));
    if (p && p.fingerprint === c.fingerprint) pairs.set(key(c), p);
    else if (p && !changed.includes(c.id)) changed.push(c.id);
  }
  return { pairs, changed };
}

/** @param {unknown} value */
const show = value => (value === undefined ? '?' : JSON.stringify(value));
/**
 * @param {string} title
 * @param {readonly Finding[]} findings
 * @param {number} limit Findings listed one per line; more are tallied by field.
 */
function section(title, findings, limit) {
  if (!findings.length) return [`${title}: none`];
  const lines = [`${title} (${findings.length}):`];
  if (findings.length > limit) {
    const tally = new Map();
    for (const f of findings) tally.set(f.field, (tally.get(f.field) ?? 0) + 1);
    lines.push(`  by field: ${[...tally].map(([field, n]) => `${field} x${n}`).join(', ')}`);
  }
  for (const f of findings.slice(0, limit)) lines.push(`  ${f.key}  ${f.rule ?? '-'}  ${f.field}: ${show(f.from)} -> ${show(f.to)}`);
  if (findings.length > limit) lines.push(`  ... ${findings.length - limit} more in target/bench/report.txt`);
  return lines;
}

/**
 * What formatReport prints.
 * @typedef {object} ReportFields
 * @property {string} action
 * @property {readonly Case[]} cases
 * @property {{ maxStates: number, memoryMiB: number }} config
 * @property {Diff} diff
 * @property {readonly Failure[]} failures
 * @property {string} [baseline] The baseline's source revision.
 * @property {string} source
 * @property {boolean} [catalogChangesAreHard]
 * @property {number} [limit] Improved and changed findings listed per section.
 */

/**
 * Report sections plus one greppable BENCH line. With catalogChangesAreHard,
 * missing and extra cases count as hard findings (check); update only lists them.
 * @param {ReportFields} report
 * @returns {{ text: string, hard: number }}
 */
export function formatReport({ action, cases, config, diff, failures, baseline, source, catalogChangesAreHard, limit = Infinity }) {
  const catalog = diff.missing.length + diff.extra.length;
  const hard = diff.hard.length + (catalogChangesAreHard ? catalog : 0);
  const lines = [
    ...(failures.length
      ? [`INVARIANT FAILURES (${failures.length}):`, ...failures.map(f => `  ${f.key}  ${f.rule}  ${f.message}`)]
      : ['Invariants: ok']),
    ...(diff.missing.length ? [`Missing cases (${diff.missing.length}): ${diff.missing.join(', ')}`] : []),
    ...(diff.extra.length ? [`Extra cases (${diff.extra.length}): ${diff.extra.join(', ')}`] : []),
    ...section('Hard', diff.hard, Infinity),
    ...section('Soft', diff.soft, Infinity),
    ...section('Improved', diff.improved, limit),
    ...section('Changed', diff.changed, limit),
    `BENCH ${action} v${SCHEMA_VERSION}: cases=${cases.length} config=${config.maxStates}/${config.memoryMiB} hard=${hard}`
      + ` soft=${diff.soft.length} improved=${diff.improved.length} changed=${diff.changed.length}`
      + ` invariants=${failures.length ? `FAIL(${failures.length})` : 'ok'} baseline=${baseline ?? 'none'} source=${source}`,
  ];
  return { text: lines.join('\n'), hard };
}
