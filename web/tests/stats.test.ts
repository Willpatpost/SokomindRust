import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../src/storage.ts';
import * as journal from '../src/journal.ts';
import { aggregates, formatDuration, heatLevel, statsText } from '../src/stats.ts';
import { ENTRIES } from '../src/catalog.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();

const NOW = new Date('2026-10-20T12:00:00.000Z');
const at = (back: number) => new Date(NOW.getTime() - back * 86_400_000).toISOString();
const tutorials = ENTRIES.filter(entry => entry.difficulty === 'tutorial');

test('an empty journal reads as zero everywhere', () => {
  const a = aggregates({ cleared: {}, daily: {}, activity: {} }, NOW);
  assert.equal(a.solved, 0);
  assert.equal(a.total, 57);
  assert.equal(a.completion, 0);
  assert.equal(a.recorded, 0);
  assert.equal(a.moves, 0);
  assert.equal(a.averagePushes, 0);
  assert.equal(a.efficiency, null);
  assert.deepEqual(a.streak, { current: 0, longest: 0 });
});

test('a clear counts by either signal, but only the dated records total', () => {
  const book = journal.record({ cleared: {}, daily: {}, activity: {} }, tutorials[0]!.id, 8, 3, 120_000, NOW);
  // A stored best from before the journal began counts as solved, not as a record.
  storage.write(`best.${tutorials[1]!.id}`, { rows: tutorials[1]!.rows.join('\n'), route: 'D', moves: 1, pushes: 1 });
  const a = aggregates(book, NOW);
  assert.equal(a.solved, 2);
  assert.equal(a.completion, (2 / 57) * 100);
  assert.equal(a.recorded, 1);
  assert.equal(a.moves, 8);
  assert.equal(a.pushes, 3);
  assert.equal(a.elapsedMs, 120_000);
  assert.equal(a.averagePushes, 3);
  assert.deepEqual(a.byDifficulty[0], { difficulty: 'tutorial', solved: 2, total: 4 });
  assert.equal(a.byDifficulty[1]!.solved, 0);
  assert.equal(a.streak.current, 1);
});

test('the efficiency names the clearest pushes-per-box record', () => {
  const first = tutorials[0]!;
  const second = tutorials[1]!;
  const book: journal.Journal = {
    cleared: {
      [first.id]: { moves: 8, pushes: first.boxes * 3, clearedAt: at(1) },
      [second.id]: { moves: 9, pushes: second.boxes * 2, clearedAt: at(1) },
    },
    daily: {},
    activity: {},
  };
  const a = aggregates(book, NOW);
  // Three pushes per box against two: the second room is the efficient one.
  assert.equal(a.efficiency?.title, second.title);
  assert.equal(a.efficiency?.pushes, second.boxes * 2);
  assert.equal(a.efficiency?.boxes, second.boxes);
  assert.equal(a.recorded, 2);
  assert.equal(a.averagePushes, (first.boxes * 3 + second.boxes * 2) / 2);
  assert.equal(a.elapsedMs, 0);
});

test('the heat levels step by the rooms a day cleared', () => {
  assert.equal(heatLevel(0), 0);
  assert.equal(heatLevel(1), 1);
  assert.equal(heatLevel(2), 1);
  assert.equal(heatLevel(3), 2);
  assert.equal(heatLevel(5), 2);
  assert.equal(heatLevel(6), 3);
  assert.equal(heatLevel(10), 3);
  assert.equal(heatLevel(11), 4);
});

test('durations read as minutes, then hours and minutes', () => {
  assert.equal(formatDuration(0), '0m');
  assert.equal(formatDuration(90_000), '1m');
  assert.equal(formatDuration(59 * 60_000), '59m');
  assert.equal(formatDuration(60 * 60_000), '1h 0m');
  assert.equal(formatDuration(90 * 60_000 + 30_000), '1h 30m');
});

test('the clipboard summary carries the aggregates, one line per fact', () => {
  const book: journal.Journal = {
    cleared: { [tutorials[0]!.id]: { moves: 8, pushes: 3, clearedAt: at(0) } },
    daily: {},
    activity: {},
  };
  const a = aggregates(book, NOW);
  const lines = statsText(a).split('\n');
  assert.equal(lines[0], 'Sokomind Rust Stats');
  assert.equal(lines[1], `${a.solved} of 57 rooms solved (${Math.round(a.completion)}%)`);
  assert.equal(lines[3], `${a.averagePushes.toFixed(1)} avg pushes per puzzle`);
  assert.equal(lines[4], `Streak: ${a.streak.current} days current, ${a.streak.longest} days best`);
  assert.equal(lines.length, 6 + 6);
  assert.ok(lines[6]!.startsWith('tutorial: '));
  assert.ok(lines[11]!.startsWith('master: '));
});
