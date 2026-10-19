import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clampPan, MAX_ZOOM, pinchScale } from '../src/zoom.ts';

test('a pinch scales between 1 and the zoom cap, from the gesture baseline', () => {
  assert.equal(pinchScale(1, 100, 200), 2);
  assert.equal(pinchScale(1, 100, 100 * MAX_ZOOM), MAX_ZOOM);
  assert.equal(pinchScale(2, 200, 100), 1);
});

test('a pan stops where the scaled board still covers its room', () => {
  assert.equal(clampPan(50, 200, 100), 50);
  assert.equal(clampPan(-80, 200, 100), -50);
  assert.equal(clampPan(30, 100, 100), 0);
  assert.equal(clampPan(-5, 100, 100), 0);
});
