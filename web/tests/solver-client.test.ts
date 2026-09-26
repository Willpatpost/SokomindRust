import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SolverClient } from '../src/solver-client.ts';
import { MAX_ROUTE } from '../src/protocol.ts';
import { Clock, Worker, deferred, native, progress } from './fakes.ts';
const request = { rows: 'rows', actions: '', mode: 'optimal', maxStates: 500000, memoryMiB: 64, timeMs: 10 };
function setup(overrides: Partial<ConstructorParameters<typeof SolverClient>[0]> = {}) {
  const clock = new Clock(), worker = new Worker(), statuses: string[] = [], updates: unknown[] = [], verified: string[] = [];
  const client = new SolverClient({ worker: () => worker, scheduler: clock, changed() {}, elapsed() {},
    status: text => statuses.push(text), update: update => updates.push(update),
    verify: (prefix, route) => verified.push(prefix + route), ...overrides });
  return { client, clock, worker, statuses, updates, verified };
}
test('synchronous construction and initial post failures restore completed state without timers', async () => {
  const construct = setup({ worker: () => { throw new Error('constructor failed'); } });
  await construct.client.solve('browser', request);
  assert.equal(construct.client.state.kind, 'completed'); assert.equal(construct.clock.tasks.size, 0);
  assert.match(construct.statuses[0], /constructor failed/);
  const post = setup(); post.worker.failPost = true;
  await post.client.solve('browser', request);
  assert.equal(post.client.busy, false); assert.equal(post.worker.terminated, true); assert.equal(post.clock.tasks.size, 0);
});
test('route is replay checked, retained after cancellation, and transport is torn down', async () => {
  const { client, worker, clock, verified } = setup();
  await client.solve('browser', { ...request, actions: 'U' });
  worker.reply(progress('D'));
  assert.deepEqual(verified, ['UD']); assert.equal(client.route, 'D');
  client.cancel(); assert.equal(worker.messages.at(-1)?.type, 'cancel');
  clock.advance(1000);
  assert.equal(client.route, 'D'); assert.equal(client.busy, false); assert.equal(worker.terminated, true);
  assert.equal(worker.onmessage, null); assert.equal(clock.tasks.size, 0);
});
test('old callbacks and watchdogs cannot finish a replacement search', async () => {
  const first = new Worker(), second = new Worker(); let calls = 0;
  const { client, clock, statuses, updates } = setup({ worker: () => calls++ ? second : first });
  await client.solve('browser', request);
  const oldMessage = first.onmessage!, oldDeadline = [...clock.tasks.values()].find(t => !t.interval)!.callback;
  client.reset(); await client.solve('browser', request);
  oldMessage({ data: { ...progress('D'), type: 'done', metrics: { ...progress('D').metrics, status: 'solved' } } } as MessageEvent);
  oldDeadline();
  assert.equal(client.busy, true); assert.equal(client.route, undefined); assert.equal(updates.length, 0); assert.equal(statuses.length, 0);
  client.reset();
});
test('unverified and overflowing routes are rejected before becoming playable', async () => {
  const rejected = setup({ verify() { throw new Error('blocked replay'); } });
  await rejected.client.solve('browser', request); rejected.worker.reply(progress('D'));
  assert.equal(rejected.client.route, undefined); assert.match(rejected.statuses[0], /blocked replay/);
  const overflow = setup(); await overflow.client.solve('browser', { ...request, actions: 'U'.repeat(MAX_ROUTE) });
  overflow.worker.reply(progress('D')); assert.equal(overflow.client.route, undefined); assert.equal(overflow.verified.length, 0);
});
test('unknown proofs fail safely and do not render as unsolvable', async () => {
  const { client, worker, statuses, updates } = setup(); await client.solve('browser', request);
  worker.reply({ ...progress(), metrics: { ...progress().metrics, proof: { kind: 'future-proof' } } });
  assert.equal(client.busy, false); assert.match(statuses[0], /Unknown solver proof/); assert.equal(updates.length, 0);
});
test('final worker reply keeps a previously delivered route and releases timers', async () => {
  const { client, worker, clock } = setup(); await client.solve('browser', request); worker.reply(progress('D'));
  worker.reply({ ...progress(), type: 'done', metrics: { ...progress('D').metrics, status: 'solved' } });
  assert.equal(client.state.kind, 'completed'); assert.equal(client.route, 'D'); assert.equal(clock.tasks.size, 0);
});
test('native cancellation ignores late successful responses', async () => {
  const pending = deferred<Response>(); let signal: AbortSignal | undefined;
  const { client, updates } = setup({ fetch: (async (_url, options) => {
    signal = options?.signal as AbortSignal; return pending.promise;
  }) as typeof fetch });
  const done = client.solve('native', request); client.cancel();
  assert.equal(signal?.aborted, true); pending.resolve(Response.json(native())); await done;
  assert.equal(client.state.kind, 'idle'); assert.equal(updates.length, 0);
});
test('native timeout completes immediately even if a transport ignores abort', async () => {
  const pending = deferred<Response>(); const { client, clock, statuses, updates } = setup({ fetch: () => pending.promise });
  const done = client.solve('native', request); clock.advance(request.timeMs + 5000);
  assert.equal(client.busy, false); assert.equal(clock.tasks.size, 0); assert.match(statuses[0], /exceeded/);
  pending.resolve(Response.json(native())); await done; assert.equal(updates.length, 0);
});
test('native reply is normalized and verified, busy API failure leaves client usable', async () => {
  const good = setup({ fetch: async () => Response.json(native()) }); await good.client.solve('native', request);
  assert.equal(good.client.route, 'D'); assert.deepEqual(good.verified, ['D']); assert.equal(good.clock.tasks.size, 0);
  const busy = setup({ fetch: async () => Response.json({ error: 'Too many solve requests' }, { status: 429 }) });
  await busy.client.solve('native', request); assert.match(busy.statuses[0], /browser solver still works/); assert.equal(busy.client.busy, false);
});
