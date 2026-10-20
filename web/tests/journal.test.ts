import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../src/storage.ts';
import * as journal from '../src/journal.ts';
import { daily } from '../src/catalog.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();

const DAY = 86_400_000;
/** A fixed present: noon UTC, so every day arithmetic stays inside its day. */
const NOW = new Date('2026-10-20T12:00:00.000Z');
const at = (back: number) => new Date(NOW.getTime() - back * DAY);
/** The ISO stamp `back` days ago, as the journal stores its dates. */
const stamp = (back: number) => at(back).toISOString();
const keyAt = (back: number) => journal.dayKey(at(back));
/** The room the daily assigns to `back`'s day, as the journal's streaks read it. */
const pick = (back: number) => daily(at(back).getTime()).id;
const runs = (backs: number[]): journal.Journal => {
  const dailyRuns: Record<string, journal.DailyClear> = {};
  for (const back of backs) dailyRuns[keyAt(back)] = { id: pick(back), at: stamp(back) };
  return { cleared: {}, daily: dailyRuns, activity: {} };
};
const active = (...backs: number[]): journal.Journal => {
  const activity: Record<string, string[]> = {};
  for (const back of backs) activity[keyAt(back)] = ['tiny'];
  return { cleared: {}, daily: {}, activity };
};

test('a first clear records its counts, its date and its day', () => {
  const book = journal.record({ cleared: {}, daily: {}, activity: {} }, 'tiny', 8, 3, 120_456.7, NOW);
  assert.equal(book.cleared.tiny?.moves, 8);
  assert.equal(book.cleared.tiny?.pushes, 3);
  assert.equal(book.cleared.tiny?.clearedAt, NOW.toISOString());
  assert.equal(book.cleared.tiny?.elapsedMs, 120_457);
  assert.deepEqual(book.activity[keyAt(0)], ['tiny']);
});

test('a better clear replaces the record; a worse one only marks the day', () => {
  let book = journal.record({ cleared: {}, daily: {}, activity: {} }, 'tiny', 8, 3, undefined, at(2));
  // Fewer moves win even at more pushes; fewer pushes win at equal moves.
  book = journal.record(book, 'tiny', 6, 4, undefined, at(1));
  assert.equal(book.cleared.tiny?.moves, 6);
  assert.equal(book.cleared.tiny?.pushes, 4);
  assert.equal(book.cleared.tiny?.clearedAt, at(1).toISOString());
  book = journal.record(book, 'tiny', 6, 2, undefined, at(1));
  assert.equal(book.cleared.tiny?.pushes, 2);
  // A worse clear, and a tie, keep the record and its first date.
  book = journal.record(book, 'tiny', 7, 1, undefined, at(0));
  book = journal.record(book, 'tiny', 6, 2, undefined, at(0));
  assert.equal(book.cleared.tiny?.moves, 6);
  assert.equal(book.cleared.tiny?.pushes, 2);
  assert.equal(book.cleared.tiny?.clearedAt, at(1).toISOString());
  // Every clear still marks its own day, once per day.
  assert.deepEqual(book.activity[keyAt(2)], ['tiny']);
  assert.deepEqual(book.activity[keyAt(1)], ['tiny']);
  assert.deepEqual(book.activity[keyAt(0)], ['tiny']);
});

test('the activity dedupes rooms per day and bounds its history', () => {
  let book = journal.record({ cleared: {}, daily: {}, activity: {} }, 'tiny', 8, 3, undefined, NOW);
  book = journal.record(book, 'huge', 400, 200, undefined, NOW);
  assert.deepEqual(book.activity[keyAt(0)], ['huge', 'tiny']);
  // The store keeps only the newest 366 days and 10,000 entries.
  const days: Record<string, string[]> = {};
  for (let back = 1; back <= 400; back++) days[keyAt(back)] = ['a'];
  storage.write('journal', { cleared: {}, daily: {}, activity: days });
  const bounded = journal.load();
  assert.equal(Object.keys(bounded.activity).length, 366);
  assert.ok(bounded.activity[keyAt(366)]);
  assert.equal(bounded.activity[keyAt(367)], undefined);
  const rooms = Array.from({ length: 10_050 }, (_, index) => `room-${index}`);
  storage.write('journal', { cleared: {}, daily: {}, activity: { [keyAt(0)]: rooms } });
  assert.equal(journal.load().activity[keyAt(0)]!.length, 10_000);
});

