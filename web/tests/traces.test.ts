import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as traces from '../src/traces.ts';

/** A stand-in engine: a 5-wide board where every letter steps, D pushes, and
 * the puzzle solves once `solvesAt` moves have passed. `blockedAt` names the
 * move indexes whose steps refuse, in the engine's own voice. */
class FakeEngine {
  private moves = 0;
  private pushes = 0;
  private readonly solvesAt: number;
  private readonly blockedAt: readonly number[];

  constructor(solvesAt: number, blockedAt: readonly number[] = []) {
    this.solvesAt = solvesAt;
    this.blockedAt = blockedAt;
  }
  width() {
    return 5;
  }
  height() {
    return 5;
  }
  tiles() {
    return new Uint8Array(25);
  }
  labels() {
    return new Uint8Array([65]);
  }
  step(direction: number): boolean {
    if (this.blockedAt.includes(this.moves)) return false;
    this.moves++;
    this.pushes += direction === 1 ? 1 : 0;
    return true;
  }
  snapshot(): Uint32Array {
    return new Uint32Array([6 + this.moves, this.moves, this.pushes, this.moves >= this.solvesAt ? 1 : 0, 11 + this.moves]);
  }
  on_goal(index: number): boolean {
    return index === 0 && this.moves >= this.solvesAt;
  }
}

test('a route replays into frames, pushes and goal states', () => {
  const trace = traces.build(new FakeEngine(3), 'RRD');
  assert.ok(trace !== null);
  assert.equal(trace!.actionLog, 'RRD');
  assert.equal(trace!.frames.length, 4);
  assert.equal(trace!.frames[0]!.moves, 0);
  assert.equal(trace!.frames[3]!.moves, 3);
  assert.equal(trace!.frames[3]!.solved, true);
  // Only the D step pushed.
  assert.deepEqual(trace!.pushedSteps, [false, false, true]);
  assert.deepEqual(trace!.onGoals[2], [false]);
  assert.deepEqual(trace!.onGoals[3], [true]);
  // The trace owns its tiles and labels, so the engine may be freed.
  assert.equal(trace!.tiles.length, 25);
  assert.equal(trace!.labels.length, 1);
});

test('a blocked step or an unsolved route builds nothing', () => {
  assert.equal(traces.build(new FakeEngine(3, [1]), 'RRD'), null);
  assert.equal(traces.build(new FakeEngine(5), 'RRD'), null);
  assert.equal(traces.build(new FakeEngine(0), 'RX'), null);
});

test('atStep bounds to the route', () => {
  const trace = traces.build(new FakeEngine(3), 'RRD')!;
  assert.equal(traces.atStep(trace, -1).state.moves, 0);
  assert.equal(traces.atStep(trace, 2).state.moves, 2);
  assert.equal(traces.atStep(trace, 99).state.moves, 3);
  assert.deepEqual(traces.atStep(trace, 3).onGoal, [true]);
});

test('the comparison names where two routes separate', () => {
  const primary = traces.build(new FakeEngine(3), 'RRD')!;
  const reference = traces.build(new FakeEngine(3), 'DDR')!;
  const comparison = traces.compare(primary, reference);
  assert.equal(comparison.commonPrefixMoves, 0);
  assert.equal(comparison.moveDelta, 0);
  assert.equal(comparison.pushDelta, -1);
  assert.deepEqual(
    comparison.markers.map(marker => marker.symbol),
    ['D'],
  );
  assert.equal(comparison.markers[0]!.step, 1);
  assert.ok(comparison.summary.includes('the watched route goes right while the comparison goes down'));
  assert.ok(comparison.summary.includes('1 pushes fewer'));
  // A route against itself never diverges.
  const same = traces.compare(primary, primary);
  assert.deepEqual(same.markers, []);
  assert.ok(same.summary.includes('The routes use the same directions.'));
  // A shorter watched route marks where the comparison finishes.
  const short = traces.build(new FakeEngine(1), 'D')!;
  const longer = traces.build(new FakeEngine(3), 'DDR')!;
  const finish = traces.compare(short, longer);
  assert.deepEqual(
    finish.markers.map(marker => marker.symbol),
    ['D', 'F'],
  );
  assert.equal(finish.markers[1]!.step, 1);
});

test('a trace’s step reads as its position', () => {
  const trace = traces.build(new FakeEngine(3), 'RRD')!;
  assert.equal(
    traces.describe(trace, 2, 'Personal best'),
    'Personal best, move 2 of 3. Keeper at row 2, column 4. 0 pushes. Puzzle in progress.',
  );
  assert.equal(traces.describe(trace, 3, 'Personal best'), 'Personal best, move 3 of 3. Keeper at row 2, column 5. 1 push. Puzzle solved.');
});
