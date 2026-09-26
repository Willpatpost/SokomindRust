import { test } from 'node:test';
import assert from 'node:assert/strict';
import { decodeMetricTuple, decodeNativeReply, decodeWorkerReply } from '../src/transport.ts';
import { native, progress } from './fakes.ts';

test('native proofs normalize without guessing unknown kinds', () => {
  assert.deepEqual(decodeNativeReply(native()).metrics.proof, { kind: 'optimal', moves: 1 });
  const unsolvable = { ...native(), route: null, moves: null, pushes: null, status: 'exhausted', proof: { kind: 'unsolvable' } };
  assert.deepEqual(decodeNativeReply(unsolvable).metrics.proof, { kind: 'unsolvable' });
  assert.throws(() => decodeNativeReply({ ...unsolvable, proof: { kind: 'future-proof' } }), /Unknown solver proof/);
  assert.throws(() => decodeNativeReply({ ...native(), proof: { kind: 'bounded', lower_bound: 2, upper_bound: 1 } }), /bounds/);
  assert.throws(() => decodeNativeReply({ ...native(), proof: { kind: 'optimal', lower_bound: 0, upper_bound: 1 } }), /bounds/);
});
test('both transport boundaries reject malformed counters, statuses and routes', () => {
  for (const patch of [ { expanded: -1 }, { generated: 0.5 }, { reserved_bytes: Infinity },
    { moves: '1' }, { pushes: 2 }, { elapsed_ms: NaN }, { status: 'new_status' },
    { route: 'XD' }, { route: 'DD' }, { route: null } ])
    assert.throws(() => decodeNativeReply({ ...native(), ...patch }));
  for (const value of [null, [], 'reply', { type: 'mystery' }, { type: 'error', message: 1 },
    { ...progress(), metrics: { ...progress().metrics, proof: { kind: 'mystery' } } },
    { ...progress(), type: 'done' }, { ...progress('D'), route: 'D'.repeat(100001) }])
    assert.throws(() => decodeWorkerReply(value));
});
test('tuple ABI preserves sentinels and tolerates appended diagnostics', () => {
  const empty = decodeMetricTuple(new Uint32Array([2, 4, 128, 0xffffffff, 0, 0xffffffff]), 'running');
  assert.equal(empty.best, undefined); assert.equal(empty.lowerBound, undefined);
  assert.deepEqual(decodeMetricTuple([2, 4, 128, 5, 1, 3, 99], 'time_limit').proof,
    { kind: 'bounded', lower: 3, upper: 5 });
  assert.deepEqual(decodeMetricTuple([2, 4, 128, 1, 2, 1], 'solved').proof, { kind: 'optimal', moves: 1 });
  assert.deepEqual(decodeMetricTuple([2, 4, 128, 0xffffffff, 3, 0xffffffff], 'exhausted').proof, { kind: 'unsolvable' });
  for (const tuple of [[1], [1, 2, 3, 4, 99, 0], [1, 2, 3, 0xffffffff, 2, 0], [1, 2, 3, 2, 1, 3]])
    assert.throws(() => decodeMetricTuple(tuple, 'solved'));
  assert.throws(() => decodeMetricTuple([1, 2, 3, 0xffffffff, 0, 0xffffffff], 'unknown'));
});
