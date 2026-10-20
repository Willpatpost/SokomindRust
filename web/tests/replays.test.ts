import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as replays from '../src/replays.ts';
import { memoryKeyValue } from './fakes.ts';

const ROWS = 'OOOOO\nO R O\nO A O\nO a O\nOOOOO';
const stamp = (day: number) => `2026-10-${String(day).padStart(2, '0')}T12:00:00.000Z`;
/** The engine's stand-in: a route replays to its length in moves, with a
 * push for every D. */
const verify: replays.Verify = (rows, actionLog) =>
  rows === ROWS && /^[UDLR]+$/.test(actionLog) ? { moves: actionLog.length, pushes: (actionLog.match(/D/g) ?? []).length } : null;
const route = (moves: number, day: number) => ({
  actionLog: 'U'.repeat(moves - 1) + 'D',
  moves,
  pushes: 1,
  completedAt: stamp(day),
});

test('a verified route enters only as an improvement', async () => {
  const store = memoryKeyValue();
  assert.equal(await replays.promote(store, 'tiny', ROWS, route(2, 18), verify), 'saved');
  // A route no shorter than the best never enters, even at fewer pushes.
  assert.equal(await replays.promote(store, 'tiny', ROWS, { ...route(2, 19), actionLog: 'UU', pushes: 0 }, verify), 'not-better');
  assert.equal(await replays.promote(store, 'tiny', ROWS, route(3, 19), verify), 'not-better');
  // A shorter route improves the history.
  assert.equal(await replays.promote(store, 'tiny', ROWS, route(1, 19), verify), 'saved');
  const read = await replays.read(store, 'tiny', ROWS, verify);
  assert.equal(read.status, 'ready');
  assert.deepEqual(
    read.routes.map(saved => saved.moves),
    [1, 2],
  );
});

test('a route the engine cannot confirm is rejected', async () => {
  const store = memoryKeyValue();
  // The replay disagrees with the claimed counts.
  assert.equal(
    await replays.promote(store, 'tiny', ROWS, { actionLog: 'D', moves: 2, pushes: 1, completedAt: stamp(18) }, verify),
    'rejected',
  );
  // The log is not a route at all.
  assert.equal(
    await replays.promote(store, 'tiny', ROWS, { actionLog: 'DX', moves: 2, pushes: 1, completedAt: stamp(18) }, verify),
    'rejected',
  );
  // A malformed save time is not a route either.
  assert.equal(
    await replays.promote(store, 'tiny', ROWS, { actionLog: 'D', moves: 1, pushes: 1, completedAt: 'yesterday' }, verify),
    'rejected',
  );
  const read = await replays.read(store, 'tiny', ROWS, verify);
  assert.equal(read.status, 'missing');
});

test('the history keeps at most eight routes, best first', async () => {
  const store = memoryKeyValue();
  for (let moves = 9; moves >= 1; moves--)
    assert.equal(await replays.promote(store, 'tiny', ROWS, route(moves, 20 - moves), verify), 'saved');
  const read = await replays.read(store, 'tiny', ROWS, verify);
  assert.equal(read.status, 'ready');
  assert.equal(read.routes.length, replays.MAX_PER_PUZZLE);
  assert.deepEqual(
    read.routes.map(saved => saved.moves),
    [1, 2, 3, 4, 5, 6, 7, 8],
  );
});

test('a stale history reads as stale, a failing one as corrupt', async () => {
  const store = memoryKeyValue();
  assert.equal(await replays.promote(store, 'tiny', ROWS, route(1, 18), verify), 'saved');
  assert.equal((await replays.read(store, 'tiny', 'other rows', verify)).status, 'stale');
  // A route that no longer verifies — the engine moved, or the record was
  // tampered with — never reaches the shelf.
  const strict: replays.Verify = () => null;
  assert.equal((await replays.read(store, 'tiny', ROWS, strict)).status, 'corrupt');
});

test('an unavailable store reads and writes as unavailable', async () => {
  const broken = { get: async () => null, update: async () => false };
  assert.equal((await replays.read(broken, 'tiny', ROWS, verify)).status, 'unavailable');
  assert.equal(await replays.promote(broken, 'tiny', ROWS, route(1, 18), verify), 'unavailable');
  assert.deepEqual(await replays.index(broken), { available: false, ids: [] });
});

test('the index lists rooms with routes, newest best first', async () => {
  const store = memoryKeyValue();
  assert.equal(await replays.promote(store, 'tiny', ROWS, route(1, 20), verify), 'saved');
  assert.equal(await replays.promote(store, 'huge', ROWS, route(2, 19), verify), 'saved');
  assert.deepEqual(await replays.index(store), { available: true, ids: ['tiny', 'huge'] });
});

test('clear empties the repository', async () => {
  const store = memoryKeyValue();
  assert.equal(await replays.promote(store, 'tiny', ROWS, route(1, 18), verify), 'saved');
  assert.equal(await replays.clear(store), true);
  assert.deepEqual(await replays.index(store), { available: true, ids: [] });
  assert.equal((await replays.read(store, 'tiny', ROWS, verify)).status, 'missing');
});

test('the repository keeps at most 250 rooms', () => {
  const puzzles: Record<string, replays.History> = {};
  for (let number = 0; number < 300; number++)
    puzzles[`room-${number}`] = {
      rows: ROWS,
      routes: [{ routeId: `r-${number}`, actionLog: 'D', moves: 1, pushes: 1, completedAt: stamp(20) }],
    };
  assert.equal(Object.keys(replays.normalize({ puzzles }).puzzles).length, 250);
});
