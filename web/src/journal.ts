import * as storage from './storage.ts';
import { daily, ENTRIES } from './catalog.ts';

/** The completion journal: every room's best clear with its date, each day's
 * daily run, and the rooms cleared on each day — the record the streaks, the
 * heat map and the recently-cleared strip read. Days are UTC: the catalog's
 * daily pick rolls on the UTC day, so one time base, free of DST shifts, runs
 * the whole journal. */

/** One room's best clear: its counts, when it landed, and the time it took. */
export interface Clear {
  moves: number;
  pushes: number;
  clearedAt: string;
  elapsedMs?: number;
}

/** A day's daily clear: the room that day assigned, once cleared. */
export interface DailyClear {
  id: string;
  at: string;
}

/** The stored journal: the clears by room, the daily runs by day, and the
 * rooms cleared on each day. */
export interface Journal {
  cleared: Record<string, Clear>;
  daily: Record<string, DailyClear>;
  activity: Record<string, string[]>;
}

const DAY_MS = 86_400_000;
/** The activity a journal keeps: at most the newest 366 days and 10,000
 * entries, so storage stays bounded however long the journal runs. */
const MAX_ACTIVITY_DAYS = 366;
const MAX_ACTIVITY_ENTRIES = 10_000;
const DAY_KEY = /^\d{4}-\d{2}-\d{2}$/;

/** A UTC calendar day, the journal's unit of time. */
export function dayKey(date: Date): string {
  return date.toISOString().slice(0, 10);
}

/** The UTC day `key` names, as the ms stamp the daily pick reads. */
function dayMs(key: string): number {
  return Date.parse(`${key}T00:00:00.000Z`);
}

function isDayKey(key: string): boolean {
  if (!DAY_KEY.test(key)) return false;
  const ms = dayMs(key);
  return Number.isFinite(ms) && dayKey(new Date(ms)) === key;
}

function isRoomId(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 100;
}

function isClear(value: unknown): value is Clear {
  if (!value || typeof value !== 'object') return false;
  const record = value as Partial<Clear>;
  const at = typeof record.clearedAt === 'string' ? Date.parse(record.clearedAt) : Number.NaN;
  return (
    Number.isSafeInteger(record.moves)
    && Number(record.moves) >= 0
    && Number.isSafeInteger(record.pushes)
    && Number(record.pushes) >= 0
    && Number(record.pushes) <= Number(record.moves)
    && Number.isFinite(at)
    && new Date(at).toISOString() === record.clearedAt
    && (record.elapsedMs === undefined
      || (typeof record.elapsedMs === 'number' && Number.isFinite(record.elapsedMs) && record.elapsedMs >= 0))
  );
}

function isDailyClear(value: unknown): value is DailyClear {
  if (!value || typeof value !== 'object') return false;
  const record = value as Partial<DailyClear>;
  const at = typeof record.at === 'string' ? Date.parse(record.at) : Number.NaN;
  return isRoomId(record.id) && Number.isFinite(at) && new Date(at).toISOString() === record.at;
}

function normalizeActivity(value: unknown): Record<string, string[]> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const days = Object.entries(value as Record<string, unknown>)
    .filter(([key, rooms]) => isDayKey(key) && Array.isArray(rooms))
    .sort(([first], [second]) => second.localeCompare(first))
    .slice(0, MAX_ACTIVITY_DAYS);
  const activity: Record<string, string[]> = {};
  let kept = 0;
  for (const [key, rooms] of days) {
    const retained = [...new Set((rooms as unknown[]).filter(isRoomId))].sort().slice(0, MAX_ACTIVITY_ENTRIES - kept);
    if (retained.length === 0) continue;
    activity[key] = retained;
    kept += retained.length;
  }
  return activity;
}

/** The journal `value` names, or an empty one: a malformed field reads as
 * absent, never as a crash. */
function fromValue(value: unknown): Journal {
  if (!value || typeof value !== 'object') return { cleared: {}, daily: {}, activity: {} };
  const source = value as Partial<Journal>;
  const cleared: Record<string, Clear> = {};
  if (source.cleared && typeof source.cleared === 'object')
    for (const [id, record] of Object.entries(source.cleared)) if (isClear(record)) cleared[id] = record;
  const dailyRuns: Record<string, DailyClear> = {};
  if (source.daily && typeof source.daily === 'object')
    for (const [key, record] of Object.entries(source.daily)) if (isDayKey(key) && isDailyClear(record)) dailyRuns[key] = record;
  return { cleared, daily: dailyRuns, activity: normalizeActivity(source.activity) };
}

