import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SolverClient } from '../src/solver-client.ts';
import { MAX_ROUTE, MAX_STATES, PAST_LIMIT_MESSAGE, type SolveRequest } from '../src/protocol.ts';
import { Clock, Worker, brandCheckedFetch, deferred, native, progress, withGlobalFetch } from './fakes.ts';
const request: SolveRequest = { rows: 'rows', actions: '', mode: 'optimal', maxStates: MAX_STATES, memoryMiB: 64, timeMs: 10 };
function setup(overrides: Partial<ConstructorParameters<typeof SolverClient>[0]> = {}) {
  const clock = new Clock(),
    worker = new Worker(),
    statuses: string[] = [],
    updates: unknown[] = [],
    verified: string[] = [];
  const client = new SolverClient({
    worker: () => worker,
    scheduler: clock,
    changed() {},
    elapsed() {},
    status: text => statuses.push(text),
    update: update => updates.push(update),
    verify: (prefix, route) => verified.push(prefix + route),
    ...overrides,
  });
  return { client, clock, worker, statuses, updates, verified };
}
test('synchronous construction and initial post failures restore completed state without timers', async () => {
  const construct = setup({
    worker: () => {
      throw new Error('constructor failed');
    },
  });
  await construct.client.solve('browser', request);
  assert.equal(construct.client.state.kind, 'completed');
  assert.equal(construct.clock.tasks.size, 0);
  assert.match(construct.statuses[0], /constructor failed/);
  const post = setup();
  post.worker.failPost = true;
  await post.client.solve('browser', request);
  assert.equal(post.client.busy, false);
  assert.equal(post.worker.terminated, true);
  assert.equal(post.clock.tasks.size, 0);
});
test('route is replay checked, retained after cancellation, and transport is torn down', async () => {
  const { client, worker, clock, verified } = setup();
  await client.solve('browser', { ...request, actions: 'U' });
  worker.reply(progress('D'));
  assert.deepEqual(verified, ['UD']);
  assert.equal(client.route, 'D');
  client.cancel();
  assert.equal(worker.messages.at(-1)?.type, 'cancel');
  clock.advance(1000);
  assert.equal(client.route, 'D');
  assert.equal(client.busy, false);
  assert.equal(worker.terminated, true);
  assert.equal(worker.onmessage, null);
  assert.equal(clock.tasks.size, 0);
});
test('old callbacks and watchdogs cannot finish a replacement search', async () => {
  const first = new Worker(),
    second = new Worker();
  let calls = 0;
  const { client, clock, statuses, updates } = setup({ worker: () => (calls++ ? second : first) });
  await client.solve('browser', request);
  const oldMessage = first.onmessage!,
    oldDeadline = [...clock.tasks.values()].find(t => !t.interval)!.callback;
  client.reset();
  await client.solve('browser', request);
  oldMessage({ data: { ...progress('D'), type: 'done', metrics: { ...progress('D').metrics, status: 'solved' } } } as MessageEvent);
  oldDeadline();
  assert.equal(client.busy, true);
  assert.equal(client.route, undefined);
  assert.equal(updates.length, 0);
  assert.equal(statuses.length, 0);
  client.reset();
});
test('unverified and overflowing routes are rejected before becoming playable', async () => {
  const rejected = setup({
    verify() {
      throw new Error('blocked replay');
    },
  });
  await rejected.client.solve('browser', request);
  rejected.worker.reply(progress('D'));
  assert.equal(rejected.client.route, undefined);
  assert.match(rejected.statuses[0], /blocked replay/);
  const overflow = setup();
  await overflow.client.solve('browser', { ...request, actions: 'U'.repeat(MAX_ROUTE) });
  overflow.worker.reply(progress('D'));
  assert.equal(overflow.client.route, undefined);
  assert.equal(overflow.verified.length, 0);
  // ReplayError::PastLimit's text in crates/core/src/game.rs, which POST /api/solve sends too.
  assert.deepEqual(overflow.statuses, ['Position and route together exceed the 100000-move replay limit']);
});
test('a best past the replay limit leaves no route, and the replay-limit error ends the search', async () => {
  const { client, worker, statuses, updates } = setup();
  await client.solve('browser', request);
  worker.reply({ ...progress(), metrics: { ...progress().metrics, best: MAX_ROUTE + 1 } });
  assert.equal(client.busy, true);
  assert.equal(client.route, undefined);
  assert.equal(updates.length, 1);
  worker.reply({ type: 'error', message: PAST_LIMIT_MESSAGE });
  assert.equal(client.state.kind, 'completed');
  assert.equal(client.route, undefined);
  assert.deepEqual(statuses, [PAST_LIMIT_MESSAGE]);
  assert.equal(worker.terminated, true);
});
test('unknown proofs fail safely and do not render as unsolvable', async () => {
  const { client, worker, statuses, updates } = setup();
  await client.solve('browser', request);
  worker.reply({ ...progress(), metrics: { ...progress().metrics, proof: { kind: 'future-proof' } } });
  assert.equal(client.busy, false);
  assert.match(statuses[0], /Unknown solver proof/);
  assert.equal(updates.length, 0);
});
test('final worker reply keeps a previously delivered route and releases timers', async () => {
  const { client, worker, clock } = setup();
  await client.solve('browser', request);
  worker.reply(progress('D'));
  worker.reply({ ...progress(), type: 'done', metrics: { ...progress('D').metrics, status: 'solved' } });
  assert.equal(client.state.kind, 'completed');
  assert.equal(client.route, 'D');
  assert.equal(clock.tasks.size, 0);
});
test('final optimal and bounded proofs must match the verified route length', async () => {
  const done = (best: number, proof: object, status = 'solved') => ({
    type: 'done',
    elapsedMs: 20,
    metrics: { ...progress().metrics, best, lowerBound: 1, proof, status },
  });
  const optimal = setup();
  await optimal.client.solve('browser', request);
  optimal.worker.reply(progress('DD'));
  optimal.worker.reply(done(1, { kind: 'optimal', moves: 1 }));
  assert.equal(optimal.client.state.kind, 'completed');
  assert.equal(optimal.client.route, 'DD');
  assert.match(optimal.statuses[0], /Final proof does not match/);
  assert.equal(optimal.updates.length, 1);
  assert.equal(optimal.clock.tasks.size, 0);
  const bounded = setup();
  await bounded.client.solve('browser', request);
  bounded.worker.reply(progress('DD'));
  bounded.worker.reply(done(3, { kind: 'bounded', lower: 1, upper: 3 }));
  assert.equal(bounded.client.route, 'DD');
  assert.match(bounded.statuses[0], /Final proof does not match/);
  assert.equal(bounded.updates.length, 1);
  const routeless = setup();
  await routeless.client.solve('browser', request);
  routeless.worker.reply(done(2, { kind: 'bounded', lower: 1, upper: 2 }, 'state_limit'));
  assert.equal(routeless.client.route, undefined);
  assert.match(routeless.statuses[0], /Final proof does not match/);
  assert.equal(routeless.updates.length, 0);
  const matching = setup();
  await matching.client.solve('browser', request);
  matching.worker.reply(progress('D'));
  matching.worker.reply(done(1, { kind: 'optimal', moves: 1 }));
  assert.equal(matching.client.state.kind, 'completed');
  assert.equal(matching.client.route, 'D');
  assert.deepEqual(matching.statuses, []);
  assert.equal(matching.updates.length, 2);
});
test('native cancellation ignores late successful responses', async () => {
  const pending = deferred<Response>();
  let signal: AbortSignal | undefined;
  const { client, updates } = setup({
    fetch: (async (_url, options) => {
      signal = options?.signal as AbortSignal;
      return pending.promise;
    }) as typeof fetch,
  });
  const done = client.solve('native', request);
  client.cancel();
  assert.equal(signal?.aborted, true);
  pending.resolve(Response.json(native()));
  await done;
  assert.equal(client.state.kind, 'idle');
  assert.equal(updates.length, 0);
});
test('native timeout completes immediately even if a transport ignores abort', async () => {
  const pending = deferred<Response>();
  const { client, clock, statuses, updates } = setup({ fetch: () => pending.promise });
  const done = client.solve('native', request);
  clock.advance(request.timeMs + 5000);
  assert.equal(client.busy, false);
  assert.equal(clock.tasks.size, 0);
  assert.match(statuses[0], /exceeded/);
  pending.resolve(Response.json(native()));
  await done;
  assert.equal(updates.length, 0);
});
test('native reply is normalized and verified, busy API failure leaves client usable', async () => {
  const good = setup({ fetch: async () => Response.json(native()) });
  await good.client.solve('native', request);
  assert.equal(good.client.route, 'D');
  assert.deepEqual(good.verified, ['D']);
  assert.equal(good.clock.tasks.size, 0);
  const limited = 'Too many solve requests; try the browser solver or try again shortly';
  const busy = setup({ fetch: async () => Response.json({ error: limited }, { status: 429 }) });
  await busy.client.solve('native', request);
  assert.equal(busy.statuses[0], limited);
  assert.equal(busy.client.busy, false);
  assert.equal(busy.clock.tasks.size, 0);
  const bare = setup({ fetch: async () => new Response(null, { status: 429 }) });
  await bare.client.solve('native', request);
  assert.match(bare.statuses[0], /^Server returned HTTP 429\. The browser solver still works/);
});
test('native default transport passes the browser fetch brand check', async () => {
  await withGlobalFetch(
    brandCheckedFetch(async () => Response.json(native())),
    async () => {
      const { client, statuses } = setup();
      await client.solve('native', request);
      assert.equal(client.route, 'D');
      assert.deepEqual(statuses, []);
      assert.equal(client.state.kind, 'completed');
    },
  );
});
