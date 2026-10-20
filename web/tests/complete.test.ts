import { test } from 'node:test';
import assert from 'node:assert/strict';
import { present } from '../src/complete.ts';
import type { Best } from '../src/storage.ts';

const best = (moves: number, pushes: number): Best => ({ rows: 'rows', route: 'D'.repeat(moves), moves, pushes });

test('a first solve is a first clear', () => {
  assert.deepEqual(present({ moves: 24, pushes: 12 }, null), {
    eyebrow: 'FIRST CLEAR',
    title: 'Room cleared',
    summary: 'Solved in 24 moves and 12 pushes — your first route here.',
    milestones: ['First clear'],
  });
});

test('a better solve is a personal milestone, by moves or by pushes', () => {
  assert.deepEqual(present({ moves: 20, pushes: 11 }, best(24, 10)), {
    eyebrow: 'PERSONAL MILESTONE',
    title: 'New personal best',
    summary: '20 moves — 4 fewer than your best.',
    milestones: ['Fewer moves than your best'],
  });
  assert.deepEqual(present({ moves: 24, pushes: 8 }, best(24, 10)), {
    eyebrow: 'PERSONAL MILESTONE',
    title: 'New personal best',
    summary: '8 pushes — 2 fewer at your best moves.',
    milestones: ['Fewer pushes at your best moves'],
  });
});

test('a matching or worse solve is a plain room cleared', () => {
  const plain = present({ moves: 24, pushes: 10 }, best(24, 10));
  assert.equal(plain.eyebrow, 'ROOM CLEARED');
  assert.deepEqual(plain.milestones, []);
  const worse = present({ moves: 30, pushes: 12 }, best(24, 10));
  assert.equal(worse.eyebrow, 'ROOM CLEARED');
  assert.deepEqual(worse.milestones, []);
});

test('a solve that matches the certified optimum earns the star', () => {
  assert.deepEqual(present({ moves: 24, pushes: 12 }, null, { moves: 24 }), {
    eyebrow: 'FIRST CLEAR',
    title: 'Room cleared',
    summary: 'Solved in 24 moves and 12 pushes — your first route here.',
    milestones: ['First clear', '★ Proven move-optimal'],
  });
  const improved = present({ moves: 20, pushes: 11 }, best(24, 10), { moves: 20 });
  assert.deepEqual(improved.milestones, ['Fewer moves than your best', '★ Proven move-optimal']);
  const plain = present({ moves: 24, pushes: 10 }, best(24, 10), { moves: 24 });
  assert.deepEqual(plain.milestones, ['★ Proven move-optimal']);
});

test('a solve short of the certified optimum earns no star', () => {
  const near = present({ moves: 25, pushes: 12 }, null, { moves: 24 });
  assert.deepEqual(near.milestones, ['First clear']);
});
