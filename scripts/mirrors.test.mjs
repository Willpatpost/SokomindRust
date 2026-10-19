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

/**
 * The values of the number words the README spells limits with.
 * @type {Record<string, number>}
 */
const WORDS = { two: 2, three: 3, six: 6, nine: 9 };

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
    text('crates/server/src/api.rs', /fn solve_permit[\s\S]*?Error::too_many\("([^"]*)"\)/),
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

test("decodeHealth reads api.rs's health reply, which CI's smoke test polls for", () => {
  const [key, value] = [/json!\(\{ "(\w+)": "\w+" \}\)/, /json!\(\{ "\w+": "(\w+)" \}\)/].map(pattern =>
    text('crates/server/src/api.rs', pattern),
  );
  assert.equal(text('web/src/transport.ts', /\(value as \{ (\w+)\?: unknown \}\)\.\w+ === '\w+'/), key);
  assert.equal(text('web/src/transport.ts', /\(value as \{ \w+\?: unknown \}\)\.\w+ === '(\w+)'/), value);
  assert.equal(text('.github/workflows/ci.yml', /jq -e '\.(\w+) == "\w+"'/), key);
  assert.equal(text('.github/workflows/ci.yml', /jq -e '\.\w+ == "(\w+)"'/), value);
});

test('the #memory select offers only budgets within MEMORY_MIB, up to its cap', () => {
  const memory = items('web/index.html', /<select id="memory">([\s\S]*?)<\/select>/, /value="(\d+)"/g).map(Number);
  const cap = read('crates/server/src/solve.rs', /const MEMORY_MIB: RangeInclusive<usize> = \*MEMORY_MIB_RANGE\.start\(\)\.\.=(\d+);/);
  const floor = read('crates/search/src/lib.rs', /pub const MEMORY_MIB_RANGE: RangeInclusive<usize> = (\d+)\.\.=MAX_MEMORY_MIB;/);
  assert.equal(Math.max(...memory), cap);
  assert.ok(Math.min(...memory) >= floor, `#memory offers less than MEMORY_MIB_RANGE's ${floor} MiB start`);
});

