import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MAX_ROUTE, STATUSES } from '../src/protocol.ts';
import {
  decodeHealth, decodeMetricTuple, decodeNativeReply, decodeProgress, decodeSaveReply, decodeSnapshot, decodeWorkerReply,
  encodeSolveRequest, nativeErrorText,
} from '../src/transport.ts';
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
    { moves: '1' }, { pushes: 2 }, { elapsed_ms: NaN }, { status: 'new_status' }, { status: 'running' },
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
test('a best past MAX_ROUTE is a legal count, but a route past it is still refused', () => {
  for (const best of [MAX_ROUTE + 1, 0xfffffffe])
    assert.equal(decodeMetricTuple([2, 4, 128, best, 0, 0xffffffff], 'running').best, best);
  const proven = decodeMetricTuple([2, 4, 128, MAX_ROUTE + 1, 2, MAX_ROUTE + 1], 'solved');
  assert.deepEqual(proven.proof, { kind: 'optimal', moves: MAX_ROUTE + 1 });
  const over = { ...progress(), metrics: { ...progress().metrics, best: MAX_ROUTE + 1 } };
  const reply = decodeWorkerReply(over);
  assert.ok(reply.type === 'progress' && reply.metrics.best === MAX_ROUTE + 1 && reply.route === undefined);
  assert.throws(() => decodeWorkerReply({ ...over, route: 'D'.repeat(MAX_ROUTE + 1) }), /Invalid solver route/);
});
test('every search status decodes as itself and no other string does', () => {
  const none = [1, 2, 3, 0xffffffff, 0, 0xffffffff];
  for (const status of STATUSES) assert.equal(decodeMetricTuple(none, status).status, status);
  for (const status of ['Solved', 'stopped', '', null]) assert.throws(() => decodeMetricTuple(none, status), /Unknown solver status/);
});
test('snapshot ABI decodes the header and views the box cells', () => {
  const raw = new Uint32Array([7, 5, 2, 1, 12, 13]);
  const { boxes, ...header } = decodeSnapshot(raw, 2);
  assert.deepEqual(header, { player: 7, moves: 5, pushes: 2, solved: true });
  assert.deepEqual(Array.from(boxes), [12, 13]);
  assert.equal(boxes.buffer, raw.buffer, 'box cells are a view, not a copy');
  assert.equal(boxes.byteOffset, 4 * Uint32Array.BYTES_PER_ELEMENT);
  assert.equal(decodeSnapshot(new Uint32Array([7, 0, 0, 0]), 0).solved, false);
  assert.equal(decodeSnapshot(new Uint32Array([7, 100000, 0, 0, 9]), 1).moves, 100000);
  const lengths: [number[], number][] = [[[7, 5, 2, 1, 12], 2], [[7, 5, 2, 1, 12, 13], 1], [[7, 5, 2], 0]];
  for (const [values, count] of lengths)
    assert.throws(() => decodeSnapshot(new Uint32Array(values), count), /Invalid WASM snapshot length/);
  // A solved flag other than 0/1, more pushes than moves, or a count past MAX_ROUTE.
  for (const values of [[7, 5, 2, 2, 12], [7, 2, 5, 0, 12], [7, 100001, 0, 0, 12]])
    assert.throws(() => decodeSnapshot(new Uint32Array(values), 1), /Invalid WASM snapshot counters/);
});
test('solve requests encode to the server field names in a fixed order', () => {
  const body = encodeSolveRequest({ rows: 'OOO\nORO', actions: 'D', mode: 'quality', maxStates: 1000, memoryMiB: 32, timeMs: 5000 });
  assert.equal(body, '{"rows":["OOO","ORO"],"actions":"D","mode":"quality","time_ms":5000,"max_states":1000,"memory_mib":32}');
});
test('native errors show the server text as sent, and only a bare 429 points to the browser solver', () => {
  for (const [status, error] of [[429, 'Solver busy; try the browser solver or retry later'],
    [429, 'Too many solve requests; try the browser solver or try again shortly'],
    [429, 'Too many requests; try the browser solver or try again shortly'], [400, 'Unknown search mode']] as const)
    assert.equal(nativeErrorText(status, error), error);
  assert.equal(nativeErrorText(429, undefined), 'Server returned HTTP 429. The browser solver still works: set Run on to This browser.');
  assert.equal(nativeErrorText(502, undefined), 'Server returned HTTP 502');
});
test('health replies report persistence only when it is exactly true', () => {
  assert.equal(decodeHealth({ status: 'ok', persistence: true }), true);
  for (const value of [{ status: 'ok', persistence: false }, { status: 'ok' }, { persistence: 'true' }, [{ persistence: true }]])
    assert.equal(decodeHealth(value), false);
  for (const value of [null, 'ok', 1, '']) assert.equal(decodeHealth(value), undefined);
});
test('save replies must say whether the route improved the stored best', () => {
  assert.equal(decodeSaveReply({ saved: true, improved: true }), true);
  assert.equal(decodeSaveReply({ saved: true, improved: false }), false);
  for (const value of [null, [], {}, { saved: true, improved: 'yes' }])
    assert.throws(() => decodeSaveReply(value), /Invalid progress response/);
});
test('stored progress decodes to a full route with the counts the server reported', () => {
  const stored = { puzzle_id: 'p', route: 'DD', moves: 2, pushes: 1 };
  assert.deepEqual(decodeProgress(stored), { puzzleId: 'p', route: 'DD', moves: 2, pushes: 1 });
  for (const [patch, error] of [[{ puzzle_id: 1 }, /Invalid progress record/], [{ route: null }, /Invalid progress record/],
    [{ route: 'XD' }, /Invalid progress route/], [{ route: 'D'.repeat(100001) }, /Invalid progress route/],
    [{ moves: -1 }, /Invalid progress moves/], [{ moves: '2' }, /Invalid progress moves/],
    [{ pushes: 0.5 }, /Invalid progress pushes/]] as const)
    assert.throws(() => decodeProgress({ ...stored, ...patch }), error);
  for (const value of [null, [], 'DD']) assert.throws(() => decodeProgress(value), /Invalid progress record/);
});
