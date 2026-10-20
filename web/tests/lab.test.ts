import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as lab from '../src/lab.ts';

const record = (overrides: Partial<lab.RunRecord> = {}): lab.RunRecord => ({
  id: 'run',
  puzzleId: 'ultra-tiny',
  actions: '',
  mode: 'fast',
  timeMs: 5000,
  memoryMiB: 64,
  engine: 'browser',
  status: 'solved',
  expanded: 100,
  generated: 200,
  reservedBytes: 1048576,
  elapsedMs: 120,
  capturedAt: '2026-10-20T12:00:00.000Z',
  ...overrides,
});

test('a run names itself by its mode, engine and outcome', () => {
  assert.equal(lab.label(record({ moves: 24 })), 'fast · browser · 24 moves');
  assert.equal(lab.label(record({ status: 'memory_limit' })), 'fast · browser · memory limit');
});

test('the populations scale to the larger count', () => {
  const rows = lab.population(100, 400);
  assert.deepEqual(
    rows.map(row => row.id),
    ['generated', 'visited'],
  );
  assert.equal(rows[0]!.value, 400);
  assert.equal(rows[0]!.share, 1);
  assert.equal(rows[1]!.value, 100);
  assert.equal(rows[1]!.share, 0.25);
  const empty = lab.population(0, 0);
  assert.equal(empty[0]!.share, 0);
  assert.equal(empty[1]!.share, 0);
});

test('the comparison names what two runs shared and where they differ', () => {
  const fast = record({ moves: 24, pushes: 10 });
  const optimal = record({ mode: 'optimal', moves: 22, pushes: 9, expanded: 300, generated: 600, elapsedMs: 400 });
  const comparison = lab.compareRuns(fast, optimal);
  assert.equal(comparison.sameInput, true);
  assert.equal(comparison.sameLimits, false);
  assert.equal(comparison.elapsedDeltaMs, -280);
  assert.equal(comparison.expandedDelta, -200);
  assert.equal(comparison.generatedDelta, -400);
  assert.equal(comparison.moveDelta, 2);
  assert.equal(comparison.pushDelta, 1);
  // A run without a route carries no route deltas.
  const capped = lab.compareRuns(record({ status: 'time_limit' }), record({ moves: 24, pushes: 10 }));
  assert.equal(capped.moveDelta, undefined);
  assert.equal(capped.pushDelta, undefined);
  // A different position is a different input.
  const moved = lab.compareRuns(record({ actions: 'DD' }), record());
  assert.equal(moved.sameInput, false);
});
