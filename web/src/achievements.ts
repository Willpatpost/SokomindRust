import { ENTRIES } from './catalog.ts';
import type { Aggregates } from './stats.ts';
import { dayKey, type Journal } from './journal.ts';

/** The seventeen achievements, ported from the original app's definitions and
 * evaluated over our aggregates and journal. Count and tier achievements read
 * the union solved signal — a dated record or a stored best — so they agree
 * with the selector rings and the home count; craft achievements read the
 * dated records, whose totals they measure; streaks read the journal's days.
 * Rewards never lock rooms or change board rules. */

export type CollectionId = 'exploration' | 'momentum' | 'curriculum' | 'craft';

export interface Collection {
  id: CollectionId;
  title: string;
  description: string;
}

export interface Progress {
  current: number;
  target: number;
  label: string;
  complete: boolean;
}

export interface Def {
  id: string;
  collection: CollectionId;
  title: string;
  description: string;
  icon: string;
  progress(a: Aggregates, book: Journal): Progress;
  check(a: Aggregates, book: Journal): boolean;
}

export const COLLECTIONS: readonly Collection[] = [
  { id: 'exploration', title: 'Room by room', description: 'Explore more of the open catalog at your own pace.' },
  { id: 'momentum', title: 'Steady practice', description: 'Build a routine across consecutive days.' },
  { id: 'curriculum', title: 'Catalog mastery', description: 'Clear complete difficulty tiers from tutorial through master.' },
  { id: 'craft', title: 'Route craft', description: 'Develop efficient routes and accumulate deliberate movement.' },
];

function bounded(current: number, target: number, label: string, complete = current >= target): Progress {
  return { current: Math.max(0, Math.min(current, target)), target, label, complete };
}

const count = (id: string, title: string, target: number, icon: string): Def => ({
  id,
  collection: 'exploration',
  title,
  description: `Solve ${target} ${target === 1 ? 'room' : 'rooms'}`,
  icon,
  progress: a => bounded(a.solved, target, `${Math.min(a.solved, target)} of ${target} rooms`),
  check: a => a.solved >= target,
});

const streak = (id: string, title: string, target: number, icon: string): Def => ({
  id,
  collection: 'momentum',
  title,
  description: `Maintain a ${target}-day streak`,
  icon,
  progress: a => bounded(a.streak.longest, target, `${Math.min(a.streak.longest, target)} of ${target} consecutive days`),
  check: a => a.streak.longest >= target,
});

const tier = (id: string, title: string, difficulty: string, icon: string): Def => ({
  id,
  collection: 'curriculum',
  title,
  description: `Clear every ${difficulty} room`,
  icon,
  progress: a => {
    const row = a.byDifficulty.find(entry => entry.difficulty === difficulty);
    const solved = row?.solved ?? 0;
    const total = row?.total ?? 0;
    return bounded(solved, Math.max(total, 1), `${solved} of ${total} ${difficulty} rooms`, total > 0 && solved === total);
  },
  check: a => {
    const row = a.byDifficulty.find(entry => entry.difficulty === difficulty);
    return row !== undefined && row.total > 0 && row.solved === row.total;
  },
});

const moves = (id: string, title: string, target: number, icon: string): Def => ({
  id,
  collection: 'craft',
  title,
  description: `Record ${target.toLocaleString('en-US')} total moves in cleared rooms`,
  icon,
  progress: a =>
    bounded(a.moves, target, `${Math.min(a.moves, target).toLocaleString('en-US')} of ${target.toLocaleString('en-US')} recorded moves`),
  check: a => a.moves >= target,
});

export const ACHIEVEMENTS: readonly Def[] = [
  count('first-solve', 'First Steps', 1, '1'),
  count('ten-solved', 'Getting Started', 10, '10'),
  count('fifty-solved', 'Puzzle Enthusiast', 50, '50'),
  count('hundred-solved', 'Century Club', 100, 'C'),
  {
    id: 'all-solved',
    collection: 'exploration',
    title: 'Completionist',
    description: 'Clear every room in the catalog',
    icon: 'A',
    progress: a => bounded(a.solved, Math.max(a.total, 1), `${a.solved} of ${a.total} catalog rooms`, a.total > 0 && a.solved === a.total),
    check: a => a.total > 0 && a.solved === a.total,
  },
  streak('streak-3', 'Three-Peat', 3, '3'),
  streak('streak-7', 'Full Week', 7, '7'),
  streak('streak-30', 'Monthly Devotion', 30, '30'),
  tier('tutorial-complete', 'Graduate', 'tutorial', 'T'),
  tier('beginner-complete', 'Beyond Beginner', 'beginner', 'B'),
  tier('intermediate-complete', 'Intermediate Master', 'intermediate', 'I'),
  tier('advanced-complete', 'Advanced Studies', 'advanced', 'A'),
  tier('expert-complete', 'Expert Eye', 'expert', 'E'),
  tier('master-complete', 'Master of Rooms', 'master', 'M'),
  {
    id: 'efficient-mover',
    collection: 'craft',
    title: 'Efficient Mover',
    description: 'Clear 10 rooms while averaging under 5 pushes',
    icon: 'E',
    progress: a => {
      const enough = a.recorded >= 10;
      const efficient = a.averagePushes < 5;
      const average = a.recorded > 0 ? a.averagePushes.toFixed(1) : '—';
      return bounded(
        enough && efficient ? 10 : Math.min(a.recorded, efficient ? 10 : 9),
        10,
        `${Math.min(a.recorded, 10)} of 10 rooms · ${average} pushes average (under 5 needed)`,
        enough && efficient,
      );
    },
    check: a => a.recorded >= 10 && a.averagePushes < 5,
  },
  moves('thousand-moves', 'Well-Traveled', 1_000, '1K'),
  moves('ten-thousand-moves', 'Marathon Runner', 10_000, '10K'),
];