test('malformed journals read as empty, or as their valid parts', () => {
  storage.write('journal', 'garbage');
  assert.deepEqual(journal.load(), { cleared: {}, daily: {}, activity: {} });
  storage.write('journal', {
    cleared: {
      good: { moves: 3, pushes: 1, clearedAt: NOW.toISOString() },
      negative: { moves: -1, pushes: 0, clearedAt: NOW.toISOString() },
      morePushes: { moves: 2, pushes: 3, clearedAt: NOW.toISOString() },
      prose: { moves: 2, pushes: 1, clearedAt: 'yesterday' },
      loose: { moves: 2, pushes: 1, clearedAt: '2026-10-20' },
    },
    daily: { [keyAt(0)]: { id: 'good', at: NOW.toISOString() }, 'not-a-day': { id: 'x', at: NOW.toISOString() } },
    activity: { [keyAt(0)]: ['good', 7, ''], '2026-13-40': ['x'] },
  });
  const book = journal.load();
  assert.deepEqual(Object.keys(book.cleared), ['good']);
  assert.equal(book.daily['not-a-day'], undefined);
  assert.equal(book.daily[keyAt(0)]?.id, 'good');
  assert.deepEqual(book.activity[keyAt(0)], ['good']);
  assert.equal(book.activity['2026-13-40'], undefined);
});

test('daily clears record once under their day', () => {
  const empty = { cleared: {}, daily: {}, activity: {} } as const;
  let book = journal.recordDaily({ ...empty, daily: { ...empty.daily } }, pick(0), NOW);
  const again = journal.recordDaily(book, pick(0), NOW);
  assert.ok(again === book, 'the same day’s room records once');
  book = journal.recordDaily(book, 'other', NOW);
  assert.equal(book.daily[keyAt(0)]?.id, 'other');
});

test('keep and keepDaily write through browser storage', () => {
  storage.write('journal', { cleared: {}, daily: {}, activity: {} });
  journal.keep('tiny', 8, 3, 120_000, NOW);
  journal.keepDaily(pick(0), NOW);
  const book = journal.load();
  assert.equal(book.cleared.tiny?.moves, 8);
  assert.equal(book.cleared.tiny?.elapsedMs, 120_000);
  assert.equal(book.daily[keyAt(0)]?.id, pick(0));
});

test('the daily run counts consecutive cleared picks, ending today', () => {
  assert.equal(journal.dailyStreak(runs([0, 1, 2]), NOW), 3);
  // A missed yesterday ends the run at today.
  assert.equal(journal.dailyStreak(runs([0, 2]), NOW), 1);
  // Today still pending holds the run at zero until it is cleared.
  assert.equal(journal.dailyStreak(runs([1, 2]), NOW), 0);
  assert.equal(journal.dailyStreak({ cleared: {}, daily: {}, activity: {} }, NOW), 0);
});

test('the daily history marks each day cleared, missed, or today', () => {
  const history = journal.dailyHistory(runs([0, 3]), NOW);
  assert.equal(history.length, 7);
  assert.equal(history[6]!.label, 'Today');
  assert.equal(history[6]!.outcome, 'cleared');
  assert.equal(history[3]!.outcome, 'cleared');
  for (const [index, day] of history.entries()) if (index !== 3 && index !== 6) assert.equal(day.outcome, 'missed');
  for (const day of history.slice(0, 6)) assert.match(day.label, /^[A-Z][a-z]{2}$/);
});

test('the daily card reads its state off the history', () => {
  assert.equal(journal.dailyView(runs([0]), NOW).state, 'cleared');
  assert.equal(journal.dailyView(runs([2]), NOW).state, 'recovery');
  assert.equal(journal.dailyView({ cleared: {}, daily: {}, activity: {} }, NOW).state, 'ready');
  assert.equal(journal.dailyView(runs([0, 1]), NOW).streak, 2);
});

