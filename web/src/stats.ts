import { DIFFICULTIES, ENTRIES, solved } from './catalog.ts';
import { solvingStreak, type Journal } from './journal.ts';

/** The stats page's read of the journal: what a solved room is worth in
 * aggregate — counts, totals, tiers, efficiency and the streak — computed
 * without touching the DOM, so the page renders and the units test the same
 * numbers. */

/** The tier colors the stats page paints, the original app's map on our
 * tokens. */
export const TIER_COLORS: Record<string, string> = {
  tutorial: 'var(--sage-500)',
  beginner: 'var(--sage-600)',
  intermediate: 'var(--blue-500)',
  advanced: 'var(--amber-500)',
  expert: 'var(--coral-500)',
  master: 'var(--ink-700)',
};

/** One tier's completion count. */
export interface TierStat {
  difficulty: string;
  solved: number;
  total: number;
}

/** The most efficient clear: fewest pushes per box. */
export interface Efficiency {
  title: string;
  pushes: number;
  boxes: number;
}

export interface Aggregates {
  /** Rooms cleared by either signal: the journal's dated record, or the
   * stored best from before the journal began. */
  solved: number;
  total: number;
  /** The solved share, 0 to 100. */
  completion: number;
  /** The clears the journal holds with their dates, whose totals follow. */
  recorded: number;
  moves: number;
  pushes: number;
  elapsedMs: number;
  averagePushes: number;
  byDifficulty: TierStat[];
  efficiency: Efficiency | null;
  streak: { current: number; longest: number };
}

/** The aggregates over the journal: a room counts as solved when either the
 * journal or a stored best says so, so every page agrees on "cleared", while
 * the totals count only the dated records the journal holds. */
export function aggregates(journal: Journal, now: Date = new Date()): Aggregates {
  const byDifficulty: TierStat[] = [];
  for (const difficulty of DIFFICULTIES) {
    const tier = ENTRIES.filter(entry => entry.difficulty === difficulty);
    byDifficulty.push({
      difficulty,
      solved: tier.filter(entry => journal.cleared[entry.id] !== undefined || solved(entry)).length,
      total: tier.length,
    });
  }
  let recorded = 0;
  let moves = 0;
  let pushes = 0;
  let elapsedMs = 0;
  let efficiency: Efficiency | null = null;
  let bestRatio = Infinity;
  for (const entry of ENTRIES) {
    const clear = journal.cleared[entry.id];
    if (!clear) continue;
    recorded++;
    moves += clear.moves;
    pushes += clear.pushes;
    if (clear.elapsedMs !== undefined) elapsedMs += clear.elapsedMs;
    if (entry.boxes > 0 && clear.pushes / entry.boxes < bestRatio) {
      bestRatio = clear.pushes / entry.boxes;
      efficiency = { title: entry.title, pushes: clear.pushes, boxes: entry.boxes };
    }
  }
  const solvedCount = byDifficulty.reduce((sum, tier) => sum + tier.solved, 0);
  return {
    solved: solvedCount,
    total: ENTRIES.length,
    completion: ENTRIES.length === 0 ? 0 : (solvedCount / ENTRIES.length) * 100,
    recorded,
    moves,
    pushes,
    elapsedMs,
    averagePushes: recorded === 0 ? 0 : pushes / recorded,
    byDifficulty,
    efficiency,
    streak: solvingStreak(journal, now),
  };
}

/** The heat map's five levels, by the rooms a day cleared. */
export function heatLevel(count: number): number {
  if (count === 0) return 0;
  if (count <= 2) return 1;
  if (count <= 5) return 2;
  if (count <= 10) return 3;
  return 4;
}

/** How a played duration reads: minutes, or hours and minutes. */
export function formatDuration(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`;
}

/** The clipboard's summary of the aggregates, one line per fact. */
export function statsText(stats: Aggregates): string {
  return [
    'Sokomind Rust Stats',
    `${stats.solved} of ${stats.total} rooms solved (${Math.round(stats.completion)}%)`,
    `${stats.moves.toLocaleString()} total moves, ${stats.pushes.toLocaleString()} total pushes`,
    `${stats.averagePushes.toFixed(1)} avg pushes per puzzle`,
    `Streak: ${stats.streak.current} days current, ${stats.streak.longest} days best`,
    '',
    ...stats.byDifficulty.map(tier => `${tier.difficulty}: ${tier.solved}/${tier.total}`),
  ].join('\n');
}