/** The journal in browser storage, or an empty one when nothing valid is
 * stored. */
export function load(): Journal {
  return fromValue(storage.read('journal'));
}

/** Adds `id` to its day's activity, then bounds the whole activity record. */
function withActivity(activity: Record<string, string[]>, id: string, at: string): Record<string, string[]> {
  const key = dayKey(new Date(at));
  if (activity[key]?.includes(id)) return activity;
  return normalizeActivity({ ...activity, [key]: [...(activity[key] ?? []), id] });
}

/** Records a clear: a strictly better route — fewer moves, then fewer pushes,
 * the same tie-break the best routes keep — replaces the room's record, and
 * the day's activity gains the room either way, so a replay counts as playing
 * without counting as progress. */
export function record(journal: Journal, id: string, moves: number, pushes: number, elapsedMs?: number, now: Date = new Date()): Journal {
  const at = now.toISOString();
  const activity = withActivity(journal.activity, id, at);
  const current = journal.cleared[id];
  if (current && (moves > current.moves || (moves === current.moves && pushes >= current.pushes)))
    return activity === journal.activity ? journal : { ...journal, activity };
  const clear: Clear = {
    moves,
    pushes,
    clearedAt: at,
    ...(elapsedMs !== undefined && elapsedMs > 0 ? { elapsedMs: Math.round(elapsedMs) } : {}),
  };
  return { cleared: { ...journal.cleared, [id]: clear }, daily: journal.daily, activity };
}

/** Records a daily clear under its UTC day. The caller confirms the room is
 * that day's pick — the catalog owns which room a day assigns. */
export function recordDaily(journal: Journal, id: string, now: Date = new Date()): Journal {
  const key = dayKey(now);
  if (journal.daily[key]?.id === id) return journal;
  return { ...journal, daily: { ...journal.daily, [key]: { id, at: now.toISOString() } } };
}

/** Records a clear in browser storage, returning the journal it wrote. */
export function keep(id: string, moves: number, pushes: number, elapsedMs?: number, now: Date = new Date()): Journal {
  const next = record(load(), id, moves, pushes, elapsedMs, now);
  storage.write('journal', next);
  return next;
}

/** Records a daily clear in browser storage, returning the journal it wrote. */
export function keepDaily(id: string, now: Date = new Date()): Journal {
  const next = recordDaily(load(), id, now);
  storage.write('journal', next);
  return next;
}

/** A progress file's read: the journal it holds, with the records it lost on
 * the way, or why it cannot be one. */
export type ImportResult = { ok: true; journal: Journal; invalid: number; rejected: number } | { ok: false; error: string };

/** How many bytes a progress file may carry, and how many records it may
 * hold, so a stray file cannot balloon the store. */
const MAX_IMPORT_BYTES = 1_000_000;
const MAX_IMPORT_RECORDS = MAX_ACTIVITY_ENTRIES + MAX_ACTIVITY_DAYS + 10_000;

/** Reads a progress file's text: its records keep only the catalog's rooms,
 * and what the file loses — malformed fields, unknown rooms — is counted, so
 * the import can say what it took in. */
