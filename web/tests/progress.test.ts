import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ProgressClient } from '../src/progress.ts';
import { Clock, brandCheckedFetch, deferred, installMemoryStorage, withGlobalFetch } from './fakes.ts';
const storage = installMemoryStorage();
const PROFILE = 'a'.repeat(32);
const SAVED = 'Verified best route saved in PostgreSQL for this browser profile.';
const FAILED = 'Server save failed. Local progress is still available if browser storage is enabled.';
const missing = () => Promise.resolve(new Response(null, { status: 404 }));
type Reply = (url: string, init?: RequestInit) => Promise<Response>;
function setup(reply: Reply = async () => Response.json({ improved: true }),
  overrides: Partial<ConstructorParameters<typeof ProgressClient>[0]> = {}) {
  storage.clear();
  const statuses: string[] = [], shown: unknown[] = [], calls: { url: string; init?: RequestInit }[] = [];
  const client = new ProgressClient({ profile: PROFILE, scheduler: new Clock(),
    verify: (_rows, route) => ({ moves: route.length, pushes: 1 }),
    show: best => shown.push(best), status: text => statuses.push(text),
    fetch: ((url, init) => { calls.push({ url: String(url), init }); return reply(String(url), init); }) as typeof fetch,
    ...overrides });
  return { client, statuses, shown, calls };
}
// persistence starts false, so select() itself does not pull.
function connect(client: ProgressClient, id = 'ultra-tiny') {
  client.select(id, 'rows'); client.persistence = true;
}
test('sync posts the route with the profile header and reports the server verdict', async () => {
  const { client, calls, statuses } = setup(); connect(client); await client.sync('D');
  assert.equal(calls.length, 1);
  const { url, init } = calls[0], headers = init?.headers as Record<string, string>;
  assert.equal(url, '/api/progress/ultra-tiny'); assert.equal(init?.method, 'POST');
  assert.equal(headers['x-profile-id'], PROFILE); assert.equal(headers['content-type'], 'application/json');
  assert.deepEqual(JSON.parse(String(init?.body)), { route: 'D' }); assert.ok(init?.signal instanceof AbortSignal);
  assert.equal(statuses.at(-1), SAVED);
  const equal = setup(async () => Response.json({ improved: false })); connect(equal.client); await equal.client.sync('D');
  assert.equal(equal.statuses.at(-1), 'Server already stored an equal or better route for this puzzle.');
  const encoded = setup(); connect(encoded.client, 'a b/c'); await encoded.client.sync('D');
  assert.equal(encoded.calls[0].url, '/api/progress/a%20b%2Fc');
});
test('remote calls are skipped without persistence, a profile, or a catalog puzzle', async () => {
  for (const [id, persistence, overrides] of [['p', false, {}], ['p', true, { profile: null }], ['custom', true, {}]] as const) {
    const { client, calls, statuses } = setup(undefined, overrides);
    client.select(id, 'rows'); client.persistence = persistence;
    await client.sync('D'); await client.pull();
    assert.equal(calls.length, 0); assert.deepEqual(statuses, []);
  }
});
test('sync maps rate limits, rejections and failures to distinct statuses', async () => {
  const status = async (reply: Reply) => {
    const { client, statuses } = setup(reply); connect(client); await client.sync('D'); return statuses.at(-1);
  };
  assert.match(await status(async () => Response.json({ error: 'Too many saves' }, { status: 429 })) ?? '', /rate-limited/);
  assert.equal(await status(async () => Response.json({ error: 'Route does not solve' }, { status: 400 })),
    'Server save rejected: Route does not solve');
  assert.equal(await status(async () => new Response(null, { status: 422 })), 'Server save rejected: HTTP 422');
  for (const reply of [async () => Response.json({ error: 'down' }, { status: 503 }), async () => Response.json({}),
    () => Promise.reject(new TypeError('offline'))])
    assert.equal(await status(reply), FAILED);
});
test('replies for a previous puzzle never update status or best', async () => {
  const save = deferred<Response>();
  const stale = setup((_url, init) => init?.method === 'POST' ? save.promise : missing()); connect(stale.client, 'one');
  const pending = stale.client.sync('D'); stale.client.select('two', 'rows');
  save.resolve(Response.json({ improved: true })); await pending;
  assert.ok(!stale.statuses.some(text => text.startsWith('Verified')));
  const load = deferred<Response>();
  const late = setup(url => url.endsWith('/one') ? load.promise : missing()); connect(late.client, 'one');
  const pulled = late.client.pull(); late.client.select('two', 'rows');
  load.resolve(Response.json({ puzzle_id: 'one', route: 'D', moves: 1, pushes: 1 })); await pulled;
  assert.equal(late.client.best(), null); assert.equal(late.shown.at(-1), null);
  assert.ok(![...storage.keys()].some(key => key.includes('best.')));
});
test('pull keeps only a server route that replays to the reported score', async () => {
  const stored = { puzzle_id: 'p', route: 'DD', moves: 2, pushes: 1 };
  const kept = setup(async () => Response.json(stored)); connect(kept.client, 'p'); await kept.client.pull();
  assert.deepEqual(kept.shown.at(-1), { rows: 'rows', route: 'DD', moves: 2, pushes: 1 });
  assert.equal(kept.client.best()?.route, 'DD');
  const rejected = [{ ...stored, moves: 3 }, { ...stored, puzzle_id: 'q' }, { ...stored, route: 'XD' }]
    .map(body => () => setup(async () => Response.json(body)));
  rejected.push(() => setup(async () => Response.json(stored), { verify() { throw new Error('Route does not solve this puzzle'); } }),
    () => setup(missing));
  for (const make of rejected) {
    const { client, shown, statuses } = make(); connect(client, 'p'); await client.pull();
    assert.equal(client.best(), null); assert.deepEqual(shown, [null]); assert.deepEqual(statuses, []);
  }
});
test('default and injected transports call fetch unbound (browser brand check)', async () => {
  const stub = brandCheckedFetch(async () => Response.json({ improved: true }));
  await assert.rejects(({ request: stub }).request('/x'), /Illegal invocation/);
  await withGlobalFetch(stub, async () => {
    const standard = setup(undefined, { fetch: undefined }); connect(standard.client); await standard.client.sync('D');
    assert.equal(standard.statuses.at(-1), SAVED);
  });
  const injected = setup(undefined, { fetch: stub }); connect(injected.client); await injected.client.sync('D');
  assert.equal(injected.statuses.at(-1), SAVED);
});
// Lets an unawaited pull finish without hanging the suite if it never does.
async function settle(done: () => boolean) {
  for (let i = 0; i < 50 && !done(); i++) await new Promise(resolve => setImmediate(resolve));
}
test('health probe asks again with backoff until the server reports persistence, then pulls', async () => {
  const clock = new Clock(), connected: boolean[] = [];
  const health = [Response.json({ status: 'ok', persistence: false }), Response.json({ error: 'busy' }, { status: 503 }),
    Response.json({ status: 'ok', persistence: true })];
  const { client, calls } = setup(async url => url === '/api/health' ? health.shift()!
    : Response.json({ puzzle_id: 'p', route: 'DD', moves: 2, pushes: 1 }),
  { scheduler: clock, connected: value => connected.push(value) });
  client.select('p', 'rows'); await client.probe();
  assert.equal(client.persistence, false); assert.deepEqual(connected, [false]);
  assert.deepEqual([...clock.tasks.values()].map(task => task.at), [2000]);
  clock.advance(2000); await client.probe();
  assert.equal(client.persistence, false); assert.deepEqual([...clock.tasks.values()].map(task => task.at), [6000]);
  clock.advance(4000); await client.probe();
  assert.equal(client.persistence, true); assert.deepEqual(connected, [false, true]); assert.equal(clock.tasks.size, 0);
  assert.deepEqual(calls.map(call => call.url), ['/api/health', '/api/health', '/api/health', '/api/progress/p']);
  await settle(() => client.best() !== null); assert.equal(client.best()?.route, 'DD');
});
test('health probe backs off from 2 s to a 5 minute cap', async () => {
  const clock = new Clock(), delays: number[] = [];
  const { client } = setup(async () => Response.json({ status: 'ok', persistence: false }), { scheduler: clock });
  await client.probe();
  for (let i = 0; i < 10; i++) {
    const wait = [...clock.tasks.values()][0].at - clock.time;
    delays.push(wait); clock.advance(wait); await client.probe();
  }
  assert.deepEqual(delays, [2000, 4000, 8000, 16000, 32000, 64000, 128000, 256000, 300000, 300000]);
});
test('health probe stops without the API, retries offline, and shares one request', async () => {
  for (const reply of [missing, async () => new Response('<!doctype html>', { status: 200 })]) {
    const clock = new Clock(), connected: boolean[] = [];
    const { client, calls } = setup(reply, { scheduler: clock, connected: value => connected.push(value) });
    await client.probe();
    assert.equal(calls.length, 1); assert.equal(clock.tasks.size, 0); assert.equal(client.persistence, false);
    assert.deepEqual(connected, []);
  }
  const offlineClock = new Clock();
  const offline = setup(() => Promise.reject(new TypeError('offline')), { scheduler: offlineClock });
  await offline.client.probe(); assert.equal(offline.client.persistence, false); assert.equal(offlineClock.tasks.size, 1);
  const clock = new Clock(), pending = deferred<Response>();
  const shared = setup(() => pending.promise, { scheduler: clock });
  const first = shared.client.probe();
  assert.equal(shared.client.probe(), first); assert.equal(shared.calls.length, 1);
  pending.resolve(Response.json({ error: 'busy' }, { status: 503 })); await first;
  assert.equal(clock.tasks.size, 1); assert.equal(shared.calls.length, 1);
});
const posts = (calls: { init?: RequestInit }[]) => calls.filter(call => call.init?.method === 'POST').length;
test('sync posts a route the server has answered for only once, across puzzle switches', async () => {
  const { client, calls, statuses } = setup(); connect(client, 'p');
  await client.sync('DD'); await client.sync('DD');
  assert.equal(posts(calls), 1); assert.deepEqual(statuses, [SAVED]);
  client.select('q', 'rows'); client.select('p', 'rows'); await client.sync('DD');
  assert.equal(posts(calls), 1);
  await client.sync('DDD'); assert.equal(posts(calls), 2); assert.deepEqual(statuses, [SAVED, SAVED]);
});
test('sync posts a route again until the server answers for it', async () => {
  const replies = [Response.json({ error: 'down' }, { status: 503 }), Response.json({}), Response.json({ improved: false })];
  const { client, calls, statuses } = setup(async () => replies.shift()!); connect(client, 'p');
  for (let i = 0; i < 4; i++) await client.sync('DD');
  assert.equal(posts(calls), 3);
  assert.deepEqual(statuses, [FAILED, FAILED, 'Server already stored an equal or better route for this puzzle.']);
});
test('sync does not post back the route pull took from the server', async () => {
  const { client, calls } = setup(async (_url, init) => init?.method === 'POST' ? Response.json({ improved: true })
    : Response.json({ puzzle_id: 'p', route: 'DD', moves: 2, pushes: 1 }));
  connect(client, 'p'); await client.pull(); await client.sync('DD');
  assert.equal(client.best()?.route, 'DD'); assert.equal(posts(calls), 0);
  await client.sync('DDD'); assert.equal(posts(calls), 1);
});
test('keep returns early on a tie with the best on show, and a better route still replaces it', () => {
  let verified = 0;
  const { client, shown } = setup(undefined, { verify: (_rows, route) => { verified++; return { moves: route.length, pushes: 1 }; } });
  client.select('p', 'rows'); client.keep('DDD', 3, 1);
  assert.deepEqual(shown, [null, { rows: 'rows', route: 'DDD', moves: 3, pushes: 1 }]); assert.equal(verified, 0);
  client.keep('UUU', 3, 1);
  assert.equal(shown.length, 2); assert.equal(verified, 0);
  client.keep('DD', 2, 1);
  assert.equal(verified, 1); assert.deepEqual(shown.at(-1), { rows: 'rows', route: 'DD', moves: 2, pushes: 1 });
  assert.equal(client.best()?.route, 'DD');
});
