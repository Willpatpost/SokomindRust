import * as storage from './storage.ts';

/** The proven-optimal cache: the move counts our engine has certified as
 * optimal, kept per room with the rows they were proven for — a changed
 * board never inherits a certificate. Records come only from searches that
 * ran from a room's start and finished with an optimal proof; the cache is
 * engine knowledge rather than user progress, so the progress reset keeps
 * it. */

export interface Optimum {
  rows: string;
  moves: number;
}

function fromValue(value: unknown): Record<string, Optimum> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const records: Record<string, Optimum> = {};
  for (const [id, record] of Object.entries(value as Record<string, unknown>)) {
    if (!record || typeof record !== 'object') continue;
    const candidate = record as Partial<Optimum>;
    if (typeof candidate.rows === 'string' && Number.isSafeInteger(candidate.moves) && (candidate.moves as number) >= 0)
      records[id] = { rows: candidate.rows, moves: candidate.moves as number };
  }
  return records;
}

export function load(): Record<string, Optimum> {
  return fromValue(storage.read('optimal'));
}

/** Records a certified optimum. A proof for changed rows replaces whatever
 * stood; a proof for the same rows keeps the lower count, though two sound
 * proofs of one room agree. */
export function record(id: string, rows: string, moves: number): void {
  const records = load();
  const current = records[id];
  if (current !== undefined && current.rows === rows && current.moves <= moves) return;
  records[id] = { rows, moves };
  storage.write('optimal', records);
}

/** The certified optimum for exactly these rows, or none. */
export function proven(id: string, rows: string): Optimum | undefined {
  const record = load()[id];
  return record !== undefined && record.rows === rows ? record : undefined;
}

/** Whether `moves` is optimal for these rows: no route can beat a certified
 * optimum, so matching it is the optimum. */
export function isOptimal(id: string, rows: string, moves: number): boolean {
  const record = proven(id, rows);
  return record !== undefined && moves <= record.moves;
}
