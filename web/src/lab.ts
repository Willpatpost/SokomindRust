import type { Mode, SearchStatus } from './protocol.ts';

/** The Solver Lab's model: one run's record, and the comparison of two —
 * what they shared, and where their work and routes differ. The records
 * live only in the workspace's memory, newest first and at most six; the
 * comparison reads them without touching the DOM. */

export interface RunRecord {
  id: string;
  puzzleId: string;
  /** The moves the search started from — the lab's transferred position. */
  actions: string;
  mode: Mode;
  timeMs: number;
  memoryMiB: number;
  engine: string;
  status: SearchStatus;
  expanded: number;
  generated: number;
  reservedBytes: number;
  elapsedMs: number;
  /** The verified route's counts, when the search returned one. */
  moves?: number;
  pushes?: number;
  /** The transferred position with the route appended, replay-verified. */
  verifiedLog?: string;
  capturedAt: string;
}

/** A run's name in the watch and compare selects. */
export function label(record: RunRecord): string {
  const outcome = record.moves !== undefined ? `${record.moves} moves` : record.status.replace('_', ' ');
  return `${record.mode} · ${record.engine} · ${outcome}`;
}

export interface PopulationRow {
  id: 'generated' | 'visited';
  label: string;
  value: number;
  /** The bar's share of the larger population. */
  share: number;
  description: string;
}

/** The telemetry panel's populations, as the engine's own counters name
 * them: generated counts successor states produced, visited counts states
 * expanded. */
export function population(expanded: number, generated: number): PopulationRow[] {
  const larger = Math.max(1, expanded, generated);
  return [
    {
      id: 'generated',
      label: 'Generated',
      value: generated,
      share: generated / larger,
      description: 'Successor states produced before duplicate decisions.',
    },
    {
      id: 'visited',
      label: 'Visited',
      value: expanded,
      share: expanded / larger,
      description: 'States removed from the frontier and expanded.',
    },
  ];
}

export interface RunComparison {
  sameInput: boolean;
  sameLimits: boolean;
  elapsedDeltaMs: number;
  expandedDelta: number;
  generatedDelta: number;
  moveDelta?: number;
  pushDelta?: number;
}

/** Compares two runs: whether they searched the same position under the
 * same limits, and the signed gaps in their work and routes. Route deltas
 * appear only when both runs returned one. */
export function compareRuns(primary: RunRecord, reference: RunRecord): RunComparison {
  return {
    sameInput: primary.puzzleId === reference.puzzleId && primary.actions === reference.actions,
    sameLimits: primary.mode === reference.mode && primary.timeMs === reference.timeMs && primary.memoryMiB === reference.memoryMiB,
    elapsedDeltaMs: primary.elapsedMs - reference.elapsedMs,
    expandedDelta: primary.expanded - reference.expanded,
    generatedDelta: primary.generated - reference.generated,
    ...(primary.moves !== undefined && reference.moves !== undefined && primary.pushes !== undefined && reference.pushes !== undefined
      ? { moveDelta: primary.moves - reference.moves, pushDelta: primary.pushes - reference.pushes }
      : {}),
  };
}