export function unlocked(a: Aggregates, book: Journal): readonly Def[] {
  return ACHIEVEMENTS.filter(def => def.check(a, book));
}

export function unlockedIds(a: Aggregates, book: Journal): Set<string> {
  return new Set(unlocked(a, book).map(def => def.id));
}

/** The achievements `after` holds that `before` did not: the unlock moment. */
export function newlyUnlocked(before: Set<string>, after: Set<string>): readonly Def[] {
  return ACHIEVEMENTS.filter(def => after.has(def.id) && !before.has(def.id));
}

export interface Milestone {
  id: string;
  title: string;
  description: string;
  collectionTitle: string;
  earnedAt: string;
}

/** The known rooms' clears, oldest first: the record milestone dates read. */
function completions(book: Journal): { id: string; clearedAt: string }[] {
  const known = new Set(ENTRIES.map(entry => entry.id));
  return Object.entries(book.cleared)
    .filter(([id]) => known.has(id))
    .map(([id, clear]) => ({ id, clearedAt: clear.clearedAt }))
    .sort((left, right) => left.clearedAt.localeCompare(right.clearedAt));
}

/** When a count or tier achievement was completed, from the dated records. */
function completionDate(def: Def, book: Journal): string | null {
  const dates = completions(book);
  const targets: Record<string, number> = { 'first-solve': 1, 'ten-solved': 10, 'fifty-solved': 50, 'hundred-solved': 100 };
  const target = targets[def.id];
  if (target !== undefined) return dates[target - 1]?.clearedAt ?? null;
  if (def.id === 'all-solved') return dates.length === ENTRIES.length ? (dates[dates.length - 1]?.clearedAt ?? null) : null;
  if (def.collection !== 'curriculum') return null;
  const difficulty = def.id.replace('-complete', '');
  const tierIds = new Set(ENTRIES.filter(entry => entry.difficulty === difficulty).map(entry => entry.id));
  const tierDates = dates.filter(record => tierIds.has(record.id));
  return tierDates.length === tierIds.size ? (tierDates[tierDates.length - 1]?.clearedAt ?? null) : null;
}

/** When a streak achievement's run reached its target day. */
function streakDate(def: Def, book: Journal): string | null {
  const target = def.id === 'streak-3' ? 3 : def.id === 'streak-7' ? 7 : def.id === 'streak-30' ? 30 : 0;
  if (target === 0) return null;
  const days = Object.keys(book.activity)
    .filter(key => (book.activity[key]?.length ?? 0) > 0)
    .sort();
  let run = 0;
  let previous: number | null = null;
  for (const key of days) {
    const [year, month, day] = key.split('-').map(Number);
    const ordinal = Math.floor(Date.UTC(year!, month! - 1, day!) / 86_400_000);
    run = previous !== null && ordinal - previous === 1 ? run + 1 : 1;
    previous = ordinal;
    if (run >= target) return `${key}T12:00:00.000Z`;
  }
  return null;
}

/** The newest earned achievements, with the dates their records show. */
export function milestones(a: Aggregates, book: Journal, limit = 6): readonly Milestone[] {
  const titles = new Map(COLLECTIONS.map(collection => [collection.id, collection.title] as const));
  return unlocked(a, book)
    .flatMap(def => {
      const earnedAt = completionDate(def, book) ?? streakDate(def, book);
      return earnedAt
        ? [
            {
              id: def.id,
              title: def.title,
              description: def.description,
              collectionTitle: titles.get(def.collection) ?? 'Achievement',
              earnedAt,
            },
          ]
        : [];
    })
    .sort((left, right) => right.earnedAt.localeCompare(left.earnedAt))
    .slice(0, Math.min(12, Math.max(0, Math.floor(limit))));
}

/** A milestone date as it reads: Oct 20, 2026. */
export function milestoneLabel(earnedAt: string): string {
  return new Intl.DateTimeFormat('en', { month: 'short', day: 'numeric', year: 'numeric' }).format(new Date(earnedAt));
}

/** The UTC day a milestone's instant names, for tests that pin it. */
export function milestoneDay(earnedAt: string): string {
  return dayKey(new Date(earnedAt));
}
