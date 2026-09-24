export interface SolveRequest { rows: string; actions: string; mode: string; maxStates: number; memoryMiB: number; timeMs: number }
export type WorkerRequest = { type: 'solve'; request: SolveRequest } | { type: 'cancel' };
export interface SearchUpdate { type: 'progress' | 'done'; status: string; metrics: Uint32Array; elapsedMs: number; route?: string }
export type WorkerReply = SearchUpdate | { type: 'error'; message: string };