test("the README, .env.example and protocol.ts quote MEMORY_MIB's cap, and the README its default", () => {
  const cap = read('crates/server/src/solve.rs', /const MEMORY_MIB: RangeInclusive<usize> = \*MEMORY_MIB_RANGE\.start\(\)\.\.=(\d+);/);
  assert.equal(read('README.md', /(\d+) MiB accounted search storage, and/), cap);
  assert.equal(read('README.md', /memory_mib: (\d+)\}/), read('crates/server/src/solve.rs', /const DEFAULT_MEMORY_MIB: usize = (\d+);/));
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

test("nginx, .env.example and the README quote compose's default TRUSTED_SUBNET", () => {
  const subnet = text('compose.yaml', /\$\{TRUSTED_SUBNET:-([\d.\/]+)\}/);
  assert.equal(text('deploy/nginx.conf', /# set_real_ip_from ([\d.\/]+);/), subnet);
  assert.equal(text('.env.example', /Commented or empty, it is ([\d.\/]+);/), subnet);
  assert.equal(text('README.md', /pins its network to `([\d.\/]+)`/), subnet);
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
  assert.equal(
    text('web/src/main.ts', /await fetch\('([\w\/]+)'\)/),
    text('crates/server/src/main.rs', /\.route\("([\w\/]+)", get\(api::health\)\)/),
  );
  assert.equal(
    text('web/src/solver-client.ts', /this\.request\('([\w\/]+)'/),
    text('crates/server/src/main.rs', /\.route\("([\w\/]+)", post\(solve::solve\)\)/),
  );
});

test("nginx's exact location is main.rs's health route", () => {
  assert.equal(
    text('deploy/nginx.conf', /location = ([\w\/]+) \{/),
    text('crates/server/src/main.rs', /\.route\("([\w\/]+)", get\(api::health\)\)/),
  );
});

test("CI's smoke solve sends solve_body, the router tests' request", () => {
  assert.deepEqual(
    JSON.parse(text('.github/workflows/ci.yml', /-d '(\{"rows"[^']*)'/)),
    JSON.parse(text('crates/server/src/tests/mod.rs', /json!\((\{"rows"[\s\S]*?\})\)/)),
  );
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

test('the README quotes MAX_ROUTE', () => {
  const max = read('crates/core/src/game.rs', /pub const MAX_ROUTE: usize = ([\d_]+);/);
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
  const floor = read('crates/search/src/lib.rs', /pub const MEMORY_MIB_RANGE: RangeInclusive<usize> = (\d+)\.\.=MAX_MEMORY_MIB;/);
  const native = read('crates/search/src/lib.rs', /#\[cfg\(target_pointer_width = "64"\)\]\nconst MAX_MEMORY_MIB: usize = ([\d_]+);/);
  const wasm = read('crates/search/src/lib.rs', /#\[cfg\(target_pointer_width = "32"\)\]\nconst MAX_MEMORY_MIB: usize = ([\d_]+);/);
  const states = read('crates/search/src/lib.rs', /pub const MAX_STATES_RANGE: RangeInclusive<usize> = (\d+)\.\.=MAX_STATES;/);
  assert.equal(read('README.md', /budgets up to (\d+) MiB in the browser worker/), wasm);
  assert.equal(read('README.md', /the wasm32 ceiling, and (\d+) MiB natively/), native);
  assert.equal(read('README.md', /(\d+) MiB holds it at every box count/), native);
  assert.ok(
    read('web/src/protocol.ts', /(\d+) MiB, the largest budget the #memory select offers\s+\*\s+and both engines accept/) <= wasm,
    'the #memory select offers a budget the browser worker refuses',
  );
  assert.equal(read('README.md', /state, and (\d+) MiB and cap/), floor);
  assert.equal(read('README.md', /ms, (\d+) state, and/), states);
});

test("the README quotes the #memory select's range and default", () => {
  const memory = items('web/index.html', /<select id="memory">([\s\S]*?)<\/select>/, /value="(\d+)"/g).map(Number);
  assert.equal(read('README.md', /memory select names \((\d+) to/), Math.min(...memory));
  assert.equal(read('README.md', /\(\d+ to\s+(\d+) MiB, \d+ by default\)/), Math.max(...memory));
  assert.equal(read('README.md', /MiB, (\d+) by default\)/), read('web/index.html', /<option value="(\d+)" selected>/));
});

test("protocol.ts and transport.ts use sokomind_core::ACTIONS's letters", () => {
  const actions = text('crates/core/src/lib.rs', /pub const ACTIONS: &\[u8; 4\] = b"([A-Z]+)";/);
  const route = text('web/src/transport.ts', /!\/\^\[([A-Z]+)\]\*\$\/\.test\(value\)/);
  assert.equal(text('web/src/protocol.ts', /export const ACTIONS = '([A-Z]+)';/), actions);
  assert.equal([...route].toSorted().join(''), [...actions].toSorted().join(''));
});

test("the direction pad's data-direction values index ACTIONS by each button's label", () => {
  const actions = text('web/src/protocol.ts', /export const ACTIONS = '([A-Z]+)';/);
  assert.equal(actions[read('web/index.html', /data-direction="(\d)" aria-label="Move up"/)], 'U');
  assert.equal(actions[read('web/index.html', /data-direction="(\d)" aria-label="Move down"/)], 'D');
  assert.equal(actions[read('web/index.html', /data-direction="(\d)" aria-label="Move left"/)], 'L');
  assert.equal(actions[read('web/index.html', /data-direction="(\d)" aria-label="Move right"/)], 'R');
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

test("corpus.mjs's diagnosticFields are SearchStats::FIELDS, in order", () => {
  assert.deepEqual(
    items('scripts/corpus.mjs', /export const diagnosticFields = \[([^\]]*)\];/, /'(\w+)'/g),
    items('crates/search/src/lib.rs', /pub const FIELDS: \[&'static str; STAT_COUNT\] = \[([^\]]*)\];/, /"(\w+)"/g),
  );
});

test("the #engine select offers solver-client.ts's ENGINES", () => {
  assert.deepEqual(
    items('web/index.html', /<select id="engine">([\s\S]*?)<\/select>/, /value="(\w+)"/g).toSorted(),
    items('web/src/solver-client.ts', /export const ENGINES = \[([^\]]*)\] as const;/, /'(\w+)'/g).toSorted(),
  );
});

test("protocol.ts's WALL equals sokomind_core::WALL", () => {
  assert.equal(read('web/src/protocol.ts', /export const WALL = (\d+);/), read('crates/core/src/board.rs', /pub const WALL: u8 = (\d+);/));
});

test("the README, Mode's docs and the arena's key test quote the weights of Policy::FAST, Policy::QUALITY and Policy::QUALITY_LIGHT", () => {
  const fast = read('crates/search/src/engine.rs', /const FAST: Self = Self \{\s+weight: (\d+),/);
  const quality = read('crates/search/src/engine.rs', /const QUALITY: Self = Self \{\s+weight: (\d+),/);
  const light = read('crates/search/src/engine.rs', /const QUALITY_LIGHT: Self = Self \{\s+weight: (\d+),/);
  assert.equal(read('README.md', /weighted A\* \(`g \+ (\d+)h`\)/), fast);
  assert.equal(read('crates/search/src/lib.rs', /Weighted A\* \(weight (\d+)\)/), fast);
  assert.equal(read('README.md', /at `g \+ (\d+)h` with reopenings/), light);
  assert.equal(read('README.md', /starts over at\s+`g \+ (\d+)h`/), quality);
  assert.equal(read('crates/search/src/lib.rs', /then weight (\d+) in the same arena/), light);
  assert.equal(read('crates/search/src/lib.rs', /starts over at weight (\d+)\./), quality);
  assert.equal(read('crates/search/src/arena.rs', /let worst = MAX_ROUTE as u64 \+ (\d+) \* u64::from\(MAX_QUEUED_H\);/), fast);
});

test("the README quotes heuristic.rs's REPAIR_CROSSOVER", () => {
  assert.equal(
    read('README.md', /for groups\s+of (\d+) or more/),
    read('crates/search/src/heuristic.rs', /const REPAIR_CROSSOVER: usize = (\d+);/),
  );
});

test("the README quotes corral.rs's CAP on the pushes a sealed corral check tries", () => {
  assert.equal(
    WORDS[text('README.md', /corral with more than (\w+) such pushes/)],
    read('crates/search/src/corral.rs', /const CAP: usize = (\d+);/),
  );
});

test("the README quotes arena.rs's ID_BITS and its record and queue entry sizes", () => {
  const record = read('crates/search/src/arena.rs', /size_of::<Record>\(\) == (\d+)/);
  const entry = read('crates/search/src/arena.rs', /size_of::<Key>\(\) == (\d+)/);
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
  assert.equal(read('crates/search/src/lib.rs', /freed first table,(?:\s+\/\/\/)? (\d+) KiB/), kib);
  assert.equal(read('crates/search/src/arena.rs', /smaller: 2\^(\d+) slots/), shift);
  assert.equal(read('crates/search/src/arena.rs', /slots, (\d+) KiB, so/), kib);
  assert.equal(read('crates/search/src/arena.rs', /INITIAL_TABLE slots \((\d+) KiB\)/), kib);
  assert.equal(read('scripts/parity.mjs', /at most (\d+) KiB, which/), kib);
});

test("the README and reserved_bytes' doc quote engine.rs's ROUTE_ALLOWANCE, and Mode's docs stage.rs's MIN_STATES", () => {
  const allowance =
    read('crates/search/src/engine.rs', /const ROUTE_ALLOWANCE: usize = (\d+) \* MAX_ROUTE;/)
    * read('crates/core/src/game.rs', /pub const MAX_ROUTE: usize = ([\d_]+);/);
  assert.equal(read('README.md', /stage search up to ([\d,]+) bytes/), allowance);
  assert.equal(read('crates/search/src/lib.rs', /stage search up to(?:\s+\/\/\/)? ([\d,]+) bytes more/), allowance);
  const states = read('crates/search/src/stage.rs', /pub\(crate\) const MIN_STATES: usize = (\d+);/);
  assert.equal(read('crates/search/src/lib.rs', /under a limit of ([\d,]+) states or(?:\s+\/\/\/)? more, a bounded/), states);
  assert.equal(read('crates/search/src/lib.rs', /no route under a limit of ([\d,]+) states or more runs/), states);
});

test("the README and Mode's docs quote stage.rs's STAGE_AT, above the benchmark gate's limit and below observe's", () => {
  const at = read('crates/search/src/stage.rs', /pub\(crate\) const STAGE_AT: usize = ([\d_]+);/);
  assert.equal(read('README.md', /once the arena holds ([\d,]+) states/), at);
  assert.equal(read('crates/search/src/lib.rs', /once the arena holds ([\d,]+)(?:\s+\/\/\/)? states/), at);
  const gate = read('benchmarks/catalog-baseline.json', /"maxStates": (\d+),/);
  assert.ok(gate <= at, `the gate's ${gate} states must not exceed STAGE_AT ${at}, or its runs stage before a full arena`);
  const observe = read('scripts/benchmark.mjs', /const OBSERVE = \{ maxStates: ([\d_]+),/);
  assert.ok(observe > at, `observe's ${observe} states must exceed STAGE_AT ${at}, or its runs stage only at a full arena`);
});

test("the README and Mode's docs quote room_bound.rs's BUILD_MIN_STATES, above the benchmark gate's limit and at observe's", () => {
  const min = read('crates/search/src/room_bound.rs', /pub\(crate\) const BUILD_MIN_STATES: usize = ([\d_]+);/);
  assert.equal(read('README.md', /state budget of at least ([\d,]+) states/), min);
  assert.equal(read('crates/search/src/lib.rs', /state budget of at least ([\d,]+) states/), min);
  const gate = read('benchmarks/catalog-baseline.json', /"maxStates": (\d+),/);
  assert.ok(
    gate < min,
    `the gate's ${gate} states must stay below BUILD_MIN_STATES ${min}, or its runs pay the tables' build without repaying it`,
  );
  const observe = read('scripts/benchmark.mjs', /const OBSERVE = \{ maxStates: ([\d_]+),/);
  assert.ok(observe >= min, `observe's ${observe} states must reach BUILD_MIN_STATES ${min}, or its runs lose the tables' pruning`);
});

test("parity.mjs's stage cases run at the server's default budget, as the README says", () => {
  const stage = read('scripts/parity.mjs', /const STAGE_MIB = (\d+);/);
  assert.equal(stage, read('crates/server/src/solve.rs', /const DEFAULT_MEMORY_MIB: usize = (\d+);/));
  assert.equal(read('README.md', /Two Fast cases at the server's default (\d+) MiB/), stage);
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

test('the boot script reads the key theme.ts stores its preferences under', () => {
  const prefix = text('web/src/storage.ts', /const prefix = '([^']+)';/);
  const name = text('web/src/theme.ts', /export const EXPERIENCE_KEY = '([^']+)';/);
  assert.equal(text('web/index.html', /localStorage\.getItem\('([^']+)'\)/), prefix + name);
});

test('the boot script offers the same options theme.ts exports', () => {
  const pairs = [
    [/export const THEME_FAMILIES = \[([\s\S]*?)\] as const;/, /pick\(\[([^\]]*)\], saved\?\.themeFamily/],
    [/export const APPEARANCES = \[([\s\S]*?)\] as const;/, /pick\(\[([^\]]*)\], saved\?\.appearance/],
    [/export const MOTIONS = \[([\s\S]*?)\] as const;/, /pick\(\[([^\]]*)\], saved\?\.motion/],
  ];
  for (const [declared, booted] of pairs)
    assert.deepEqual(items('web/index.html', booted, /'([^']+)'/g), items('web/src/theme.ts', declared, /'([^']+)'/g));
});

test("the boot script applies the theme attributes theme.ts's apply writes", () => {
  const applied = [
    ...text('web/src/theme.ts', /export function apply\(prefs: Preferences, os: System\): void \{([\s\S]*?)\n\}/).matchAll(
      /dataset\.(\w+) =/g,
    ),
  ].map(match => match[1]);
  const booted = [...text('web/index.html', /<script>([\s\S]*?)<\/script>/).matchAll(/dataset\.(\w+) =/g)].map(match => match[1]);
  assert.deepEqual(booted.toSorted(), applied.toSorted());
});

test("theme.ts's meta colors are tokens.css's app-background bases", () => {
  const colors = [...source('web/src/theme.ts').matchAll(/light: '(#[0-9a-f]{6})', dark: '(#[0-9a-f]{6})'/g)];
  assert.equal(colors.length, 3);
  const sheet = source('web/src/styles/tokens.css');
  for (const [, light, dark] of colors) {
    assert.ok(sheet.includes(light), `${light} is not one of tokens.css's colors`);
    assert.ok(sheet.includes(dark), `${dark} is not one of tokens.css's colors`);
  }
});

test("the home page's start link goes to the catalog's first puzzle", () => {
  const first = JSON.parse(source('data/puzzles.json'))[0];
  assert.equal(text('web/index.html', /id="start"[^>]*href="#\/play\/([^"]+)"/), first.id);
});
