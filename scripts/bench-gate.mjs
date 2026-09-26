// Pure benchmark-gate logic: baseline schema, regression rules, false-proof
// invariants and the scoreboard. No filesystem or process access, so
// scripts/bench-gate.test.mjs can exercise every rule without cargo.
//
// A key missing from a case means "unknown" (v1 baselines never recorded
// status, bounds or stats), and every rule skips it.
export const SCHEMA_VERSION = 2;
export const MODES = ['fast', 'quality', 'optimal'];
export const FINISHED = new Set(['solved', 'exhausted']);
export const CAPPED = new Set(['state_limit', 'memory_limit', 'time_limit']);
const PROOFS = new Set(['optimal', 'unsolvable']);

export const key = c => `${c.id}:${c.mode}`;
const rank = status => FINISHED.has(status) ? 2 : CAPPED.has(status) ? 1 : 0;
const known = value => value !== undefined;
const tolerance = previous => Math.max(previous + 32, Math.ceil(previous * 1.2));

/** One catalog.rs record as a baseline case; every field is present (null when absent). */
export function toCase(r) {
  return {
    id: r.id, mode: r.mode, fingerprint: r.fingerprint ?? null, status: r.status ?? null,
    moves: r.moves ?? null, pushes: r.pushes ?? null, proof: r.proof?.kind ?? null, lower_bound: r.lower_bound ?? null,
    expanded: r.expanded ?? null, generated: r.generated ?? null, reserved_bytes: r.reserved_bytes ?? null,
    first_route_expanded: r.first_route_expanded ?? null, first_route_generated: r.first_route_generated ?? null,
    stats: r.stats ? { ...r.stats } : null,
  };
}

/** Read any supported baseline as v2. v1 knew only moves, proven and counts. */
export function upgrade(baseline) {
  if (baseline.version === SCHEMA_VERSION) return baseline;
  if (baseline.version !== 1) throw new Error(`Unsupported benchmark baseline version ${baseline.version}`);
  const cases = baseline.cases.map(({ id, mode, moves, proven, expanded, generated, reserved_bytes }) =>
    ({ id, mode, moves, expanded, generated, reserved_bytes, ...(proven ? { proof: 'optimal' } : {}) }));
  return { ...baseline, version: SCHEMA_VERSION, cases };
}

export function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.keys(value).sort().map(name => [name, sortKeys(value[name])]));
}

/** Deterministic text: sorted header keys, then one case per line in catalog
 * order (ids from `order`) and mode order, LF endings, trailing newline. */
export function serialize({ cases, ...header }, order) {
  const index = new Map(order.map((id, i) => [id, i]));
  const at = id => index.has(id) ? index.get(id) : order.length;
  const sorted = [...cases].sort((a, b) => at(a.id) - at(b.id)
    || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0) || MODES.indexOf(a.mode) - MODES.indexOf(b.mode));
  return ['{',
    ...Object.entries(sortKeys(header)).map(([name, value]) => `  ${JSON.stringify(name)}: ${JSON.stringify(value)},`),
    '  "cases": [',
    ...sorted.map((c, i) => `    ${JSON.stringify(sortKeys(c))}${i + 1 < sorted.length ? ',' : ''}`),
    '  ]', '}', ''].join('\n');
}

/** Known fields of a case, with stats flattened to "stats.<name>". */
function fields(c) {
  const out = {};
  for (const [name, value] of Object.entries(c)) {
    if (name === 'id' || name === 'mode' || !known(value)) continue;
    if (name === 'stats' && value) for (const [stat, count] of Object.entries(value)) out[`stats.${stat}`] = count;
    else out[name] = value;
  }
  return out;
}

/**
 * Hard rules (update needs --accept-regressions to record them):
 *   R1 moves never worsen and a route is never lost;
 *   R2 an optimal or unsolvable proof is retained;
 *   R3 Optimal never drops status rank (finished > capped > other);
 *   R4 an Optimal lower bound never falls, unless the run is now proven unsolvable.
 * Soft rules:
 *   S1 expanded/generated stay within max(+32, 1.2x), judged only when both runs finished;
 *   S2 reserved_bytes never grows at the same config;
 *   S3 Fast/Quality never drop status rank.
 * Every other difference is reported as changed, and unknown-to-known as recorded.
 */
