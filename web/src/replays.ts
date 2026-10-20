import type { KeyValue } from './idb.ts';
import { idb } from './idb.ts';

/** The personal-best routes repository: up to eight replay-verified routes
 * per room in IndexedDB, each one an improvement over the best before it,
 * bounded in total so the store cannot grow without end. A route enters
 * only after the engine replays it on its room's rows and confirms its
 * counts — the verifier is injected, keeping the Rust engine the source of
 * truth — and every read verifies again, so a tampered or stale route never
 * reaches the shelf. */

export const MAX_PER_PUZZLE = 8;
const MAX_ACTIONS_PER_ROUTE = 25_000;
const MAX_ROUTES = 512;
const MAX_PUZZLES = 250;
const MAX_ACTIONS = 2_000_000;
const KEY = 'personal-best-routes';

/** What the engine answers when it replays a route: its counts, or nothing
 * when the route does not solve the rows. */
export type Verify = (rows: string, actionLog: string) => { moves: number; pushes: number } | null;

/** A route as the repository stores it, identity and all. */
export interface Route {
  routeId: string;
  actionLog: string;
  moves: number;
  pushes: number;
  completedAt: string;
  elapsedMs?: number;
}

/** One room's rows with its routes, best first. */
export interface History {
  rows: string;
  routes: Route[];
}

export interface Repository {
  puzzles: Record<string, History>;
}

/** A route on its way in, before verification names it. */
export interface Candidate {
  actionLog: string;
  moves: number;
  pushes: number;
  completedAt: string;
  elapsedMs?: number;
}

export type Promotion = 'saved' | 'not-better' | 'rejected' | 'unavailable';

export interface Read {
  status: 'ready' | 'missing' | 'stale' | 'corrupt' | 'unavailable';
  routes: Route[];
}

const ACTION_LOG = /^[UDLR]*$/;

function hashText(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index++) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16).padStart(8, '0');
}

function isTimestamp(value: unknown): value is string {
  if (typeof value !== 'string') return false;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) && new Date(parsed).toISOString() === value;
}

function isCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}

/** Best first: fewer moves, then fewer pushes, then the newer save. */
function compareRoutes(first: Route, second: Route): number {
  if (first.moves !== second.moves) return first.moves - second.moves;
  if (first.pushes !== second.pushes) return first.pushes - second.pushes;
  const saved = second.completedAt.localeCompare(first.completedAt);
  return saved !== 0 ? saved : first.routeId.localeCompare(second.routeId);
}

/** A stored route's shape, without replaying it — the structural half of
 * verification; the engine's replay is the other. */
function asRoute(value: unknown): Route | null {
  if (!value || typeof value !== 'object') return null;
  const route = value as Partial<Route>;
  if (
    typeof route.routeId !== 'string'
    || route.routeId.length === 0
    || route.routeId.length > 160
    || typeof route.actionLog !== 'string'
    || !ACTION_LOG.test(route.actionLog)
    || route.actionLog.length > MAX_ACTIONS_PER_ROUTE
    || !isCount(route.moves)
    || route.moves !== route.actionLog.length
    || !isCount(route.pushes)
    || route.pushes > route.moves
    || !isTimestamp(route.completedAt)
    || (route.elapsedMs !== undefined && (typeof route.elapsedMs !== 'number' || !Number.isFinite(route.elapsedMs) || route.elapsedMs < 0))
  )
    return null;
  return {
    routeId: route.routeId,
    actionLog: route.actionLog,
    moves: route.moves,
    pushes: route.pushes,
    completedAt: route.completedAt,
    ...(route.elapsedMs === undefined ? {} : { elapsedMs: Math.round(route.elapsedMs) }),
  };
}

/** Verifies a candidate on its rows — the engine must replay it to exactly
 * its counts — and names it: the save time, the moves and a hash of the
 * route, so a stored route's identity always matches its content. */
export function verified(rows: string, candidate: Candidate, verify: Verify): Route | null {
  const shaped = asRoute({ ...candidate, routeId: 'pending' });
  if (shaped === null) return null;
  const replayed = verify(rows, candidate.actionLog);
  if (replayed === null || replayed.moves !== candidate.moves || replayed.pushes !== candidate.pushes) return null;
  return {
    routeId: `${candidate.completedAt}:${candidate.moves}:${hashText(candidate.actionLog)}`,
    actionLog: candidate.actionLog,
    moves: candidate.moves,
    pushes: candidate.pushes,
    completedAt: candidate.completedAt,
    ...(candidate.elapsedMs === undefined || candidate.elapsedMs <= 0 ? {} : { elapsedMs: Math.round(candidate.elapsedMs) }),
  };
}

/** Bounds the repository: the newest 250 rooms, then each room's best
 * route, then its second, and so on, until 512 routes or two million moves
 * — so summary progress survives even when an old route falls outside. */
