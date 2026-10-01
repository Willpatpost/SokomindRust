/** sokomind_core::MAX_ROUTE: the longest route Rust replays. */
export const MAX_ROUTE = 100_000;
/** sokomind_core::ReplayError::PastLimit's text, which POST /api/solve sends when the position's
 * moves plus the best route pass MAX_ROUTE. The worker and SolverClient show the same text. */
export const PAST_LIMIT_MESSAGE = `Position and route together exceed the ${MAX_ROUTE}-move replay limit`;
/** States per search; must equal the server and arena cap (sokomind_search::MAX_STATES).
 * A state costs about 28 B plus 2 B per box (record, queue entry, box cells and index
 * table), so the memory budget always binds first: on the catalog's boards 64 MiB holds
 * about 0.91-2.10M states, and 256 MiB, the largest budget a search accepts, 3.67-8.39M.
 * Every solve sends this cap, so its memory budget alone sizes the arena, which reserves
 * as many states as the budget holds up front, even on a small board. */
export const MAX_STATES = 60_000_000;
/** Matches sokomind_search::Mode::parse, which WasmSearch and POST /api/solve
 * apply to `mode`; also the values of the #mode select. */
export const MODES = ['fast', 'quality', 'optimal'] as const;
export type Mode = typeof MODES[number];
/** Matches sokomind_search::Status::as_str, which the worker and POST /api/solve report as `status`. */
export const STATUSES = ['running', 'solved', 'exhausted', 'state_limit', 'memory_limit', 'time_limit', 'cancelled'] as const;
export type SearchStatus = typeof STATUSES[number];
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
/** WasmGame.snapshot as decoded by transport's decodeSnapshot: the robot cell,
 * the move counters, and a view of the box cells in label order. */
export interface Snapshot { player: number; moves: number; pushes: number; solved: boolean; boxes: Uint32Array }
export interface SolveRequest {
  rows: string;
  actions: string;
  mode: Mode;
  maxStates: number;
  memoryMiB: number;
  timeMs: number;
}
export type WorkerRequest = { type: 'solve'; request: SolveRequest } | { type: 'cancel' };
// `route` carries a better verified route: throttled while running, and the
// final best on 'done' whenever it improved since the last one sent.
export interface SearchUpdate { type: 'progress' | 'done'; metrics: Metrics; elapsedMs: number; route?: string }
export type WorkerReply = SearchUpdate | { type: 'error'; message: string };
/** GET /api/progress/{id} as decoded by transport's decodeProgress: the profile's
 * stored best (Record in crates/server/src/progress.rs), with the server's counts. */
export interface ProgressRecord { puzzleId: string; route: string; moves: number; pushes: number }
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);
