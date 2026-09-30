import { test } from 'node:test';
import assert from 'node:assert/strict';
import { keyAction, swipeDirection } from '../src/input.ts';

test('a swipe moves along its main axis, with screen y growing down', () => {
  assert.equal(swipeDirection(0, -30), 0); assert.equal(swipeDirection(0, 30), 1);
  assert.equal(swipeDirection(-30, 0), 2); assert.equal(swipeDirection(30, 0), 3);
  // Drift across the main axis is fine while it stays within half the travel.
  assert.equal(swipeDirection(-40, 12), 2); assert.equal(swipeDirection(-9, 36), 1);
});
test('a swipe travels at least 24 px along its main axis', () => {
  assert.equal(swipeDirection(24, 0), 3); assert.equal(swipeDirection(0, -24), 0);
  for (const [dx, dy] of [[23, 0], [0, -23], [23.9, 5], [0, 0]]) assert.equal(swipeDirection(dx, dy), undefined);
});
test('a swipe travels at least twice as far along one axis as the other', () => {
  assert.equal(swipeDirection(48, 24), 3); assert.equal(swipeDirection(-24, -48), 0);
  for (const [dx, dy] of [[48, 24.5], [25, 13], [-30, 30], [40, -40]]) assert.equal(swipeDirection(dx, dy), undefined);
});
test('arrow keys and WASD move in either case, and Z undoes', () => {
  for (const [key, action] of [
    ['ArrowUp', 0], ['w', 0], ['W', 0],
    ['ArrowDown', 1], ['s', 1], ['S', 1],
    ['ArrowLeft', 2], ['a', 2], ['A', 2],
    ['ArrowRight', 3], ['d', 3], ['D', 3],
    ['z', 'undo'], ['Z', 'undo'],
  ] as const) assert.equal(keyAction(key), action, key);
  for (const key of ['q', 'Enter', ' ', '', 'Shift', 'constructor']) assert.equal(keyAction(key), undefined, key);
});