export function bound(repository: Repository): Repository {
  const histories = Object.entries(repository.puzzles)
    .sort(([, first], [, second]) => (second.routes[0]?.completedAt ?? '').localeCompare(first.routes[0]?.completedAt ?? ''))
    .slice(0, MAX_PUZZLES);
  const selected = new Map<string, Route[]>();
  let routeCount = 0;
  let actionCount = 0;
  for (let rank = 0; rank < MAX_PER_PUZZLE; rank++) {
    for (const [id, history] of histories) {
      const route = history.routes[rank];
      if (route === undefined) continue;
      if (routeCount >= MAX_ROUTES || actionCount + route.actionLog.length > MAX_ACTIONS) continue;
      const kept = selected.get(id) ?? [];
      kept.push(route);
      selected.set(id, kept);
      routeCount++;
      actionCount += route.actionLog.length;
    }
  }
  const puzzles: Record<string, History> = {};
  for (const [id, history] of histories) {
    const routes = selected.get(id);
    if (routes !== undefined && routes.length > 0) puzzles[id] = { rows: history.rows, routes };
  }
  return { puzzles };
}

/** The repository a stored value names, or an empty one: invalid records
 * drop, duplicates drop, and what survives sorts best-first within its
 * bounds. */
export function normalize(value: unknown): Repository {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return { puzzles: {} };
  const source = value as Partial<Repository>;
  if (!source.puzzles || typeof source.puzzles !== 'object' || Array.isArray(source.puzzles)) return { puzzles: {} };
  const puzzles: Record<string, History> = {};
  for (const [id, history] of Object.entries(source.puzzles)) {
    if (!history || typeof history !== 'object') continue;
    const candidate = history as Partial<History>;
    if (typeof candidate.rows !== 'string' || !Array.isArray(candidate.routes)) continue;
    const routeIds = new Set<string>();
    const routes: Route[] = [];
    for (const raw of candidate.routes) {
      const route = asRoute(raw);
      if (route === null || routeIds.has(route.routeId)) continue;
      routeIds.add(route.routeId);
      routes.push(route);
    }
    if (routes.length === 0) continue;
    routes.sort(compareRoutes);
    puzzles[id] = { rows: candidate.rows, routes: routes.slice(0, MAX_PER_PUZZLE) };
  }
  return bound({ puzzles });
}

/** Promotes a verified route into its room's history: only a route shorter
 * than the best before it enters — every save is an improvement — and the
 * bound keeps the repository finite. */
export async function promote(store: KeyValue, id: string, rows: string, candidate: Candidate, verify: Verify): Promise<Promotion> {
  const route = verified(rows, candidate, verify);
  if (route === null) return 'rejected';
  let promoted = false;
  const applied = await store.update(KEY, stored => {
    const repository = normalize(stored);
    const existing = repository.puzzles[id];
    const compatible = existing !== undefined && existing.rows === rows ? existing.routes.slice(0, MAX_PER_PUZZLE - 1) : [];
    const best = compatible[0];
    if (best !== undefined && best.moves <= route.moves) return repository;
    promoted = true;
    return bound({ puzzles: { ...repository.puzzles, [id]: { rows, routes: [route, ...compatible] } } });
  });
  if (!applied) return 'unavailable';
  return promoted ? 'saved' : 'not-better';
}

/** A room's routes, re-verified on their rows; a stale or tampered history
 * reads as such, never as a route to play. */
export async function read(store: KeyValue, id: string, rows: string, verify: Verify): Promise<Read> {
  const stored = await store.get(KEY);
  if (stored === null) return { status: 'unavailable', routes: [] };
  const repository = normalize(stored);
  const history = repository.puzzles[id];
  if (history === undefined) return { status: 'missing', routes: [] };
  if (history.rows !== rows) return { status: 'stale', routes: [] };
  const routes = history.routes.flatMap(route => {
    const reverified = verified(rows, route, verify);
    return reverified !== null && reverified.routeId === route.routeId ? [reverified] : [];
  });
  if (routes.length === 0) return { status: 'corrupt', routes: [] };
  return { status: 'ready', routes };
}

/** The rooms with routes, newest best first: the shelf's index. */
export async function index(store: KeyValue = idb): Promise<{ available: boolean; ids: string[] }> {
  const stored = await store.get(KEY);
  if (stored === null) return { available: false, ids: [] };
  const repository = normalize(stored);
  const ids = Object.entries(repository.puzzles)
    .sort(([, first], [, second]) => (second.routes[0]?.completedAt ?? '').localeCompare(first.routes[0]?.completedAt ?? ''))
    .map(([id]) => id);
  return { available: true, ids };
}

/** Clears the repository: the reset's replay side. */
export async function clear(store: KeyValue = idb): Promise<boolean> {
  return store.update(KEY, () => ({ puzzles: {} }));
}
