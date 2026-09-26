import { MAX_ROUTE, type Metrics, type Proof, type SearchStatus, type SearchUpdate, type WorkerReply } from './protocol.ts';

type ObjectValue = Record<string, unknown>;
function object(value: unknown): ObjectValue {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid solver response');
  return value as ObjectValue;
}
export function counter(value: unknown, name: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error(`Invalid solver ${name}`);
  return value;
}
const optionalCounter = (value: unknown, name: string) => value == null ? undefined : counter(value, name);
export function status(value: unknown): SearchStatus {
  switch (value) {
    case 'running': case 'solved': case 'exhausted': case 'state_limit': case 'memory_limit': case 'time_limit': case 'cancelled': return value;
    default: throw new Error('Unknown solver status');
  }
}
export function route(value: unknown): string | undefined {
  if (value == null) return undefined;
  if (typeof value !== 'string' || value.length > MAX_ROUTE || !/^[UDLR]*$/.test(value))
    throw new Error('Invalid solver route');
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
  if (result.best !== undefined && result.best > MAX_ROUTE) throw new Error('Invalid solver best moves');
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
export function decodeWorkerReply(value: unknown): WorkerReply {
  const v = object(value);
  if (v.type === 'error') {
    if (typeof v.message !== 'string') throw new Error('Invalid worker error');
    return { type: 'error', message: v.message };
  }
  if (v.type !== 'progress' && v.type !== 'done') throw new Error('Unknown worker reply');
  const m = metrics(v.metrics), r = route(v.route);
  if ((v.type === 'progress') !== (m.status === 'running')) throw new Error('Inconsistent solver completion');
  if (r !== undefined && r.length !== m.best) throw new Error('Route and move count disagree');
  return { type: v.type, metrics: m, route: r, elapsedMs: elapsed(v.elapsedMs) };
}
function elapsed(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) throw new Error('Invalid solver elapsed time');
  return value;
}
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
  const decoded = decodeWorkerReply({ type: 'done', route: r, elapsedMs: v.elapsed_ms, metrics: {
    expanded: v.expanded, generated: v.generated, reservedBytes: v.reserved_bytes,
    best: moves, lowerBound: lower, proof: p, status: v.status,
  } });
  if (decoded.type === 'error') throw new Error(decoded.message);
  return decoded;
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
export async function errorText(response: Response): Promise<string | undefined> {
  const value: unknown = await response.json().catch(() => null);
  if (!value || typeof value !== 'object') return undefined;
  const error = (value as ObjectValue).error;
  return typeof error === 'string' ? error : undefined;
}
