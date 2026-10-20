import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../src/storage.ts';
import * as optimal from '../src/optimal.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();

test('a record is the certified optimum for exactly its rows', () => {
  optimal.record('tiny', 'OOO\nORO', 8);
  assert.deepEqual(optimal.proven('tiny', 'OOO\nORO'), { rows: 'OOO\nORO', moves: 8 });
  assert.equal(optimal.proven('tiny', 'other rows'), undefined);
  assert.equal(optimal.proven('huge', 'OOO\nORO'), undefined);
  assert.equal(optimal.isOptimal('tiny', 'OOO\nORO', 8), true);
  // No route can beat a certified optimum; matching it is the optimum.
  assert.equal(optimal.isOptimal('tiny', 'OOO\nORO', 7), true);
  assert.equal(optimal.isOptimal('tiny', 'OOO\nORO', 9), false);
});

test('a proof for changed rows replaces; a sound one keeps the lower count', () => {
  optimal.record('tiny', 'first rows', 8);
  optimal.record('tiny', 'second rows', 12);
  assert.deepEqual(optimal.proven('tiny', 'second rows'), { rows: 'second rows', moves: 12 });
  optimal.record('tiny', 'second rows', 10);
  assert.equal(optimal.proven('tiny', 'second rows')?.moves, 10);
  optimal.record('tiny', 'second rows', 11);
  assert.equal(optimal.proven('tiny', 'second rows')?.moves, 10);
});

test('malformed caches read as empty', () => {
  storage.write('optimal', 'garbage');
  assert.equal(optimal.proven('tiny', 'rows'), undefined);
  storage.write('optimal', { tiny: { rows: 'rows', moves: 'many' }, broken: { rows: 7, moves: 3 } });
  assert.deepEqual(optimal.load(), {});
  optimal.record('tiny', 'rows', 5);
  assert.deepEqual(optimal.load(), { tiny: { rows: 'rows', moves: 5 } });
});
