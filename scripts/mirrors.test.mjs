import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Values that one file copies from another by hand: limits, names and versions
// shared across languages, configs, CI and the README, each copy beside a
// comment naming the original where its format has comments. The checks read
// the source text, so no build is needed.

/**
 * A repo file's text.
 * @param {string} path Relative to the repo root.
 * @returns {string}
 */
function source(path) {
  return readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
}

/**
 * The text a repo file's only match of `pattern` captures. A Markdown file's whitespace runs
 * match as single spaces, so re-wrapping a README paragraph keeps its pins.
 * @param {string} path Relative to the repo root.
 * @param {RegExp} pattern One capture group.
 * @returns {string}
 */
function text(path, pattern) {
  const flags = pattern.flags.includes('g') ? pattern.flags : `${pattern.flags}g`;
  const body = path.endsWith('.md') ? source(path).replaceAll(/\s+/g, ' ') : source(path);
  const matches = [...body.matchAll(new RegExp(pattern.source, flags))];
  assert.equal(matches.length, 1, `${path} matches ${pattern} ${matches.length} times, not once`);
  const value = matches[0][1];
  assert.ok(value, `${path}'s match of ${pattern} captures nothing`);
  return value;
}

/**
 * The number a repo file's only match of `pattern` captures, digit separators removed.
 * @param {string} path Relative to the repo root.
 * @param {RegExp} pattern One capture group around the number.
 * @returns {number}
 */
function read(path, pattern) {
  return Number(text(path, pattern).replaceAll(/[_,]/g, ''));
}

/**
 * Every capture of `item` inside the text that `block` captures in a repo file, in order.
 * @param {string} path Relative to the repo root.
 * @param {RegExp} block One capture group around the list.
 * @param {RegExp} item A global pattern with one capture group per entry.
 * @returns {string[]}
 */
function items(path, block, item) {
  return [...text(path, block).matchAll(item)].map(match => match[1]);
}

test("nginx's client_max_body_size equals the server's BODY_LIMIT, so nginx passes every body the API accepts", () => {
  assert.equal(
    read('deploy/nginx.conf', /client_max_body_size (\d+)k;/),
    read('crates/server/src/main.rs', /const BODY_LIMIT: usize = (\d+) \* 1024;/),
  );
});

test("the README's 413 line and BODY_TIMEOUT's doc quote BODY_LIMIT in KiB", () => {
  const limit = read('crates/server/src/main.rs', /const BODY_LIMIT: usize = (\d+) \* 1024;/);
  assert.equal(read('README.md', /413 a body over (\d+) KiB/), limit);
  assert.equal(read('crates/server/src/api.rs', /bodies are at most (\d+) KiB/), limit);
});

test("the README's 408 line quotes BODY_TIMEOUT", () => {
  assert.equal(
    read('README.md', /body not received within (\d+) seconds/),
    read('crates/server/src/api.rs', /const BODY_TIMEOUT: Duration = Duration::from_secs\((\d+)\);/),
  );
});

test("compose's stop_grace_period outlasts the longest native solve, the top of TIME_MS", () => {
  const grace = read('compose.yaml', /stop_grace_period: (\d+)s/);
  const top = read('crates/server/src/solve.rs', /const TIME_MS: RangeInclusive<u64> = [\d_]+\.\.=([\d_]+);/);
  assert.ok(grace * 1000 > top, `stop_grace_period ${grace}s must outlast TIME_MS's ${top} ms top`);
});

test("nginx's proxy_read_timeout outlasts the page's longest native wait, TIME_MS's top plus NATIVE_GRACE_MS", () => {
  const timeout = read('deploy/nginx.conf', /proxy_read_timeout (\d+)s;/);
  const top = read('crates/server/src/solve.rs', /const TIME_MS: RangeInclusive<u64> = [\d_]+\.\.=([\d_]+);/);
  const grace = read('web/src/solver-client.ts', /const NATIVE_GRACE_MS = (\d+);/);
  assert.ok(timeout * 1000 > top + grace, `proxy_read_timeout ${timeout}s must outlast ${top} + ${grace} ms`);
});

