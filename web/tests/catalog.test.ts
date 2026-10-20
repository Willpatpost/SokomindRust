import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../src/storage.ts';
import { DIFFICULTIES, ENTRIES, daily, favorites, filter, paginate, rate, rating, solved, toggleFavorite } from '../src/catalog.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();

test('the catalog carries the data its rows name', () => {
  assert.equal(ENTRIES.length, 57);
  assert.deepEqual(DIFFICULTIES, ['tutorial', 'beginner', 'intermediate', 'advanced', 'expert', 'master']);
  const first = ENTRIES[0]!;
  assert.equal(first.id, 'ultra-tiny');
  assert.equal(first.width, 5);
  assert.equal(first.height, 5);
  assert.equal(first.collection, 'Sokomind Originals');
  // A ragged board's width is its longest row.
  for (const entry of ENTRIES) assert.equal(Math.max(...entry.rows.map(row => row.length)), entry.width, entry.id);
});

test('the solved signal is a stored best for exactly these rows', () => {
  const entry = ENTRIES[1]!;
  assert.equal(solved(entry), false);
  storage.write(`best.${entry.id}`, { rows: entry.rows.join('\n'), route: 'D', moves: 1, pushes: 1 });
  assert.equal(solved(entry), true);
  // A best stored for other rows does not count.
  storage.write(`best.${ENTRIES[2]!.id}`, { rows: 'other rows', route: 'D', moves: 1, pushes: 1 });
  assert.equal(solved(ENTRIES[2]!), false);
});

test('the filters keep what they name', () => {
  const expert = filter(ENTRIES, { difficulty: 'expert' });
  assert.equal(expert.length, 8);
  assert.ok(expert.every(entry => entry.difficulty === 'expert'));
  const originals = filter(ENTRIES, { collection: 'Sokomind Originals' });
  assert.equal(originals.length, 19);
  const search = filter(ENTRIES, { search: ' first steps ' });
  assert.deepEqual(
    search.map(entry => entry.id),
    ['ultra-tiny'],
  );
  const boxed = filter(ENTRIES, { boxes: 22 });
  assert.ok(boxed.length >= 1 && boxed.every(entry => entry.boxes === 22));
  // The status filters read the stored bests and the favorites.
  storage.write(`best.${ENTRIES[0]!.id}`, { rows: ENTRIES[0]!.rows.join('\n'), route: 'D', moves: 1, pushes: 1 });
  const solvedRows = filter(ENTRIES, { status: 'solved' });
  assert.deepEqual(
    solvedRows.map(entry => entry.id),
    [ENTRIES[0]!.id, ENTRIES[1]!.id],
  );
  assert.equal(filter(ENTRIES, { status: 'open' }).length, ENTRIES.length - solvedRows.length);
  toggleFavorite('huge');
  assert.deepEqual([...favorites()], ['huge']);
  assert.deepEqual(
    filter(ENTRIES, { status: 'favorites' }).map(entry => entry.id),
    ['huge'],
  );
  // Search matches the title in any case, or the id.
  assert.deepEqual(
    filter(ENTRIES, { search: 'GRAND' }).map(entry => entry.id),
    ['huge'],
  );
});

test('favorites toggle both ways', () => {
  // An earlier test favorited 'huge'; this one starts from a clean set.
  storage.write('favorites', []);
  assert.equal(toggleFavorite('ultra-tiny'), true);
  assert.equal(toggleFavorite('ultra-tiny'), false);
  assert.equal(favorites().size, 0);
});

test('pagination clamps past-the-end pages to the last', () => {
  const whole = paginate(ENTRIES, 1);
  assert.equal(whole.pages, 2);
  assert.equal(whole.slice.length, 50);
  const tail = paginate(ENTRIES, 2);
  assert.equal(tail.page, 2);
  assert.equal(tail.slice.length, ENTRIES.length - 50);
  const past = paginate(ENTRIES, 99);
  assert.equal(past.page, 2);
  assert.deepEqual(past.slice, tail.slice);
  const empty = paginate([], 3);
  assert.equal(empty.page, 1);
  assert.equal(empty.pages, 1);
  assert.deepEqual(empty.slice, []);
});

test('the daily challenge is the UTC day’s pick, stable through the day', () => {
  const midnight = Math.floor(Date.now() / 86_400_000) * 86_400_000;
  assert.equal(daily(midnight).id, daily(midnight + 86_400_000 - 1).id);
  assert.ok(daily(midnight + 86_400_000 - 1).id !== daily(midnight + 86_400_000).id, 'the next UTC day picks a different puzzle');
});

test('ratings store and replace per puzzle, reading malformed stores as none', () => {
  assert.equal(rating('ultra-tiny'), undefined);
  rate('ultra-tiny', 'right');
  assert.equal(rating('ultra-tiny'), 'right');
  rate('ultra-tiny', 'hard');
  assert.equal(rating('ultra-tiny'), 'hard');
  assert.equal(rating('tiny'), undefined);
  storage.write('ratings', 'garbage');
  assert.equal(rating('ultra-tiny'), undefined);
  storage.write('ratings', ['x']);
  assert.equal(rating('ultra-tiny'), undefined);
  rate('tiny', 'easy');
  assert.equal(rating('tiny'), 'easy');
});
