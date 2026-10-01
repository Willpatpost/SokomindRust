import {
  MAX_ROUTE, STATUSES, type Metrics, type ProgressRecord, type Proof, type SearchStatus, type SearchUpdate, type Snapshot,
  type SolveRequest, type WorkerReply,
} from './protocol.ts';

type ObjectValue = Record<string, unknown>;
function object(value: unknown, name = 'solver response'): ObjectValue {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`Invalid ${name}`);
  return value as ObjectValue;
}
function counter(value: unknown, name: string, what = 'solver'): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error(`Invalid ${what} ${name}`);
  return value;
}
const optionalCounter = (value: unknown, name: string) => value == null ? undefined : counter(value, name);
function status(value: unknown): SearchStatus {
  const known = STATUSES.find(item => item === value);
  if (known === undefined) throw new Error('Unknown solver status');
  return known;
}
function route(value: unknown, what = 'solver'): string | undefined {
  if (value == null) return undefined;
  if (typeof value !== 'string' || value.length > MAX_ROUTE || !/^[UDLR]*$/.test(value))
    throw new Error(`Invalid ${what} route`);
  return value;
}
function proof(value: unknown): Proof {
  const p = object(value);
  switch (p.kind) {
    case 'none': return { kind: 'none' };
    case 'unsolvable': return { kind: 'unsolvable' };
    case 'optimal': return { kind: 'optimal', moves: counter(p.moves, 'optimal moves') };
    case 'bounded': {
      const lower = counter(p.lower, 'lower bound'), upper = counter(p.upper, 'upper bound');
      if (lower > upper) throw new Error('Invalid solver proof bounds');
      return { kind: 'bounded', lower, upper };
    }
    default: throw new Error('Unknown solver proof kind');
  }
}
function metrics(value: unknown): Metrics {
  const m = object(value);
  const result: Metrics = {
    expanded: counter(m.expanded, 'expanded'), generated: counter(m.generated, 'generated'),
    reservedBytes: counter(m.reservedBytes, 'reserved bytes'),
    best: optionalCounter(m.best, 'best moves'), lowerBound: optionalCounter(m.lowerBound, 'lower bound'),
    proof: proof(m.proof), status: status(m.status),
  };
  // `best` is only a count, which saturates in Rust instead of stopping at MAX_ROUTE, so a legal
  // search may report one past it; route() caps every route actually sent.
  if (result.best !== undefined && result.lowerBound !== undefined && result.lowerBound > result.best)
    throw new Error('Invalid solver bounds');
  if (result.proof.kind === 'unsolvable' && (result.best !== undefined || result.status !== 'exhausted'))
    throw new Error('Inconsistent unsolvable proof');
  if (result.proof.kind === 'optimal' && result.proof.moves !== result.best)
    throw new Error('Inconsistent optimal proof');
  if (result.proof.kind === 'bounded' && (result.proof.upper !== result.best || result.proof.lower !== result.lowerBound))
    throw new Error('Inconsistent bounded proof');
  return result;
}
/** A worker message as a WorkerReply; throws on anything malformed or inconsistent. */
export function decodeWorkerReply(value: unknown): WorkerReply {
  const v = object(value);
  if (v.type === 'error') {
    if (typeof v.message !== 'string') throw new Error('Invalid worker error');
    return { type: 'error', message: v.message };
  }
  if (v.type !== 'progress' && v.type !== 'done') throw new Error('Unknown worker reply');
  return decodeUpdate(v, v.type);
}
/** The checks a worker update and a native reply share, on the worker's field names. */
function decodeUpdate(v: ObjectValue, type: SearchUpdate['type']): SearchUpdate {
  const m = metrics(v.metrics), r = route(v.route);
  if ((type === 'progress') !== (m.status === 'running')) throw new Error('Inconsistent solver completion');
  if (r !== undefined && r.length !== m.best) throw new Error('Route and move count disagree');
  return { type, metrics: m, route: r, elapsedMs: elapsed(v.elapsedMs) };
}
function elapsed(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) throw new Error('Invalid solver elapsed time');
  return value;
}
/** POST /api/solve's ResultBody (crates/server/src/solve.rs) as a done SearchUpdate, held to the
 * checks a worker's final update gets. */
