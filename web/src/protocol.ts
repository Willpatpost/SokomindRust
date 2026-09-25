export interface SolveRequest { rows: string; actions: string; mode: string; maxStates: number; memoryMiB: number; timeMs: number }
export type WorkerRequest = { type: 'solve'; request: SolveRequest } | { type: 'cancel' };
// `route` carries a better verified route: throttled while running, and the
// final best on 'done' whenever it improved since the last one sent.
export interface SearchUpdate { type: 'progress' | 'done'; status: string; metrics: Uint32Array; elapsedMs: number; route?: string }
export type WorkerReply = SearchUpdate | { type: 'error'; message: string };