test("solve.rs's timeout chain doc quotes TIME_MS's top and NATIVE_GRACE_MS", () => {
  const top = read('crates/server/src/solve.rs', /const TIME_MS: RangeInclusive<u64> = [\d_]+\.\.=([\d_]+);/);
  const grace = read('web/src/solver-client.ts', /const NATIVE_GRACE_MS = (\d+);/);
  assert.equal(read('crates/server/src/solve.rs', /must keep (\d+) s \(this range's top\)/) * 1000, top);
  assert.equal(read('crates/server/src/solve.rs', /the page's (\d+) s \+ \d+ s wait/) * 1000, top);
  assert.equal(read('crates/server/src/solve.rs', /the page's \d+ s \+ (\d+) s wait/) * 1000, grace);
  assert.equal(read('crates/server/src/solve.rs', /`time_ms` \+ (\d+) s, /) * 1000, grace);
  assert.equal(read('crates/server/src/solve.rs', /\+ \d+ s, (\d+) s at most/) * 1000, top + grace);
});

test("solve.rs's timeout chain doc quotes compose's stop_grace_period and nginx's proxy_read_timeout", () => {
  const grace = read('compose.yaml', /stop_grace_period: (\d+)s/);
  const timeout = read('deploy/nginx.conf', /proxy_read_timeout (\d+)s;/);
  assert.equal(read('crates/server/src/solve.rs', /< (\d+) s \(compose grace/), grace);
  assert.equal(read('crates/server/src/solve.rs', /`stop_grace_period: (\d+)s`/), grace);
  assert.equal(read('crates/server/src/solve.rs', /< (\d+) s \(nginx\):/), timeout);
  assert.equal(read('crates/server/src/solve.rs', /`proxy_read_timeout (\d+)s`/), timeout);
});

test('the README quotes the ends of TIME_MS and DEFAULT_MS', () => {
  const start = read('crates/server/src/solve.rs', /const TIME_MS: RangeInclusive<u64> = ([\d_]+)\.\.=/);
  const top = read('crates/server/src/solve.rs', /const TIME_MS: RangeInclusive<u64> = [\d_]+\.\.=([\d_]+);/);
  assert.equal(read('README.md', /limits start at (\d+) ms/), start);
  assert.equal(read('README.md', /Native requests cap at (\d+) seconds/) * 1000, top);
  assert.equal(read('README.md', /time_ms: (\d+),/), read('crates/server/src/solve.rs', /const DEFAULT_MS: u64 = ([\d_]+);/));
});

test('the #seconds select offers only budgets within TIME_MS', () => {
  const seconds = items('web/index.html', /<select id="seconds">([\s\S]*?)<\/select>/, /value="(\d+)"/g).map(Number);
  const start = read('crates/server/src/solve.rs', /const TIME_MS: RangeInclusive<u64> = ([\d_]+)\.\.=/);
  const top = read('crates/server/src/solve.rs', /const TIME_MS: RangeInclusive<u64> = [\d_]+\.\.=([\d_]+);/);
  assert.ok(Math.min(...seconds) * 1000 >= start, `#seconds offers less than TIME_MS's ${start} ms start`);
  assert.ok(Math.max(...seconds) * 1000 <= top, `#seconds offers more than TIME_MS's ${top} ms top`);
});

test("the server's HEALTH_TIMEOUT stays under the page's HEALTH_TIMEOUT_MS", () => {
  const server = read('crates/server/src/api.rs', /const HEALTH_TIMEOUT: Duration = Duration::from_millis\((\d+)\);/);
  const page = read('web/src/progress.ts', /const HEALTH_TIMEOUT_MS = (\d+);/);
  assert.ok(server < page, `HEALTH_TIMEOUT ${server} ms must stay under HEALTH_TIMEOUT_MS ${page} ms`);
});

test('progress.ts, solve.rs and the README quote HEALTH_TIMEOUT', () => {
  const timeout = read('crates/server/src/api.rs', /const HEALTH_TIMEOUT: Duration = Duration::from_millis\((\d+)\);/);
  assert.equal(read('web/src/progress.ts', /the server's (\d+) ms HEALTH_TIMEOUT/), timeout);
  assert.equal(read('crates/server/src/solve.rs', /HEALTH_TIMEOUT, (\d+) ms in/), timeout);
  assert.equal(read('README.md', /probe \((\d+) ms deadline\)/), timeout);
});

test("api.rs and solve.rs quote the page's HEALTH_TIMEOUT_MS", () => {
  const timeout = read('web/src/progress.ts', /const HEALTH_TIMEOUT_MS = (\d+);/);
  assert.equal(read('crates/server/src/api.rs', /the page's ([\d.]+) s HEALTH_TIMEOUT_MS/) * 1000, timeout);
  assert.equal(read('crates/server/src/solve.rs', /the page's ([\d.]+) s HEALTH_TIMEOUT_MS/) * 1000, timeout);
});

test('the README quotes HEALTH_TTL', () => {
  assert.equal(
    read('README.md', /reused for (\d+) s/),
    read('crates/server/src/api.rs', /const HEALTH_TTL: Duration = Duration::from_secs\((\d+)\);/),
  );
});

test("the README quotes nginx's request rates and bursts", () => {
  assert.equal(
    read('README.md', /limits `\/api\/` to (\d+)\s+requests per second/),
    read('deploy/nginx.conf', /zone=api_limit:\w+ rate=(\d+)r\/s;/),
  );
  assert.equal(
    read('README.md', /per address \(burst (\d+)\)/),
    read('deploy/nginx.conf', /limit_req zone=api_limit burst=(\d+) nodelay;/),
  );
  assert.equal(read('README.md', /own budget of (\d+) per second/), read('deploy/nginx.conf', /zone=health_limit:\w+ rate=(\d+)r\/s;/));
  assert.equal(
    read('README.md', /per second \(burst (\d+)\) instead/),
    read('deploy/nginx.conf', /limit_req zone=health_limit burst=(\d+) nodelay;/),
  );
});

test("CI's health poll stays within nginx's health_limit rate", () => {
  const rate = read('deploy/nginx.conf', /zone=health_limit:\w+ rate=(\d+)r\/s;/);
  const sleep = read('.github/workflows/ci.yml', /\ssleep (\d+)\s/);
  assert.equal(read('.github/workflows/ci.yml', /nginx's health_limit allows (\d+) requests a second/), rate);
  assert.ok(sleep * rate >= 1, `CI polls /api/health every ${sleep} s, faster than health_limit's ${rate} a second`);
});

test('every 429 text a solve can get names the browser solver, which errorText shows as sent', () => {
  const texts = [
    text('crates/server/src/api.rs', /&self\.solve_slots,\s+"([^"]*)"/),
    text('crates/server/src/solve.rs', /peer,\s+"([^"]*)"/),
    text('deploy/nginx.conf', /"error":"([^"]*)"/),
  ];
  for (const message of texts) assert.ok(message.includes('browser solver'), `"${message}" does not name the browser solver`);
});

test("nginx, transport.ts and the README use the key of api.rs's error body", () => {
  const key = text('crates/server/src/api.rs', /json!\(\{ "(\w+)": self\.message \}\)/);
  assert.equal(text('deploy/nginx.conf', /return 429 '\{"(\w+)":/), key);
  assert.equal(text('web/src/transport.ts', /const error = \(value as ObjectValue\)\.(\w+);/), key);
  assert.equal(text('README.md', /Every API error body is `\{"(\w+)":/), key);
});

test("decodeHealth reads the key of api.rs's health reply", () => {
  assert.equal(
    text('web/src/transport.ts', /\(value as \{ \w+\?: unknown \}\)\.(\w+) === true/),
    text('crates/server/src/api.rs', /"status": "ok", "(\w+)": persistence/),
  );
});

test('the README quotes SAVES_PER_MINUTE', () => {
  assert.equal(read('README.md', /\): (\d+) saves and/), read('crates/server/src/api.rs', /const SAVES_PER_MINUTE: u32 = (\d+);/));
});

test('the README quotes the database timeouts and retry window', () => {
  assert.equal(
    read('README.md', /each gets ([\d.]+) s\)/) * 1000,
    read('crates/server/src/database.rs', /const EXECUTION_TIMEOUT: Duration = Duration::from_millis\((\d+)\);/),
  );
  assert.equal(
    read('README.md', /fail with 503 after (\d+) s/),
    read('crates/server/src/database.rs', /const ACQUIRE_TIMEOUT: Duration = Duration::from_secs\((\d+)\);/),
  );
  assert.equal(
    read('README.md', /stop a statement after ([\d.]+) s/) * 1000,
    read('crates/server/src/database.rs', /const STATEMENT_TIMEOUT: Duration = Duration::from_millis\((\d+)\);/),
  );
  assert.equal(
    read('README.md', /lock wait\s+after (\d+) ms/),
    read('crates/server/src/database.rs', /const LOCK_TIMEOUT: Duration = Duration::from_millis\((\d+)\);/),
  );
  assert.equal(
    read('README.md', /for about (\d+) seconds; authentication/),
    read('crates/server/src/database.rs', /const RETRY_WINDOW: Duration = Duration::from_secs\((\d+)\);/),
  );
});

test('the #memory select offers only budgets within MEMORY_MIB, up to its cap', () => {
  const memory = items('web/index.html', /<select id="memory">([\s\S]*?)<\/select>/, /value="(\d+)"/g).map(Number);
  const cap = read('crates/server/src/solve.rs', /const MEMORY_MIB: RangeInclusive<usize> = \*MEMORY_MIB_RANGE\.start\(\)\.\.=(\d+);/);
  const floor = read('crates/search/src/lib.rs', /pub const MEMORY_MIB_RANGE: RangeInclusive<usize> = (\d+)\.\.=\d+;/);
  assert.equal(Math.max(...memory), cap);
  assert.ok(Math.min(...memory) >= floor, `#memory offers less than MEMORY_MIB_RANGE's ${floor} MiB start`);
});

test('the README, .env.example and protocol.ts quote the cap of MEMORY_MIB', () => {
  const cap = read('crates/server/src/solve.rs', /const MEMORY_MIB: RangeInclusive<usize> = \*MEMORY_MIB_RANGE\.start\(\)\.\.=(\d+);/);
  assert.equal(read('README.md', /(\d+) MiB accounted search storage, and/), cap);
  assert.equal(read('README.md', /memory_mib: (\d+)\}/), cap);
  assert.equal(read('README.md', /its own arena of up to (\d+) MiB/), cap);
  assert.equal(read('.env.example', /reserves up to (\d+) MiB of search storage/), cap);
  assert.equal(read('web/src/protocol.ts', /(\d+) MiB, the largest budget the #memory select offers/), cap);
});

test("the README and .env.example quote MEMORY_MIB's cap times MAX_SOLVE_CONCURRENCY", () => {
  const cap = read('crates/server/src/solve.rs', /const MEMORY_MIB: RangeInclusive<usize> = \*MEMORY_MIB_RANGE\.start\(\)\.\.=(\d+);/);
  const slots = read('crates/server/src/config.rs', /pub const MAX_SOLVE_CONCURRENCY: u32 = (\d+);/);
  assert.equal(read('README.md', /GiB at (\d+)\./), slots);
  assert.equal(read('README.md', /so up to (\d+) GiB at \d+\./) * 1024, cap * slots);
  assert.equal(read('.env.example', /so the (\d+)-slot maximum/), slots);
  assert.equal(read('.env.example', /reserves up\s+#\s+to (\d+) GiB/) * 1024, cap * slots);
});

test("the README and vite's dev proxy quote config.rs's BIND_ADDR default", () => {
  const address = text('crates/server/src/config.rs', /\.unwrap_or_else\(\|\| "([\d.:]+)"\.into\(\)\);/);
  assert.equal(text('README.md', /\| `BIND_ADDR` \| `([\d.:]+)` \|/), address);
  assert.equal(text('web/vite.config.ts', /'\/api': 'http:\/\/([\d.:]+)'/), address);
});

test("the README quotes the Dockerfile's BIND_ADDR", () => {
  assert.equal(text('README.md', /the Docker image sets `([\d.:]+)`/), text('Dockerfile', /ENV BIND_ADDR=([\d.:]+)/));
});

test("the README's Configuration table quotes config.rs's DATABASE_* defaults", () => {
  assert.equal(
    text('README.md', /\| `DATABASE_HOST` \| `(\w+)` \|/),
    text('crates/server/src/config.rs', /part\("DATABASE_HOST", "(\w+)"\)/),
  );
  assert.equal(
    text('README.md', /\| `DATABASE_PORT` \| `(\d+)` \|/),
    text('crates/server/src/config.rs', /part\("DATABASE_PORT", "(\d+)"\)/),
  );
  assert.equal(
    text('README.md', /\| `DATABASE_USER` \| `(\w+)` \|/),
    text('crates/server/src/config.rs', /part\("DATABASE_USER", "(\w+)"\)/),
  );
  assert.equal(
    text('README.md', /\| `DATABASE_NAME` \| `(\w+)` \|/),
    text('crates/server/src/config.rs', /part\("DATABASE_NAME", "(\w+)"\)/),
  );
});

test("config.rs's DATABASE_* defaults name compose's db service, user and database", () => {
  assert.equal(
    text('crates/server/src/config.rs', /part\("DATABASE_HOST", "(\w+)"\)/),
    text('compose.yaml', /services:\s+(\w+):\s+image: postgres:/),
  );
  assert.equal(text('crates/server/src/config.rs', /part\("DATABASE_USER", "(\w+)"\)/), text('compose.yaml', /POSTGRES_USER: (\w+)/));
  assert.equal(text('crates/server/src/config.rs', /part\("DATABASE_NAME", "(\w+)"\)/), text('compose.yaml', /POSTGRES_DB: (\w+)/));
});

test("the README's SOLVE_CONCURRENCY row quotes config.rs's default and range", () => {
  assert.equal(
    read('README.md', /\| `SOLVE_CONCURRENCY` \| `(\d+)` \|/),
    read('crates/server/src/config.rs', /"SOLVE_CONCURRENCY", \d+\.\.=MAX_SOLVE_CONCURRENCY, (\d+)\)/),
  );
  assert.equal(
    read('README.md', /Concurrent native solves, (\d+)\.\.\d+;/),
    read('crates/server/src/config.rs', /"SOLVE_CONCURRENCY", (\d+)\.\.=MAX_SOLVE_CONCURRENCY/),
  );
  assert.equal(
    read('README.md', /Concurrent native solves, \d+\.\.(\d+);/),
    read('crates/server/src/config.rs', /pub const MAX_SOLVE_CONCURRENCY: u32 = (\d+);/),
  );
});

test("the README's SOLVE_RATE_PER_MINUTE row quotes config.rs's default and range", () => {
  assert.equal(
    read('README.md', /\| `SOLVE_RATE_PER_MINUTE` \| `(\d+)` \|/),
    read('crates/server/src/config.rs', /"SOLVE_RATE_PER_MINUTE", \d+\.\.=\d+, (\d+)\)/),
  );
  assert.equal(
    read('README.md', /per minute, (\d+)\.\.\d+ \|/),
    read('crates/server/src/config.rs', /"SOLVE_RATE_PER_MINUTE", (\d+)\.\.=/),
  );
  assert.equal(
    read('README.md', /per minute, \d+\.\.(\d+) \|/),
    read('crates/server/src/config.rs', /"SOLVE_RATE_PER_MINUTE", \d+\.\.=(\d+),/),
  );
});

test("the README's PROGRESS_CONCURRENCY row quotes config.rs's default and range", () => {
  assert.equal(
    read('README.md', /\| `PROGRESS_CONCURRENCY` \| `(\d+)` \|/),
    read('crates/server/src/config.rs', /"PROGRESS_CONCURRENCY", \d+\.\.=MAX_PROGRESS_CONCURRENCY, (\d+)\)/),
  );
  assert.equal(
    read('README.md', /progress reads and saves, (\d+)\.\.\d+;/),
    read('crates/server/src/config.rs', /"PROGRESS_CONCURRENCY", (\d+)\.\.=MAX_PROGRESS_CONCURRENCY/),
  );
  assert.equal(
    read('README.md', /progress reads and saves, \d+\.\.(\d+);/),
    read('crates/server/src/config.rs', /const MAX_PROGRESS_CONCURRENCY: u32 = (\d+);/),
  );
});

test("the README's DB_POOL_SIZE row quotes the range of config.rs's pool_size", () => {
  const progress = read('crates/server/src/config.rs', /const MAX_PROGRESS_CONCURRENCY: u32 = (\d+);/);
  const spare = read('crates/server/src/config.rs', /let range = \d+\.\.=MAX_PROGRESS_CONCURRENCY \+ (\d+);/);
  assert.equal(
    read('README.md', /connections, (\d+)\.\.\d+:/),
    read('crates/server/src/config.rs', /let range = (\d+)\.\.=MAX_PROGRESS_CONCURRENCY \+ \d+;/),
  );
  assert.equal(read('README.md', /connections, \d+\.\.(\d+):/), progress + spare);
});

test("the README's PROGRESS_RETENTION_DAYS row quotes config.rs's default and range", () => {
  assert.equal(
    read('README.md', /\| `PROGRESS_RETENTION_DAYS` \| `(\d+)` \|/),
    read('crates/server/src/config.rs', /"PROGRESS_RETENTION_DAYS", \d+\.\.=MAX_RETENTION_DAYS, (\d+)\)/),
  );
  assert.equal(
    read('README.md', /\| (\d+) keeps progress forever/),
    read('crates/server/src/config.rs', /"PROGRESS_RETENTION_DAYS", (\d+)\.\.=MAX_RETENTION_DAYS/),
  );
  assert.equal(
    read('README.md', /forever; \d+\.\.(\d+) deletes/),
    read('crates/server/src/config.rs', /const MAX_RETENTION_DAYS: i32 = ([\d_]+);/),
  );
});

test("the README's PROGRESS_RETENTION_BATCH_SIZE row quotes config.rs's default and range", () => {
  assert.equal(
    read('README.md', /\| `PROGRESS_RETENTION_BATCH_SIZE` \| `(\d+)` \|/),
    read('crates/server/src/config.rs', /"PROGRESS_RETENTION_BATCH_SIZE", \d+\.\.=\d+, (\d+)\)/),
  );
  assert.equal(
    read('README.md', /one short transaction each, (\d+)\.\.\d+;/),
    read('crates/server/src/config.rs', /"PROGRESS_RETENTION_BATCH_SIZE", (\d+)\.\.=/),
  );
  assert.equal(
    read('README.md', /one short transaction each, \d+\.\.(\d+);/),
    read('crates/server/src/config.rs', /"PROGRESS_RETENTION_BATCH_SIZE", \d+\.\.=(\d+),/),
  );
});

test('the README and .env.example quote RETENTION_MAX_BATCHES and its products with the batch sizes', () => {
  const batches = read('crates/server/src/database.rs', /pub const RETENTION_MAX_BATCHES: usize = (\d+);/);
  const size = read('crates/server/src/config.rs', /"PROGRESS_RETENTION_BATCH_SIZE", \d+\.\.=\d+, (\d+)\)/);
  const top = read('crates/server/src/config.rs', /"PROGRESS_RETENTION_BATCH_SIZE", \d+\.\.=(\d+),/);
  assert.equal(read('README.md', /runs at most (\d+) batches/), batches);
  assert.equal(read('README.md', /deletes at most (\d+) times this many records per sweep/), batches);
  assert.equal(read('README.md', /\(([\d,]+) at the default/), batches * size);
  assert.equal(read('README.md', /at the default, ([\d,]+) at \d+\)/), batches * top);
  assert.equal(read('README.md', /at the default, [\d,]+ at (\d+)\)/), top);
  assert.equal(read('README.md', /runs all (\d+) full/), batches);
  assert.equal(read('.env.example', /runs at most (\d+) batches per hourly sweep/), batches);
  assert.equal(read('.env.example', /deletes at most\s+#\s+(\d+) times this many/), batches);
});

test("nginx, .env.example and the README quote compose's default TRUSTED_SUBNET", () => {
  const subnet = text('compose.yaml', /\$\{TRUSTED_SUBNET:-([\d.\/]+)\}/);
  assert.equal(text('deploy/nginx.conf', /# set_real_ip_from ([\d.\/]+);/), subnet);
  assert.equal(text('.env.example', /Commented or empty, it is ([\d.\/]+);/), subnet);
  assert.equal(text('README.md', /pins its network to `([\d.\/]+)`/), subnet);
});

test("CI's database service and the README use compose's PostgreSQL image", () => {
  assert.equal(text('.github/workflows/ci.yml', /image: (postgres:[\w.-]+)/), text('compose.yaml', /image: (postgres:[\w.-]+)/));
  assert.equal(read('README.md', /with a PostgreSQL (\d+)\s+service/), read('compose.yaml', /image: postgres:(\d+)/));
});

test('the web image exposes the port nginx listens on', () => {
  assert.equal(read('Dockerfile', /AS web\s[\s\S]*?EXPOSE (\d+)/), read('deploy/nginx.conf', /listen (\d+);/));
});

test("the README quotes the Dockerfile's NGINX tag", () => {
  assert.equal(text('README.md', /pins NGINX ([\d.]+)\./), text('Dockerfile', /FROM nginx:([\d.]+)-alpine AS web/));
});

test("the README and the Dockerfile quote the NGINX version nginx.conf's resolve needs", () => {
  const version = text('deploy/nginx.conf', /"resolve" needs nginx\s+#\s+([\d.]+) or later/);
  assert.equal(text('README.md', /`resolve` needs NGINX ([\d.]+) or later/), version);
  assert.equal(text('Dockerfile', /needs ([\d.]+)\+ \("resolve"\)/), version);
});

test("the README quotes nginx's resolver valid interval", () => {
  const valid = read('deploy/nginx.conf', /resolver [\d.]+ valid=(\d+)s/);
  assert.equal(read('README.md', /about every (\d+)\s+seconds/), valid);
  assert.equal(read('README.md', /within about (\d+) seconds/), valid);
});

test("the README quotes compose's published web port", () => {
  const port = read('compose.yaml', /- "127\.0\.0\.1:(\d+):\d+"/);
  assert.equal(read('README.md', /Open `http:\/\/127\.0\.0\.1:(\d+)`/), port);
  assert.equal(read('README.md', /NGINX at\s+`127\.0\.0\.1:(\d+)`/), port);
});

test("the page's API paths are main.rs's routes", () => {
  const progress = text('crates/server/src/main.rs', /"([\w\/]+)\{id\}",\s+get\(progress::get\)/);
  assert.equal(
    text('web/src/progress.ts', /this\.request\('([\w\/]+)'/),
    text('crates/server/src/main.rs', /\.route\("([\w\/]+)", get\(api::health\)\)/),
  );
  assert.equal(
    text('web/src/solver-client.ts', /this\.request\('([\w\/]+)'/),
    text('crates/server/src/main.rs', /\.route\("([\w\/]+)", post\(solve::solve\)\)/),
  );
  assert.equal(text('web/src/progress.ts', /request\(`([\w\/]+)\$\{[^`]*`, \{\s+method/), progress);
  assert.equal(text('web/src/progress.ts', /request\(`([\w\/]+)\$\{[^`]*`, \{\s+headers/), progress);
});

test("nginx's exact location is main.rs's health route", () => {
  assert.equal(
    text('deploy/nginx.conf', /location = ([\w\/]+) \{/),
    text('crates/server/src/main.rs', /\.route\("([\w\/]+)", get\(api::health\)\)/),
  );
});

test('progress.ts, the README and CI send the profile header progress.rs reads', () => {
  const header = text('crates/server/src/progress.rs', /\.get\("([\w-]+)"\)/);
  assert.equal(text('web/src/progress.ts', /'content-type': 'application\/json', '([\w-]+)': this\.profile!/), header);
  assert.equal(text('web/src/progress.ts', /headers: \{ '([\w-]+)': this\.profile! \}/), header);
  assert.equal(text('README.md', /endpoints require `([\w-]+)`/), header);
  assert.equal(text('.github/workflows/ci.yml', /-H "([\w-]+): \$profile" \\/), header);
});

test("the migration, storage.ts, the README and CI agree with progress.rs's profile token length", () => {
  const length = read('crates/server/src/progress.rs', /if value\.len\(\) != (\d+) \|\|/);
  assert.equal(read('migrations/0002_progress_fingerprint.sql', /profile ~ '\^\[0-9a-fA-F\]\{(\d+)\}\$'/), length);
  assert.equal(read('web/src/storage.ts', /\/\^\[a-f0-9\]\{(\d+)\}\$\/\.test/), length);
  assert.equal(read('web/src/storage.ts', /new Uint8Array\((\d+)\)/) * 2, length);
  assert.equal(read('README.md', /random (\d+)-hex token/), length);
  assert.equal(text('.github/workflows/ci.yml', /\sprofile=([0-9a-f]+)\s/).length, length);
});

test("CI's smoke solve sends solve_body, the router tests' request", () => {
  assert.deepEqual(
    JSON.parse(text('.github/workflows/ci.yml', /-d '(\{"rows"[^']*)'/)),
    JSON.parse(text('crates/server/src/tests/mod.rs', /json!\((\{"rows"[\s\S]*?\})\)/)),
  );
});

test("CI's smoke rows are the catalog's rows for the puzzle CI saves progress for", () => {
  const id = text('.github/workflows/ci.yml', /-d '\{"route":"D"\}' "\$base\/api\/progress\/([\w-]+)"/);
  const smoke = JSON.parse(text('.github/workflows/ci.yml', /-d '(\{"rows"[^']*)'/));
  const puzzles = /** @type {{ id: string, rows: string[] }[]} */ (JSON.parse(source('data/puzzles.json')));
  assert.deepEqual(smoke.rows, puzzles.find(puzzle => puzzle.id === id)?.rows);
});

test("the README's catalog size is data/puzzles.json's length", () => {
  assert.equal(read('README.md', /its (\d+) catalog puzzles/), JSON.parse(source('data/puzzles.json')).length);
});

test("CI's app shell check greps for the start of index.html's title", () => {
  const title = text('web/index.html', /<title>([^<]*)<\/title>/);
  const grep = text('.github/workflows/ci.yml', /grep -q '<title>([^']*)'/);
  assert.ok(title.startsWith(grep), `CI greps for "${grep}", but index.html's title is "${title}"`);
});

test("encodeSolveRequest sends exactly the fields of solve.rs's Request", () => {
  const sent = items('web/src/transport.ts', /JSON\.stringify\(\{([\s\S]*?)\}\);/, /(\w+): request\./g);
  const fields = items('crates/server/src/solve.rs', /pub struct Request \{([\s\S]*?)\r?\n\}/, /(\w+): [\w<>]+,/g);
  assert.deepEqual(sent.toSorted(), fields.toSorted());
});

test("the worker's and the catalog example's pop batches equal solve.rs's POPS_PER_CHECK", () => {
  const pops = read('crates/server/src/solve.rs', /const POPS_PER_CHECK: u32 = (\d+);/);
  assert.equal(read('web/src/solver.worker.ts', /const POPS_PER_ADVANCE = (\d+);/), pops);
  assert.equal(read('crates/search/examples/catalog.rs', /const POPS_PER_CHECK: u32 = (\d+);/), pops);
});

test('the README quotes MAX_SAVED_ROUTE', () => {
  assert.equal(
    read('README.md', /Saved routes are capped at ([\d,]+) moves/),
    read('crates/server/src/progress.rs', /const MAX_SAVED_ROUTE: usize = ([\d_]+);/),
  );
});

test("package.json's engines, validate.mjs and the README quote .node-version's Node", () => {
  const version = text('.node-version', /^(\d+\.\d+\.\d+)/);
  assert.equal(text('package.json', /"node": ">=([\d.]+)"/), version);
  assert.equal(text('scripts/validate.mjs', /package\.json's engines \(>=([\d.]+)\)/), version);
  assert.equal(text('README.md', /that CI and the Docker image use \(([\d.]+)\)/), version);
  assert.equal(text('README.md', /Requires Node ([\d.]+)\+/), version.replace(/\.0$/, ''));
});

test("the README quotes rust-toolchain.toml's channel", () => {
  const channel = text('rust-toolchain.toml', /channel = "([\d.]+)"/);
  assert.equal(text('README.md', /Rust ([\d.]+), and a native linker/), channel);
  assert.equal(text('README.md', /is the Rust ([\d.]+) image/), channel);
});

test("the README quotes Cargo.lock's wasm-bindgen version", () => {
  const version = text('Cargo.lock', /name = "wasm-bindgen"\s+version = "([\d.]+)"/);
  assert.equal(text('README.md', /wasm-bindgen-cli --version ([\d.]+)/), version);
  assert.equal(text('README.md', /`Cargo\.lock` resolves \(([\d.]+) above\)/), version);
});

test("the README quotes the release-test profile's codegen-units", () => {
  assert.equal(read('README.md', /uses (\d+) codegen units/), read('Cargo.toml', /\[profile\.release-test\][^\[]*codegen-units = (\d+)/));
});

test("the README quotes dependabot.yml's cooldown", () => {
  const days = read('README.md', /after a (\d+)-day\s+cooldown/);
  const cooldowns = [...source('.github/dependabot.yml').matchAll(/default-days: (\d+)/g)].map(match => Number(match[1]));
  assert.ok(cooldowns.length > 0, '.github/dependabot.yml no longer sets default-days');
  for (const cooldown of cooldowns) assert.equal(cooldown, days);
});

test("the README quotes ci.yml's weekly cron", () => {
  const [minute, hour] = text('.github/workflows/ci.yml', /cron: '(\d+ \d+) \* \* 1'/).split(' ');
  assert.equal(text('README.md', /Mondays at (\d\d:\d\d) UTC/), `${hour.padStart(2, '0')}:${minute.padStart(2, '0')}`);
});

test("the puzzle textarea's maxlength is Board::parse's text cap, MAX_CELLS * 2 bytes", () => {
  assert.equal(
    read('web/index.html', /id="rows"[^>]*maxlength="(\d+)"/),
    2 * read('crates/core/src/board.rs', /pub const MAX_CELLS: usize = ([\d_]+);/),
  );
});

test("protocol.ts's MAX_ROUTE equals sokomind_core::MAX_ROUTE", () => {
  assert.equal(
    read('web/src/protocol.ts', /export const MAX_ROUTE = ([\d_]+);/),
    read('crates/core/src/game.rs', /pub const MAX_ROUTE: usize = ([\d_]+);/),
  );
});

test('the migration and the README quote MAX_ROUTE', () => {
  const max = read('crates/core/src/game.rs', /pub const MAX_ROUTE: usize = ([\d_]+);/);
  assert.equal(read('migrations/0002_progress_fingerprint.sql', /length\(route\) <= (\d+)/), max);
  assert.equal(read('README.md', /Routes\s+are limited to ([\d,]+) moves/), max);
  assert.equal(read('README.md', /route over the ([\d,]+)-move replay limit/), max);
  assert.equal(read('README.md', /stop at the ([\d,]+)-move replay limit/), max);
});

test('the README quotes MAX_CELLS and MAX_BOXES', () => {
  const cells = read('crates/core/src/board.rs', /pub const MAX_CELLS: usize = ([\d_]+);/);
  const boxes = read('crates/core/src/board.rs', /pub const MAX_BOXES: usize = (\d+);/);
  assert.equal(read('README.md', /Boards are limited to (\d+) cells/), cells);
  assert.equal(read('README.md', /boards cap at (\d+) cells/), cells);
  assert.equal(read('README.md', /cells and (\d+) boxes \(all/), boxes);
  assert.equal(read('README.md', /cells and (\d+) boxes\s+\(the reference/), boxes);
});

test("protocol.ts's PAST_LIMIT_MESSAGE is the text of ReplayError::PastLimit", () => {
  assert.equal(
    text('web/src/protocol.ts', /export const PAST_LIMIT_MESSAGE = `([^`]*)`;/).replaceAll('${', '{'),
    text('crates/core/src/game.rs', /Self::PastLimit => write!\(\s+f,\s+"([^"]*)"/),
  );
});

test('protocol.ts and the README quote sokomind_search::MAX_STATES', () => {
  const max = read('crates/search/src/lib.rs', /pub const MAX_STATES: usize = ([\d_]+);/);
  assert.equal(read('web/src/protocol.ts', /export const MAX_STATES = ([\d_]+);/), max);
  assert.equal(read('README.md', /([\d,]+) states\s+per arena/), max);
  assert.equal(read('README.md', /max_states: (\d+),/), max);
});

test("the README and protocol.ts quote the ends of the search crate's limit ranges", () => {
  const floor = read('crates/search/src/lib.rs', /pub const MEMORY_MIB_RANGE: RangeInclusive<usize> = (\d+)\.\.=\d+;/);
  const top = read('crates/search/src/lib.rs', /pub const MEMORY_MIB_RANGE: RangeInclusive<usize> = \d+\.\.=(\d+);/);
  const states = read('crates/search/src/lib.rs', /pub const MAX_STATES_RANGE: RangeInclusive<usize> = (\d+)\.\.=MAX_STATES;/);
  assert.equal(read('README.md', /accepts budgets up to (\d+) MiB/), top);
  assert.equal(read('web/src/protocol.ts', /and (\d+) MiB, the largest the search crate accepts/), top);
  assert.equal(read('README.md', /state, and (\d+) MiB and cap/), floor);
  assert.equal(read('README.md', /ms, (\d+) state, and/), states);
});

test("the README quotes the #memory select's range and default", () => {
  const memory = items('web/index.html', /<select id="memory">([\s\S]*?)<\/select>/, /value="(\d+)"/g).map(Number);
  assert.equal(read('README.md', /memory select names \((\d+) to/), Math.min(...memory));
  assert.equal(read('README.md', /\(\d+ to\s+(\d+) MiB, \d+ by default\)/), Math.max(...memory));
  assert.equal(read('README.md', /MiB, (\d+) by default\)/), read('web/index.html', /<option value="(\d+)" selected>/));
});

test("protocol.ts, transport.ts and the migration use sokomind_core::ACTIONS's letters", () => {
  const actions = text('crates/core/src/lib.rs', /pub const ACTIONS: &\[u8; 4\] = b"([A-Z]+)";/);
  const route = text('web/src/transport.ts', /!\/\^\[([A-Z]+)\]\*\$\/\.test\(value\)/);
  const check = text('migrations/0002_progress_fingerprint.sql', /route !~ '\[\^([A-Z]+)\]'/);
  assert.equal(text('web/src/protocol.ts', /export const ACTIONS = '([A-Z]+)';/), actions);
  assert.equal([...route].toSorted().join(''), [...actions].toSorted().join(''));
  assert.equal([...check].toSorted().join(''), [...actions].toSorted().join(''));
});

test("the direction pad's data-direction values index ACTIONS by each button's label", () => {
  const actions = text('web/src/protocol.ts', /export const ACTIONS = '([A-Z]+)';/);
  assert.equal(actions[read('web/index.html', /data-direction="(\d)" aria-label="Move up"/)], 'U');
  assert.equal(actions[read('web/index.html', /data-direction="(\d)" aria-label="Move down"/)], 'D');
  assert.equal(actions[read('web/index.html', /data-direction="(\d)" aria-label="Move left"/)], 'L');
  assert.equal(actions[read('web/index.html', /data-direction="(\d)" aria-label="Move right"/)], 'R');
});

test("the migration and the README match the format of board.rs's fingerprint", () => {
  const prefix = text('crates/core/src/board.rs', /format!\("([\w-]+):\{hash:\d+x\}"\)/);
  const digits = read('crates/core/src/board.rs', /\{hash:0(\d+)x\}/);
  assert.equal(text('migrations/0002_progress_fingerprint.sql', /fingerprint ~ '\^([\w-]+):/), prefix);
  assert.equal(read('migrations/0002_progress_fingerprint.sql', /fingerprint ~ '\^[\w-]+:\[0-9a-f\]\{(\d+)\}\$'/), digits);
  assert.equal(text('README.md', /\(`([\w-]+):\{fnv1a\}`/), prefix);
});

test("protocol.ts's MODES, the #mode select and the README name the modes of Mode::as_str", () => {
  const modes = items('crates/search/src/lib.rs', /match self \{\s*(Self::Fast =>[\s\S]*?)\}/, /=> "(\w+)"/g).toSorted();
  assert.deepEqual(items('web/src/protocol.ts', /export const MODES = \[([^\]]*)\] as const;/, /'(\w+)'/g).toSorted(), modes);
  assert.deepEqual(items('web/index.html', /<select id="mode">([\s\S]*?)<\/select>/, /value="(\w+)"/g).toSorted(), modes);
  assert.deepEqual(items('README.md', /mode: ((?:"\w+"\|?)+)/, /"(\w+)"/g).toSorted(), modes);
});

test("protocol.ts's STATUSES are the names of Status::as_str", () => {
  assert.deepEqual(
    items('web/src/protocol.ts', /export const STATUSES = \[([^\]]*)\] as const;/, /'(\w+)'/g).toSorted(),
    items('crates/search/src/lib.rs', /match self \{\s*(Self::Running =>[\s\S]*?)\}/, /=> "(\w+)"/g).toSorted(),
  );
});

test('decodeNativeReply handles exactly the names of Proof::kind', () => {
  assert.deepEqual(
    items('web/src/transport.ts', /switch \(native\.kind\) \{([\s\S]*?)default:/, /case '(\w+)':/g).toSorted(),
    items('crates/search/src/proof.rs', /pub fn kind\(self\) -> &'static str \{([\s\S]*?)\r?\n    \}/, /=> "(\w+)"/g).toSorted(),
  );
});

test("the #engine select offers solver-client.ts's ENGINES", () => {
  assert.deepEqual(
    items('web/index.html', /<select id="engine">([\s\S]*?)<\/select>/, /value="(\w+)"/g).toSorted(),
    items('web/src/solver-client.ts', /export const ENGINES = \[([^\]]*)\] as const;/, /'(\w+)'/g).toSorted(),
  );
});

test("board.ts's WALL equals sokomind_core::WALL", () => {
  assert.equal(read('web/src/board.ts', /const WALL = (\d+);/), read('crates/core/src/board.rs', /pub const WALL: u8 = (\d+);/));
});

test("the README, Mode's docs and the arena's key test quote the weights of Policy::FAST and Policy::QUALITY", () => {
  const fast = read('crates/search/src/engine.rs', /const FAST: Self = Self \{\s+weight: (\d+),/);
  const quality = read('crates/search/src/engine.rs', /const QUALITY: Self = Self \{\s+weight: (\d+),/);
  assert.equal(read('README.md', /weighted A\* \(`g \+ (\d+)h`\)/), fast);
  assert.equal(read('crates/search/src/lib.rs', /Weighted A\* \(weight (\d+)\)/), fast);
  assert.equal(read('README.md', /at `g \+ (\d+)h` with reopenings/), quality);
  assert.equal(read('README.md', /starts over at\s+`g \+ (\d+)h`/), quality);
  assert.equal(read('crates/search/src/lib.rs', /then weight (\d+) in the same arena/), quality);
  assert.equal(read('crates/search/src/lib.rs', /starts over at weight (\d+)\./), quality);
  assert.equal(read('crates/search/src/arena.rs', /let worst = MAX_ROUTE as u64 \+ (\d+) \* u64::from\(MAX_QUEUED_H\);/), fast);
});

test("the README quotes heuristic.rs's REPAIR_CROSSOVER", () => {
  assert.equal(
    read('README.md', /for groups\s+of (\d+) or more/),
    read('crates/search/src/heuristic.rs', /const REPAIR_CROSSOVER: usize = (\d+);/),
  );
});

test("the README quotes arena.rs's ID_BITS and its record and queue entry sizes", () => {
  const record = read('crates/search/src/arena.rs', /size_of::<Record>\(\) == (\d+)/);
  const entry = read('crates/search/src/arena.rs', /size_of::<Entry>\(\) == (\d+)/);
  assert.equal(read('README.md', /and a (\d+)-bit arena id/), read('crates/search/src/arena.rs', /const ID_BITS: u32 = (\d+);/));
  assert.equal(read('README.md', /arena record is (\d+) bytes/), record);
  assert.equal(read('README.md', /costs a (\d+)-byte record/), record);
  assert.equal(read('README.md', /queue entry one (\d+)-byte/), entry);
  assert.equal(read('README.md', /an (\d+)-byte queue entry/), entry);
});

test("the README, reserved_bytes' doc, parity.mjs and the arena's own comments quote INITIAL_TABLE in slots and KiB", () => {
  const shift = read('crates/search/src/arena.rs', /const INITIAL_TABLE: usize = if cfg!\(test\) \{ \d+ \} else \{ 1 << (\d+) \};/);
  // Each slot is a u32 node id.
  const kib = (2 ** shift * 4) / 1024;
  assert.equal(read('README.md', /it starts at 2\^(\d+) slots/), shift);
  assert.equal(read('README.md', /slots \((\d+) KiB\)/), kib);
  assert.equal(read('README.md', /up to (\d+) KiB past the budget/), kib);
  assert.equal(read('crates/search/src/lib.rs', /at most 2\^(\d+) slots/), shift);
  assert.equal(read('crates/search/src/lib.rs', /\((\d+) KiB\) at first/), kib);
  assert.equal(read('crates/search/src/lib.rs', /freed first table,\s+\/\/\/ (\d+) KiB/), kib);
  assert.equal(read('crates/search/src/arena.rs', /smaller: 2\^(\d+) slots/), shift);
  assert.equal(read('crates/search/src/arena.rs', /slots, (\d+) KiB, so/), kib);
  assert.equal(read('crates/search/src/arena.rs', /INITIAL_TABLE slots \((\d+) KiB\)/), kib);
  assert.equal(read('scripts/parity.mjs', /at most (\d+) KiB, which/), kib);
});

test("the README and parity.mjs's comment quote the memory-fill case's allowance past reserved_bytes", () => {
  const allowance = read('scripts/parity.mjs', /const fillCap = fill\.metrics\.reservedBytes \+ (\d+) \* MIB;/);
  assert.equal(read('README.md', /memory ceiling plus (\d+) MiB/), allowance);
  assert.equal(read('scripts/parity.mjs', /most reserved_bytes plus (\d+) MiB/), allowance);
});

test("the README quotes the benchmark baseline's, catalog example's and observe run's limits", () => {
  assert.equal(read('README.md', /baseline's ([\d,]+) states/), read('benchmarks/catalog-baseline.json', /"maxStates": (\d+),/));
  assert.equal(
    read('README.md', /baseline's [\d,]+ states and (\d+) MiB/),
    read('benchmarks/catalog-baseline.json', /"memoryMiB": (\d+),/),
  );
  assert.equal(read('README.md', /defaults of ([\d,]+) states/), read('crates/search/examples/catalog.rs', /states: ([\d_]+),/));
  assert.equal(read('README.md', /defaults of [\d,]+ states and (\d+) MiB/), read('crates/search/examples/catalog.rs', /memory: (\d+),/));
  assert.equal(read('README.md', /at a fixed ([\d,]+)\s+states/), read('scripts/benchmark.mjs', /const OBSERVE = \{ maxStates: ([\d_]+),/));
  assert.equal(
    read('README.md', /fixed [\d,]+\s+states and (\d+) MiB/),
    read('scripts/benchmark.mjs', /const OBSERVE = \{ maxStates: [\d_]+, memoryMiB: (\d+),/),
  );
});

test("the README's fixture count is search.rs's fixtures", () => {
  const fixtures = items('crates/search/tests/search.rs', /fixtures! \{([\s\S]*?)\r?\n    \}/, /^ {8}(\w+): "/gm);
  assert.equal(read('README.md', /(\d+) boards whose independent/), fixtures.length);
});

test("the README quotes ProgressClient's probe backoff", () => {
  assert.equal(
    read('README.md', /asks again after (\d+) s and doubles/) * 1000,
    read('web/src/progress.ts', /const PROBE_FIRST_MS = (\d+),/),
  );
  assert.equal(read('README.md', /wait up to (\d+) minutes/) * 60000, read('web/src/progress.ts', /PROBE_MAX_MS = ([\d_]+);/));
});
