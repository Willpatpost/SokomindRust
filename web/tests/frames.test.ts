import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../src/storage.ts';
import * as journal from '../src/journal.ts';
import * as stats from '../src/stats.ts';
import * as frames from '../src/frames.ts';
import { ENTRIES } from '../src/catalog.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();

const DAY = 86_400_000;
const NOW = new Date('2026-10-20T12:00:00.000Z');
const stamp = (back: number) => new Date(NOW.getTime() - back * DAY).toISOString();
const tutorials = ENTRIES.filter(entry => entry.difficulty === 'tutorial');
const empty = (): journal.Journal => ({ cleared: {}, daily: {}, activity: {} });
const state = (pick: frames.FrameId, family: 'cozy-study' | 'midnight-neon', book: journal.Journal, id: frames.FrameId) =>
  frames.states(pick, family, stats.aggregates(book, NOW), book).find(frame => frame.id === id)!;

test('a frame unlocks with its achievement', () => {
  const none = empty();
  assert.equal(state('classic', 'cozy-study', none, 'sage-thread').unlocked, false);
  assert.equal(state('classic', 'cozy-study', none, 'classic').unlocked, true);
  const cleared: Record<string, journal.Clear> = {};
  for (const room of ENTRIES.slice(0, 10)) cleared[room.id] = { moves: 5, pushes: 2, clearedAt: stamp(0) };
  const ten = { cleared, daily: {}, activity: {} };
  assert.equal(state('classic', 'cozy-study', ten, 'sage-thread').unlocked, true);
});

test('the chosen frame wears only when earned and fitting', () => {
  const none = empty();
  // Locked: the choice falls back to the classic frame.
  assert.equal(frames.active('sage-thread', 'cozy-study', stats.aggregates(none, NOW), none), 'classic');
  // Earned and fitting: it wears.
  const cleared: Record<string, journal.Clear> = {};
  for (const room of ENTRIES.slice(0, 10)) cleared[room.id] = { moves: 5, pushes: 2, clearedAt: stamp(0) };
  const ten = { cleared, daily: {}, activity: {} };
  assert.equal(frames.active('sage-thread', 'cozy-study', stats.aggregates(ten, NOW), ten), 'sage-thread');
  // Brass edge never fits midnight neon, however earned.
  const graduated: Record<string, journal.Clear> = {};
  for (const room of tutorials) graduated[room.id] = { moves: 5, pushes: 2, clearedAt: stamp(0) };
  const done = { cleared: graduated, daily: {}, activity: {} };
  assert.equal(frames.active('brass-edge', 'cozy-study', stats.aggregates(done, NOW), done), 'brass-edge');
  assert.equal(frames.active('brass-edge', 'midnight-neon', stats.aggregates(done, NOW), done), 'classic');
  assert.equal(state('brass-edge', 'midnight-neon', done, 'brass-edge').compatible, false);
});

test('the frame preference reads and writes through browser storage', () => {
  assert.equal(frames.preference(), 'classic');
  storage.write('frame', 'not-a-frame');
  assert.equal(frames.preference(), 'classic');
  frames.save('brass-edge');
  assert.equal(frames.preference(), 'brass-edge');
  assert.equal(state('brass-edge', 'cozy-study', empty(), 'brass-edge').selected, true);
});
