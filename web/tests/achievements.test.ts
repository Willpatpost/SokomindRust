import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../src/storage.ts';
import * as journal from '../src/journal.ts';
import * as stats from '../src/stats.ts';
import * as achievements from '../src/achievements.ts';
import { ENTRIES } from '../src/catalog.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();

const DAY = 86_400_000;
const NOW = new Date('2026-10-20T12:00:00.000Z');
const at = (back: number) => new Date(NOW.getTime() - back * DAY);
const stamp = (back: number) => at(back).toISOString();
const keyAt = (back: number) => journal.dayKey(at(back));
const tutorials = ENTRIES.filter(entry => entry.difficulty === 'tutorial');
const clear = (moves: number, pushes: number, back = 0): journal.Clear => ({ moves, pushes, clearedAt: stamp(back) });
const bestFor = (entry: (typeof ENTRIES)[number]) =>
  storage.write(`best.${entry.id}`, { rows: entry.rows.join('\n'), route: 'D', moves: 1, pushes: 1 });

test('the seventeen achievements sit in four collections', () => {
  assert.equal(achievements.ACHIEVEMENTS.length, 17);
  assert.equal(new Set(achievements.ACHIEVEMENTS.map(def => def.id)).size, 17);
  assert.deepEqual(
    achievements.COLLECTIONS.map(collection => collection.id),
    ['exploration', 'momentum', 'curriculum', 'craft'],
  );
});

test('count and tier achievements read either solved signal', () => {
  const book: journal.Journal = { cleared: {}, daily: {}, activity: {} };
  assert.equal(achievements.unlocked(stats.aggregates(book, NOW), book).length, 0);
  // A stored best counts toward the first solve without a dated record.
  bestFor(tutorials[0]!);
  const one = stats.aggregates(book, NOW);
  assert.ok(achievements.unlockedIds(one, book).has('first-solve'));
  assert.ok(!achievements.unlockedIds(one, book).has('tutorial-complete'));
  // Four tutorial bests complete the tier, dates or none.
  for (const entry of tutorials) bestFor(entry);
  const four = stats.aggregates(book, NOW);
  assert.ok(achievements.unlockedIds(four, book).has('tutorial-complete'));
});

test('streak achievements read the journal’s consecutive days', () => {
  const book: journal.Journal = { cleared: {}, daily: {}, activity: { [keyAt(0)]: ['x'], [keyAt(1)]: ['x'], [keyAt(2)]: ['x'] } };
  const a = stats.aggregates(book, NOW);
  assert.deepEqual(a.streak, { current: 3, longest: 3 });
  assert.ok(achievements.unlockedIds(a, book).has('streak-3'));
  assert.ok(!achievements.unlockedIds(a, book).has('streak-7'));
});

test('craft achievements read the dated records', () => {
  const light: Record<string, journal.Clear> = {};
  for (const room of ENTRIES.slice(0, 10)) light[room.id] = clear(120, 4);
  const book: journal.Journal = { cleared: light, daily: {}, activity: {} };
  const a = stats.aggregates(book, NOW);
  // Ten recorded rooms, 1,200 moves at 4 pushes each.
  assert.equal(a.recorded, 10);
  assert.equal(a.moves, 1200);
  assert.equal(a.averagePushes, 4);
  assert.ok(achievements.unlockedIds(a, book).has('thousand-moves'));
  assert.ok(!achievements.unlockedIds(a, book).has('ten-thousand-moves'));
  assert.ok(achievements.unlockedIds(a, book).has('efficient-mover'));
  // An average at five pushes keeps Efficient Mover locked.
  const heavy: Record<string, journal.Clear> = {};
  for (const room of ENTRIES.slice(0, 10)) heavy[room.id] = clear(120, 5);
  const heavyBook: journal.Journal = { cleared: heavy, daily: {}, activity: {} };
  assert.ok(!achievements.unlockedIds(stats.aggregates(heavyBook, NOW), heavyBook).has('efficient-mover'));
});

test('newlyUnlocked names the achievements the moment earned', () => {
  assert.deepEqual(
    achievements.newlyUnlocked(new Set(['first-solve']), new Set(['first-solve', 'ten-solved'])).map(def => def.id),
    ['ten-solved'],
  );
  assert.deepEqual(achievements.newlyUnlocked(new Set(['first-solve']), new Set(['first-solve'])), []);
});

test('milestones carry the dates the records show', () => {
  const book: journal.Journal = {
    cleared: {
      [tutorials[0]!.id]: clear(5, 2, 3),
      [tutorials[1]!.id]: clear(6, 3, 2),
      [tutorials[2]!.id]: clear(7, 4, 1),
      [tutorials[3]!.id]: clear(8, 5, 0),
    },
    daily: {},
    activity: { [keyAt(0)]: ['x'], [keyAt(1)]: ['x'], [keyAt(2)]: ['x'] },
  };
  const marks = achievements.milestones(stats.aggregates(book, NOW), book);
  const byId = new Map(marks.map(mark => [mark.id, mark]));
  // The first clear opens the count milestones; the fourth closes the tier.
  assert.equal(byId.get('first-solve')?.earnedAt, stamp(3));
  assert.equal(byId.get('tutorial-complete')?.earnedAt, stamp(0));
  // The three-day run earns its streak on the day it reached three.
  assert.equal(byId.get('streak-3')?.earnedAt, `${keyAt(0)}T12:00:00.000Z`);
  assert.equal(byId.get('first-solve')?.collectionTitle, 'Room by room');
  assert.equal(achievements.milestoneDay(`${keyAt(0)}T12:00:00.000Z`), keyAt(0));
  assert.match(achievements.milestoneLabel(stamp(0)), /^[A-Z][a-z]{2} \d{1,2}, \d{4}$/);
});
