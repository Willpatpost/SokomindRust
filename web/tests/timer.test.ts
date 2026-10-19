import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Clock, installSessionStorage } from './fakes.ts';
import { Timer, clock } from '../src/timer.ts';

test('the clock reads M:SS, then H:MM:SS once an hour has passed', () => {
  assert.equal(clock(0), '0:00');
  assert.equal(clock(59_999), '0:59');
  assert.equal(clock(60_000), '1:00');
  assert.equal(clock(3_723_000), '1:02:03');
});

test('the timer accumulates while running and holds while paused', () => {
  const scheduler = new Clock();
  const timer = new Timer({ scheduler, show: () => {} });
  timer.select('a');
  scheduler.advance(2500);
  assert.equal(timer.elapsed(), 2500);
  timer.pause();
  scheduler.advance(1000);
  assert.equal(timer.elapsed(), 2500);
  timer.run();
  scheduler.advance(500);
  assert.equal(timer.elapsed(), 3000);
  timer.stop();
});

test('selecting a puzzle restores the time its session stored, and reset clears it', () => {
  const scheduler = new Clock();
  installSessionStorage();
  const timer = new Timer({ scheduler, show: () => {} });
  timer.select('a');
  scheduler.advance(1500);
  timer.pause();
  timer.select('b');
  assert.equal(timer.elapsed(), 0);
  timer.select('a');
  assert.equal(timer.elapsed(), 1500);
  timer.reset('a');
  assert.equal(timer.elapsed(), 0);
  timer.select('b');
  timer.select('a');
  assert.equal(timer.elapsed(), 0);
  timer.stop();
});
