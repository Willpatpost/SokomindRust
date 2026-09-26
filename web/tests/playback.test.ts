import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Playback } from '../src/playback.ts';
import { Clock } from './fakes.ts';
test('pause/resume preserves index, completion stops the timer', () => {
  const clock = new Clock(), moves: number[] = [], ends: boolean[] = [];
  const playback = new Playback(direction => { moves.push(direction); return true; }, () => {}, blocked => ends.push(blocked), clock);
  playback.play('UDLR'); clock.advance(70); playback.toggle(); clock.advance(700);
  assert.deepEqual(moves, [0]); assert.equal(playback.state.kind, 'paused');
  playback.toggle(); clock.advance(280);
  assert.deepEqual(moves, [0, 1, 2, 3]); assert.deepEqual(ends, [false]); assert.equal(clock.tasks.size, 0);
});
test('stop and blocked replay invalidate old callbacks', () => {
  const clock = new Clock(); let moves = 0; const ends: boolean[] = [];
  const playback = new Playback(() => { moves++; return false; }, () => {}, blocked => ends.push(blocked), clock);
  playback.play('D'); const old = [...clock.tasks.values()][0].callback; playback.stop(); old();
  assert.equal(moves, 0); playback.play('D'); clock.advance(70);
  assert.deepEqual(ends, [true]); assert.equal(playback.active, false); assert.equal(clock.tasks.size, 0);
});