export function imported(text: string): ImportResult {
  if (new TextEncoder().encode(text).byteLength > MAX_IMPORT_BYTES)
    return { ok: false, error: 'That file is too large to be a progress file.' };
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { ok: false, error: 'That file does not hold valid progress.' };
  }
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return { ok: false, error: 'That file does not hold valid progress.' };
  const source = raw as Partial<Journal>;
  if (!source.cleared || typeof source.cleared !== 'object' || Array.isArray(source.cleared))
    return { ok: false, error: 'That file does not hold valid progress.' };
  const rawCleared = Object.keys(source.cleared).length;
  const rawDaily = source.daily && typeof source.daily === 'object' ? Object.keys(source.daily).length : 0;
  const rawActivity =
    source.activity && typeof source.activity === 'object' && !Array.isArray(source.activity)
      ? Object.values(source.activity).reduce((count, rooms) => count + (Array.isArray(rooms) ? rooms.length : 1), 0)
      : 0;
  if (rawCleared + rawDaily + rawActivity > MAX_IMPORT_RECORDS) return { ok: false, error: 'That progress file holds too many records.' };
  const parsed = fromValue(raw);
  const validCleared = Object.keys(parsed.cleared).length;
  const validDaily = Object.keys(parsed.daily).length;
  const validActivity = Object.values(parsed.activity).reduce((count, rooms) => count + rooms.length, 0);
  if (validCleared + validDaily + validActivity === 0) return { ok: false, error: 'That file holds no valid progress records.' };
  const rooms = new Set(ENTRIES.map(entry => entry.id));
  const cleared: Record<string, Clear> = {};
  for (const [id, clear] of Object.entries(parsed.cleared)) if (rooms.has(id)) cleared[id] = clear;
  const dailyRuns: Record<string, DailyClear> = {};
  for (const [key, entry] of Object.entries(parsed.daily)) if (rooms.has(entry.id)) dailyRuns[key] = entry;
  const activity: Record<string, string[]> = {};
  for (const [key, ids] of Object.entries(parsed.activity)) {
    const kept = ids.filter(id => rooms.has(id));
    if (kept.length > 0) activity[key] = kept;
  }
  const keptCount =
    Object.keys(cleared).length + Object.keys(dailyRuns).length + Object.values(activity).reduce((count, ids) => count + ids.length, 0);
  if (keptCount === 0) return { ok: false, error: 'No matching rooms found in that file.' };
  return {
    ok: true,
    journal: { cleared, daily: dailyRuns, activity: normalizeActivity(activity) },
    invalid: Math.max(0, rawCleared + rawDaily + rawActivity - validCleared - validDaily - validActivity),
    rejected: validCleared + validDaily + validActivity - keptCount,
  };
}

/** Merges an imported journal into the current one: a better clear takes the
 * room's record, a day's standing daily clear stays, and the activity unions. */
export function merge(current: Journal, incoming: Journal): Journal {
  const cleared: Record<string, Clear> = { ...current.cleared };
  for (const [id, clear] of Object.entries(incoming.cleared)) {
    const old = cleared[id];
    if (!old || clear.moves < old.moves || (clear.moves === old.moves && clear.pushes < old.pushes)) cleared[id] = clear;
  }
  const dailyRuns: Record<string, DailyClear> = { ...current.daily };
  for (const [key, entry] of Object.entries(incoming.daily)) if (dailyRuns[key] === undefined) dailyRuns[key] = entry;
  const activity: Record<string, string[]> = { ...current.activity };
  for (const [key, rooms] of Object.entries(incoming.activity)) activity[key] = [...new Set([...(activity[key] ?? []), ...rooms])];
  return { cleared, daily: dailyRuns, activity: normalizeActivity(activity) };
}

/** The tally an import reports over the journal it merges into: what it
 * adds, improves, leaves as it stands and turns away. */
export interface MergeSummary {
  added: number;
  improved: number;
  unchanged: number;
  rejected: number;
}

export function summarizeMerge(current: Journal, incoming: Journal): MergeSummary {
  let added = 0;
  let improved = 0;
  let unchanged = 0;
  let rejected = 0;
  for (const [id, clear] of Object.entries(incoming.cleared)) {
    const old = current.cleared[id];
    if (!old) added++;
    else if (clear.moves < old.moves || (clear.moves === old.moves && clear.pushes < old.pushes)) improved++;
    else if (
      clear.moves === old.moves
      && clear.pushes === old.pushes
      && clear.clearedAt === old.clearedAt
      && clear.elapsedMs === old.elapsedMs
    )
      unchanged++;
    else rejected++;
  }
  for (const [key, entry] of Object.entries(incoming.daily)) {
    const old = current.daily[key];
    if (!old) added++;
    else if (old.id === entry.id && old.at === entry.at) unchanged++;
    else rejected++;
  }
  return { added, improved, unchanged, rejected };
}

/** Clears the journal in browser storage: the reset's progress side. */
export function reset(): void {
  storage.write('journal', { cleared: {}, daily: {}, activity: {} });
}

/** The daily run: consecutive UTC days, ending today, whose daily room was
 * cleared. Today still pending holds the run at zero until it is cleared —
 * the card says so — and a missed day ends it. */
export function dailyStreak(journal: Journal, now: Date = new Date()): number {
  const today = Math.floor(now.getTime() / DAY_MS);
  let streak = 0;
  for (let back = 0; back <= MAX_ACTIVITY_DAYS; back++) {
    const ms = (today - back) * DAY_MS;
    if (journal.daily[dayKey(new Date(ms))]?.id !== daily(ms).id) break;
    streak++;
  }
  return streak;
}

export type DailyOutcome = 'cleared' | 'missed' | 'today';