test('the solving streak anchors on today or yesterday', () => {
  assert.deepEqual(journal.solvingStreak(active(0, 1), NOW), { current: 2, longest: 2 });
  assert.deepEqual(journal.solvingStreak(active(1), NOW), { current: 1, longest: 1 });
  assert.deepEqual(journal.solvingStreak(active(2), NOW), { current: 0, longest: 1 });
  assert.deepEqual(journal.solvingStreak(active(0, 1, 3, 4), NOW), { current: 2, longest: 2 });
  assert.deepEqual(journal.solvingStreak(active(3, 4, 5), NOW), { current: 0, longest: 3 });
});

test('the activity heat spans its days, ending today', () => {
  const cells = journal.heat(active(0, 2), NOW, 5);
  assert.equal(cells.length, 5);
  assert.deepEqual(cells[4], { key: keyAt(0), count: 1 });
  assert.deepEqual(cells[3], { key: keyAt(1), count: 0 });
  assert.deepEqual(cells[2], { key: keyAt(2), count: 1 });
});

test('recently cleared lists the newest first, capped', () => {
  const book: journal.Journal = {
    cleared: {
      a: { moves: 5, pushes: 2, clearedAt: '2026-10-18T10:00:00.000Z' },
      b: { moves: 6, pushes: 3, clearedAt: '2026-10-20T09:00:00.000Z' },
      c: { moves: 7, pushes: 4, clearedAt: '2026-10-19T11:00:00.000Z' },
      d: { moves: 8, pushes: 5, clearedAt: '2026-10-17T08:00:00.000Z' },
    },
    daily: {},
    activity: {},
  };
  assert.deepEqual(
    journal.recentlyCleared(book).map(clear => clear.id),
    ['b', 'c', 'a'],
  );
  assert.deepEqual(
    journal.recentlyCleared(book, 2).map(clear => clear.id),
    ['b', 'c'],
  );
});

test('timeAgo reads in minutes, hours and days', () => {
  const now = new Date('2026-10-20T12:00:00.000Z');
  assert.equal(journal.timeAgo('2026-10-20T11:59:30.000Z', now), 'just now');
  assert.equal(journal.timeAgo('2026-10-20T11:55:00.000Z', now), '5m ago');
  assert.equal(journal.timeAgo('2026-10-20T09:00:00.000Z', now), '3h ago');
  assert.equal(journal.timeAgo('2026-10-18T12:00:00.000Z', now), '2d ago');
});

test('an import keeps the catalog’s rooms and names what it loses', () => {
  const room = pick(0);
  const text = JSON.stringify({
    cleared: {
      [room]: { moves: 5, pushes: 2, clearedAt: stamp(0) },
      'ghost-room': { moves: 5, pushes: 2, clearedAt: stamp(0) },
      broken: { moves: -1, pushes: 0, clearedAt: stamp(0) },
    },
    daily: { [keyAt(0)]: { id: room, at: stamp(0) }, [keyAt(1)]: { id: 'ghost-room', at: stamp(1) } },
    activity: { [keyAt(0)]: [room, 'ghost-room', 'junk'] },
  });
  const result = journal.imported(text);
  if (!result.ok) assert.fail('the import should have parsed');
  assert.deepEqual(Object.keys(result.journal.cleared), [room]);
  assert.equal(result.journal.cleared[room]?.moves, 5);
  assert.deepEqual(result.journal.daily, { [keyAt(0)]: { id: room, at: stamp(0) } });
  assert.deepEqual(result.journal.activity, { [keyAt(0)]: [room] });
  assert.equal(result.invalid, 1);
  assert.equal(result.rejected, 4);
});

test('an import refuses what it is not', () => {
  for (const [text, error] of [
    ['garbage', 'That file does not hold valid progress.'],
    ['[]', 'That file does not hold valid progress.'],
    ['{}', 'That file does not hold valid progress.'],
    [
      JSON.stringify({ cleared: { broken: { moves: -1, pushes: 0, clearedAt: stamp(0) } }, daily: {}, activity: {} }),
      'That file holds no valid progress records.',
    ],
    [
      JSON.stringify({ cleared: { 'ghost-room': { moves: 5, pushes: 2, clearedAt: stamp(0) } }, daily: {}, activity: {} }),
      'No matching rooms found in that file.',
    ],
  ] as const) {
    const result = journal.imported(text);
    if (result.ok) assert.fail(`the import should have refused: ${text}`);
    assert.equal(result.error, error);
  }
});