export function compare(prev, cur, { sameConfig }) {
  const out = { hard: [], soft: [], improved: [], changed: [], recorded: [], missing: [], extra: [] };
  const before = new Map(prev.map(c => [key(c), c])), now = new Map(cur.map(c => [key(c), c]));
  for (const k of before.keys()) if (!now.has(k)) out.missing.push(k);
  for (const k of now.keys()) if (!before.has(k)) out.extra.push(k);
  for (const [k, p] of before) {
    if (!now.has(k)) continue;
    const c = now.get(k), P = fields(p), C = fields(c), flagged = new Set();
    const both = field => known(P[field]) && known(C[field]);
    const note = (bucket, rule, field) => {
      flagged.add(field);
      out[bucket].push({ key: k, rule, field, from: P[field], to: C[field] });
    };
    const optimal = c.mode === 'optimal';
    if (both('moves')) {
      if (P.moves !== null && (C.moves === null || C.moves > P.moves)) note('hard', 'R1', 'moves');
      else if (C.moves !== null && (P.moves === null || C.moves < P.moves)) note('improved', 'R1', 'moves');
    }
    if (both('proof') && P.proof !== C.proof) {
      if (PROOFS.has(P.proof)) note('hard', 'R2', 'proof');
      else if (PROOFS.has(C.proof)) note('improved', 'R2', 'proof');
    }
    if (both('status') && rank(C.status) !== rank(P.status)) {
      const rule = optimal ? 'R3' : 'S3';
      note(rank(C.status) > rank(P.status) ? 'improved' : optimal ? 'hard' : 'soft', rule, 'status');
    }
    if (optimal && both('lower_bound') && C.proof !== 'unsolvable') {
      if (P.lower_bound !== null && (C.lower_bound === null || C.lower_bound < P.lower_bound)) note('hard', 'R4', 'lower_bound');
      else if (C.lower_bound !== null && (P.lower_bound === null || C.lower_bound > P.lower_bound)) note('improved', 'R4', 'lower_bound');
    }
    const finished = FINISHED.has(C.status) && FINISHED.has(known(P.status) ? P.status : C.status);
    for (const field of ['expanded', 'generated']) {
      if (!finished || !both(field)) continue;
      if (C[field] > tolerance(P[field])) note('soft', 'S1', field);
      else if (C[field] < P[field]) note('improved', 'S1', field);
    }
    if (sameConfig && both('reserved_bytes')) {
      if (C.reserved_bytes > P.reserved_bytes) note('soft', 'S2', 'reserved_bytes');
      else if (C.reserved_bytes < P.reserved_bytes) note('improved', 'S2', 'reserved_bytes');
    }
    for (const field of new Set([...Object.keys(P), ...Object.keys(C)])) {
      if (flagged.has(field) || !known(C[field])) continue;
      if (!known(P[field])) note('recorded', null, field);
      else if (P[field] !== C[field]) note('changed', null, field);
    }
  }
  return out;
}

/**
 * Absolute rules with no escape hatch; any failure is a soundness bug.
 *   I1 only Optimal carries a proof or a lower bound;
 *   I2 the proof agrees with moves, lower_bound and status;
 *   I3 no Optimal bound or "optimal" proof lies above, and no "unsolvable"
 *      sits alongside, a replay-verified route from any mode or evidence case
 *      of the same id. Evidence with a fingerprint must match the current
 *      board; evidence without one (v1) must be pre-filtered by the caller.
 */