/** One day of the daily history strip. */
export interface DailyDay {
  key: string;
  label: string;
  outcome: DailyOutcome;
}

/** The last `days` UTC days of the daily run, oldest first: each day cleared,
 * missed, or still open as today. */
export function dailyHistory(journal: Journal, now: Date = new Date(), days = 7): DailyDay[] {
  const count = Math.min(14, Math.max(1, Math.floor(days)));
  const today = Math.floor(now.getTime() / DAY_MS);
  return Array.from({ length: count }, (_, index) => {
    const back = count - index - 1;
    const ms = (today - back) * DAY_MS;
    const key = dayKey(new Date(ms));
    const cleared = journal.daily[key]?.id === daily(ms).id;
    return {
      key,
      label: back === 0 ? 'Today' : new Intl.DateTimeFormat('en', { weekday: 'short', timeZone: 'UTC' }).format(new Date(ms)),
      outcome: cleared ? 'cleared' : back === 0 ? 'today' : 'missed',
    };
  });
}

/** The daily card's state: today's room cleared, a run to recover after a
 * missed day, or a fresh run to start. */
export interface DailyView {
  state: 'ready' | 'cleared' | 'recovery';
  streak: number;
  history: DailyDay[];
}

export function dailyView(journal: Journal, now: Date = new Date()): DailyView {
  const history = dailyHistory(journal, now);
  const clearedToday = history[history.length - 1]!.outcome === 'cleared';
  const missedYesterday = history.length > 1 && history[history.length - 2]!.outcome === 'missed';
  const ranBefore = history.slice(0, -1).some(day => day.outcome === 'cleared');
  return {
    state: clearedToday ? 'cleared' : missedYesterday && ranBefore ? 'recovery' : 'ready',
    streak: dailyStreak(journal, now),
    history,
  };
}

/** The solving streak over the activity days: the current run, anchored on
 * today or yesterday, and the longest run the journal holds. */
export interface Streak {
  current: number;
  longest: number;
}

export function solvingStreak(journal: Journal, now: Date = new Date()): Streak {
  const days = Object.keys(journal.activity)
    .filter(key => (journal.activity[key]?.length ?? 0) > 0)
    .sort();
  if (days.length === 0) return { current: 0, longest: 0 };
  let longest = 1;
  let run = 1;
  for (let index = 1; index < days.length; index++) {
    run = dayMs(days[index]!) - dayMs(days[index - 1]!) === DAY_MS ? run + 1 : 1;
    if (run > longest) longest = run;
  }
  const today = Math.floor(now.getTime() / DAY_MS) * DAY_MS;
  const last = dayMs(days[days.length - 1]!);
  if (last !== today && last !== today - DAY_MS) return { current: 0, longest };
  let current = 1;
  for (let day = last - DAY_MS; (journal.activity[dayKey(new Date(day))]?.length ?? 0) > 0; day -= DAY_MS) current++;
  return { current, longest };
}

/** One cell of the activity heat map. */
export interface HeatDay {
  key: string;
  count: number;
}

/** The activity heat: the last `days` UTC days, oldest first, with the count
 * of rooms cleared on each. */
export function heat(journal: Journal, now: Date = new Date(), days = 90): HeatDay[] {
  const count = Math.min(MAX_ACTIVITY_DAYS, Math.max(0, Math.floor(days)));
  const today = Math.floor(now.getTime() / DAY_MS);
  return Array.from({ length: count }, (_, index) => {
    const key = dayKey(new Date((today - count + index + 1) * DAY_MS));
    return { key, count: journal.activity[key]?.length ?? 0 };
  });
}

/** One row of the recently-cleared strip. */
export interface RecentClear {
  id: string;
  moves: number;
  pushes: number;
  clearedAt: string;
}

/** The newest `limit` clears, newest first. */
export function recentlyCleared(journal: Journal, limit = 3): RecentClear[] {
  return Object.entries(journal.cleared)
    .sort(([, first], [, second]) => second.clearedAt.localeCompare(first.clearedAt))
    .slice(0, Math.max(0, Math.floor(limit)))
    .map(([id, clear]) => ({ id, moves: clear.moves, pushes: clear.pushes, clearedAt: clear.clearedAt }));
}

/** How long ago an instant reads: minutes, hours, or days. */
export function timeAgo(at: string, now: Date = new Date()): string {
  const ms = now.getTime() - Date.parse(at);
  if (ms < 60_000) return 'just now';
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}