test('an import’s bytes and records stay bounded', () => {
  const tooBig = journal.imported('x'.repeat(1_000_001));
  if (tooBig.ok) assert.fail('the oversized file should have been refused');
  assert.equal(tooBig.error, 'That file is too large to be a progress file.');
  const ids = Array.from({ length: 20_400 }, (_, index) => `r${index}`);
  const tooMany = journal.imported(JSON.stringify({ cleared: {}, daily: {}, activity: { [keyAt(0)]: ids } }));
  if (tooMany.ok) assert.fail('the over-recorded file should have been refused');
  assert.equal(tooMany.error, 'That progress file holds too many records.');
});

test('a merge takes better clears, keeps standing dailies, and unions activity', () => {
  const current: journal.Journal = {
    cleared: { tiny: { moves: 8, pushes: 3, clearedAt: stamp(2) }, huge: { moves: 400, pushes: 200, clearedAt: stamp(2) } },
    daily: { [keyAt(0)]: { id: 'tiny', at: stamp(0) }, [keyAt(1)]: { id: 'huge', at: stamp(1) } },
    activity: { [keyAt(0)]: ['tiny'], [keyAt(1)]: ['huge'] },
  };
  const incoming: journal.Journal = {
    cleared: {
      tiny: { moves: 6, pushes: 4, clearedAt: stamp(0) },
      huge: { moves: 401, pushes: 150, clearedAt: stamp(0) },
      grand: { moves: 50, pushes: 25, clearedAt: stamp(0) },
    },
    daily: { [keyAt(0)]: { id: 'other', at: stamp(0) }, [keyAt(2)]: { id: 'grand', at: stamp(2) } },
    activity: { [keyAt(0)]: ['tiny', 'grand'], [keyAt(1)]: ['huge', 'tiny'] },
  };
  const merged = journal.merge(current, incoming);
  assert.equal(merged.cleared.tiny?.moves, 6);
  assert.equal(merged.cleared.tiny?.pushes, 4);
  assert.equal(merged.cleared.huge?.moves, 400);
  assert.deepEqual(Object.keys(merged.cleared).sort(), ['grand', 'huge', 'tiny']);
  assert.equal(merged.daily[keyAt(0)]?.id, 'tiny');
  assert.deepEqual(merged.daily[keyAt(2)], { id: 'grand', at: stamp(2) });
  assert.deepEqual(merged.activity[keyAt(0)], ['grand', 'tiny']);
  assert.deepEqual(merged.activity[keyAt(1)], ['huge', 'tiny']);
});

test('the merge summary counts added, improved, unchanged and rejected', () => {
  const current: journal.Journal = {
    cleared: {
      a: { moves: 8, pushes: 3, clearedAt: stamp(3) },
      b: { moves: 10, pushes: 5, clearedAt: stamp(3) },
      e: { moves: 10, pushes: 5, clearedAt: stamp(3) },
      f: { moves: 7, pushes: 2, clearedAt: stamp(3) },
    },
    daily: { [keyAt(0)]: { id: 'a', at: stamp(0) }, [keyAt(3)]: { id: 'y', at: stamp(3) } },
    activity: {},
  };
  const incoming: journal.Journal = {
    cleared: {
      a: { moves: 6, pushes: 4, clearedAt: stamp(2) },
      b: { moves: 10, pushes: 5, clearedAt: stamp(3) },
      c: { moves: 5, pushes: 2, clearedAt: stamp(2) },
      e: { moves: 12, pushes: 4, clearedAt: stamp(2) },
      f: { moves: 7, pushes: 2, clearedAt: stamp(2) },
    },
    daily: { [keyAt(0)]: { id: 'a', at: stamp(0) }, [keyAt(1)]: { id: 'c', at: stamp(1) }, [keyAt(3)]: { id: 'x', at: stamp(3) } },
    activity: {},
  };
  assert.deepEqual(journal.summarizeMerge(current, incoming), { added: 2, improved: 1, unchanged: 2, rejected: 3 });
});

test('reset clears the journal in browser storage', () => {
  journal.keep('tiny', 8, 3, undefined, NOW);
  assert.ok(Object.keys(journal.load().cleared).length > 0);
  journal.reset();
  assert.deepEqual(journal.load(), { cleared: {}, daily: {}, activity: {} });
});