export function invariants(cases, evidence = []) {
  const failures = [], best = new Map();
  const fingerprints = new Map(cases.map(c => [c.id, c.fingerprint]));
  const offer = (id, moves) => {
    if (moves !== null && known(moves) && (!best.has(id) || moves < best.get(id))) best.set(id, moves);
  };
  for (const c of cases) offer(c.id, c.moves);
  for (const e of evidence) {
    if (fingerprints.has(e.id) && (!known(e.fingerprint) || e.fingerprint === fingerprints.get(e.id))) offer(e.id, e.moves);
  }
  for (const c of cases) {
    const fail = (rule, message) => failures.push({ key: key(c), rule, message });
    const { moves, lower_bound: lower, proof } = c;
    if (c.mode !== 'optimal') {
      if (proof !== 'none' || lower !== null) fail('I1', `${c.mode} claims proof=${proof} lower_bound=${lower}`);
      continue;
    }
    const consistent = {
      optimal: moves !== null && lower === moves,
      bounded: moves !== null && lower !== null && lower < moves,
      unsolvable: c.status === 'exhausted' && moves === null,
      none: moves === null,
    }[proof];
    if (!consistent) fail('I2', `proof=${proof} disagrees with moves=${moves} lower_bound=${lower} status=${c.status}`);
    if (!best.has(c.id)) continue;
    const route = best.get(c.id);
    if (lower !== null && lower > route) fail('I3', `lower bound ${lower} exceeds a verified ${route}-move route`);
    if (proof === 'optimal' && moves !== route) fail('I3', `claims ${moves} moves optimal but a ${route}-move route exists`);
    if (proof === 'unsolvable') fail('I3', `claims unsolvable but a ${route}-move route exists`);
  }
  return failures;
}

/** Per-mode totals; the delta between two scoreboards shows a change's net effect. */
export function scoreboard(cases) {
  const sum = (list, value) => list.reduce((total, c) => total + (value(c) ?? 0), 0);
  return MODES.map(mode => {
    const all = cases.filter(c => c.mode === mode);
    const finished = all.filter(c => FINISHED.has(c.status)), capped = all.filter(c => CAPPED.has(c.status));
    const proven = all.filter(c => PROOFS.has(c.proof)), routes = all.filter(c => c.moves !== null && known(c.moves));
    return {
      mode, runs: all.length, routes: routes.length, finished: finished.length, capped: capped.length,
      proofs: proven.length, movesSum: sum(routes, c => c.moves),
      cappedLowerBoundSum: mode === 'optimal' ? sum(capped, c => c.lower_bound) : 0,
      uncappedExpanded: sum(finished, c => c.expanded), uncappedGenerated: sum(finished, c => c.generated),
      uniqueStates: sum(all, c => c.stats?.unique_states), duplicateImprovements: sum(all, c => c.stats?.duplicate_improvements),
      recordsPerProof: proven.length ? Math.round(sum(proven, c => c.generated) / proven.length) : null,
    };
  });
}

/** Row-wise after - before for numeric scoreboard columns. */
export function scoreboardDelta(before, after) {
  return after.map((row, i) => Object.fromEntries(Object.entries(row).map(([name, value]) =>
    [name, typeof value === 'number' && typeof before[i]?.[name] === 'number' ? value - before[i][name] : value])));
}

const show = value => value === undefined ? '?' : JSON.stringify(value);
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

/** Report sections plus one greppable BENCH line. With catalogChangesAreHard,
 * missing and extra cases count as hard findings (check); update only lists them. */
export function formatReport({ action, cases, config, diff, failures, baseline, source, catalogChangesAreHard, limit = Infinity }) {
  const catalog = diff.missing.length + diff.extra.length;
  const hard = diff.hard.length + (catalogChangesAreHard ? catalog : 0);
  const lines = [
    ...(failures.length ? [`INVARIANT FAILURES (${failures.length}):`, ...failures.map(f => `  ${f.key}  ${f.rule}  ${f.message}`)] : ['Invariants: ok']),
    ...(diff.missing.length ? [`Missing cases (${diff.missing.length}): ${diff.missing.join(', ')}`] : []),
    ...(diff.extra.length ? [`Extra cases (${diff.extra.length}): ${diff.extra.join(', ')}`] : []),
    ...section('Hard', diff.hard, Infinity), ...section('Soft', diff.soft, Infinity),
    ...section('Improved', diff.improved, limit), ...section('Changed', diff.changed, limit),
    ...section('Recorded', diff.recorded, limit),
    `BENCH ${action} v${SCHEMA_VERSION}: cases=${cases.length} config=${config.maxStates}/${config.memoryMiB} hard=${hard}`
      + ` soft=${diff.soft.length} improved=${diff.improved.length} changed=${diff.changed.length} recorded=${diff.recorded.length}`
      + ` invariants=${failures.length ? `FAIL(${failures.length})` : 'ok'} baseline=${baseline ?? 'none'} source=${source}`,
  ];
  return { text: lines.join('\n'), hard };
}