export function decodeNativeReply(value: unknown): SearchUpdate {
  const v = object(value);
  let p: Proof = { kind: 'none' }, lower: number | undefined;
  if (v.proof != null) {
    const native = object(v.proof);
    switch (native.kind) {
      case 'optimal':
        lower = counter(native.lower_bound, 'lower bound');
        p = { kind: 'optimal', moves: counter(native.upper_bound, 'upper bound') };
        if (lower !== p.moves) throw new Error('Invalid optimal proof bounds');
        break;
      case 'bounded':
        lower = counter(native.lower_bound, 'lower bound');
        p = { kind: 'bounded', lower, upper: counter(native.upper_bound, 'upper bound') };
        break;
      case 'unsolvable': p = { kind: 'unsolvable' }; break;
      default: throw new Error('Unknown solver proof kind');
    }
  }
  const r = route(v.route), moves = optionalCounter(v.moves, 'moves'), pushes = optionalCounter(v.pushes, 'pushes');
  if ((r !== undefined) !== (moves !== undefined) || (r !== undefined) !== (pushes !== undefined)
    || (pushes !== undefined && moves !== undefined && pushes > moves)) throw new Error('Invalid native route counters');
  return decodeUpdate({ route: r, elapsedMs: v.elapsed_ms, metrics: {
    expanded: v.expanded, generated: v.generated, reservedBytes: v.reserved_bytes,
    best: moves, lowerBound: lower, proof: p, status: v.status,
  } }, 'done');
}
/** WASM metrics ABI: first six u32s are stable; appended telemetry is optional. */
export function decodeMetricTuple(values: ArrayLike<number>, searchStatus: unknown): Metrics {
  if (values.length < 6) throw new Error('Invalid WASM metrics tuple');
  const [expanded, generated, reservedBytes, best, kind, lower] = Array.from(values).slice(0, 6);
  const absent = 0xffffffff;
  let p: Proof;
  switch (kind) {
    case 0: p = { kind: 'none' }; break;
    case 1: p = { kind: 'bounded', lower, upper: best }; break;
    case 2: p = { kind: 'optimal', moves: best }; break;
    case 3: p = { kind: 'unsolvable' }; break;
    default: throw new Error('Unknown WASM proof kind');
  }
  return metrics({ expanded, generated, reservedBytes, best: best === absent ? undefined : best,
    lowerBound: lower === absent ? undefined : lower, proof: p, status: searchStatus });
}
/** WASM snapshot ABI (WasmGame.snapshot): player cell, moves, pushes, solved
 * (0 or 1), then exactly `boxCount` box cells in label order. `boxes` is a view
 * into `values`, not a copy. */
export function decodeSnapshot(values: Uint32Array, boxCount: number): Snapshot {
  if (values.length !== 4 + boxCount) throw new Error('Invalid WASM snapshot length');
  const player = values[0], moves = values[1], pushes = values[2], solved = values[3];
  if (solved > 1 || pushes > moves || moves > MAX_ROUTE) throw new Error('Invalid WASM snapshot counters');
  return { player, moves, pushes, solved: solved === 1, boxes: values.subarray(4) };
}
/** POST /api/solve's body, in the snake_case of Request in crates/server/src/solve.rs, which
 * rejects unknown fields. */
export function encodeSolveRequest(request: SolveRequest): string {
  return JSON.stringify({ rows: request.rows.split('\n'), actions: request.actions, mode: request.mode,
    time_ms: request.timeMs, max_states: request.maxStates, memory_mib: request.memoryMiB });
}
/** The message for a failed POST /api/solve. Each 429 text a solve can get from the API or nginx
 * names the browser solver (the busy and rate-limit answers in crates/server/src/api.rs and
 * solve.rs, and @api_limited in deploy/nginx.conf), so a sent error is shown as is and only a 429
 * whose body carries no text gains that pointer. */
export function nativeErrorText(status: number, error: string | undefined): string {
  if (error !== undefined) return error;
  const text = `Server returned HTTP ${status}`;
  return status === 429 ? `${text}. The browser solver still works: set Run on to This browser.` : text;
}
/** GET /api/health's `persistence`, or undefined when the reply is not an object (as when a host
 * without the API serves an app page or an empty body); an array or an object without
 * `persistence: true` reports false. */
export function decodeHealth(value: unknown): boolean | undefined {
  return value && typeof value === 'object' ? (value as { persistence?: unknown }).persistence === true : undefined;
}
/** POST /api/progress/{id}'s `improved`: whether the route replaced the stored best. */
export function decodeSaveReply(value: unknown): boolean {
  const v = object(value, 'progress response');
  if (typeof v.improved !== 'boolean') throw new Error('Invalid progress response');
  return v.improved;
}
/** GET /api/progress/{id}'s stored best. The counts are the server's claim: the caller replays the
 * route and compares them before keeping it. */
export function decodeProgress(value: unknown): ProgressRecord {
  const v = object(value, 'progress record'), stored = route(v.route, 'progress');
  if (typeof v.puzzle_id !== 'string' || stored === undefined) throw new Error('Invalid progress record');
  return {
    puzzleId: v.puzzle_id, route: stored, moves: counter(v.moves, 'moves', 'progress'), pushes: counter(v.pushes, 'pushes', 'progress'),
  };
}
/** A failed response's JSON `error` text, or undefined when its body carries none. */
export async function errorText(response: Response): Promise<string | undefined> {
  const value: unknown = await response.json().catch(() => null);
  if (!value || typeof value !== 'object') return undefined;
  const error = (value as ObjectValue).error;
  return typeof error === 'string' ? error : undefined;
}
/** Browsers brand-check fetch's receiver ("Illegal invocation"), so a stored
 * fetch must never be called as a method. The wrapper always calls with an
 * undefined receiver and looks the global up per call, so tests can swap it. */
export function unboundFetch(custom?: typeof fetch): typeof fetch {
  return custom ? (input, init) => custom(input, init) : (input, init) => fetch(input, init);
}
