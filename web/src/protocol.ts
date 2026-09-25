/** sokomind_core::MAX_ROUTE: the longest route Rust replays. */
export const MAX_ROUTE = 100_000;
/** States per search; must stay within the server's MAX_STATES cap in solve.rs. */
export const MAX_STATES = 500_000;
/** Matches sokomind_search::Status::as_str. */
export type SearchStatus =
  'running' | 'solved' | 'exhausted' | 'state_limit' | 'memory_limit' | 'time_limit' | 'cancelled';
export type Proof =
  | { kind: 'optimal'; moves: number }
  | { kind: 'bounded'; lower: number; upper: number }
  | { kind: 'unsolvable' }
  | { kind: 'none' };
/** `best` is the best route's move count and `lowerBound` the live certified bound, when known. */
export interface Metrics {
  expanded: number;
  generated: number;
  reservedBytes: number;
  best?: number;
  lowerBound?: number;
  proof: Proof;
  status: SearchStatus;
}
export interface SolveRequest {
  rows: string;
  actions: string;
  mode: string;
  maxStates: number;
  memoryMiB: number;
  timeMs: number;
}
export type WorkerRequest = { type: 'solve'; request: SolveRequest } | { type: 'cancel' };
// `route` carries a better verified route: throttled while running, and the
// final best on 'done' whenever it improved since the last one sent.
export interface SearchUpdate { type: 'progress' | 'done'; metrics: Metrics; elapsedMs: number; route?: string }
export type WorkerReply = SearchUpdate | { type: 'error'; message: string };
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);
