# Sokomind Rust

A small Rust/WASM port of `SokomindSolver`: its 57 catalog puzzles, labeled
box rules, keyboard/touch play, undo, restart, route replay, custom board import,
browser session/best-route storage, worker search, and optional native search and
PostgreSQL persistence. No frontend framework and no JavaScript game-rule duplicate.

## Run locally

Requires Node 24.14+, Rust 1.98.1, and a native linker (Visual Studio C++ Build
Tools on Windows). PostgreSQL is optional for local play/search.
`rust-toolchain.toml` pins the Rust version, and `.node-version` the exact Node
that CI and the Docker image use (24.14.0), which is also the floor
`package.json`'s `engines` gives.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked
npm ci
npm run dev
```

Open the Vite URL (normally `http://127.0.0.1:5173`). In another terminal:

```sh
npm run server
```

The server answers only `/api`, which the Vite dev server proxies to it. For server
persistence, export `DATABASE_URL` (see Configuration) before starting the
server. Example PowerShell:

```powershell
$env:DATABASE_URL = 'postgres://sokomind:your-password@127.0.0.1:5432/sokomind'
npm.cmd run server
```

This checkout also supports project-local tools in `.tools/cargo`, `.tools/rustup`,
and `.tools/bin`; the npm scripts discover them automatically. `.tools` is ignored.
`npm run wasm` stops unless `wasm-bindgen --version` matches the version that
`Cargo.lock` resolves (0.2.128 above), so reinstall the CLI when that version
changes, or put a prebuilt `wasm-bindgen` in `.tools/bin`. Use `npm run dev:web`
when the WASM package is already built. Rust changes require `npm run wasm` and a
page reload. `npm run build` creates `web/dist` for NGINX.

## Full stack with Docker

Copy `.env.example` to `.env`, choose a PostgreSQL password, then run
`docker compose up --build`. Open `http://127.0.0.1:8080`. To change a budget,
rate or retention setting (see Configuration), uncomment it in `.env`; compose
passes one left commented or empty as an empty value, which the server reads as
unset, so its default applies. PostgreSQL reads `POSTGRES_PASSWORD` only when it
initializes the empty named database volume: to change the password later, also
change it in the database (`ALTER ROLE sokomind PASSWORD ...`), or the API cannot
log in and keeps restarting. Only NGINX publishes a port, and only on
`127.0.0.1`. NGINX is required in front of the API binary: it serves the web
app and adds gzip and the headers and limits described under HTTP. The image
pins NGINX 1.30. NGINX looks the `api` container up again about every 10
seconds and keeps idle connections to it open for reuse, so it follows a
recreated `api` container within about 10 seconds, without a restart of its
own, and it starts, serving the game, even while the API is down (API requests
get 502 until the API answers). For existing NGINX, serve
`web/dist` and adapt `deploy/nginx.conf`'s `upstream api` block: point `server`
at the API's `BIND_ADDR`; `resolve` needs NGINX 1.27.3 or later, and
`127.0.0.11` is Docker's DNS server, so point `resolver` at yours, or drop the
`resolver` lines and `resolve` for a fixed address. Then set the API's
`TRUSTED_PROXIES` to that NGINX's address. Terminate HTTPS at your existing
proxy before exposing this beyond localhost; when that proxy forwards to this
NGINX, also enable the commented `real_ip` block in `deploy/nginx.conf` so
NGINX's and the API's rate limits see real clients.

Upgrading a database created before migration `0002` deletes every saved server
route: `0002` drops and recreates the `progress` table to key it by layout
fingerprint. If you need the old rows, back them up with `pg_dump` before the
upgraded API first starts, because it migrates at startup. Browsers keep their
profile token and local best routes, but the server only receives a route when
that browser solves the puzzle again; opening a puzzle in a page loaded after
the upgrade and letting Replay best play to the end re-uploads its local best.

Compose pins its network to `172.16.57.0/24` (`TRUSTED_SUBNET` in `.env` changes
it), the range the API trusts (see `TRUSTED_PROXIES` under Configuration). After
upgrading a stack created before the pin, or changing `TRUSTED_SUBNET`, run
`docker compose down` once before `up` (the database volume is kept) so Compose
recreates the network with the new subnet. Were the old network left in place,
NGINX's address would fall outside `TRUSTED_PROXIES` and every client would share
one rate-limit bucket.

The `Dockerfile` builds both images. Its Rust, Debian and Node base images
share the Debian release that `ARG DEBIAN_RELEASE` names once (the server binary
links the Rust image's glibc and runs on the Debian one, so the two must match).
`rust-base` is the Rust 1.98.1 image with `rust-toolchain.toml`, and `stubs`
adds the workspace manifests with stub sources, so each build stage compiles its
dependencies first, in a layer that survives crate edits. Target `server`
(compose's `api`) builds in `server-build`. Target `web` reads `wasm-bindgen`'s
version from `Cargo.lock` in `wasm-bindgen-version` and installs that
`wasm-bindgen-cli` in `wasm-tools` (a `Cargo.lock` edit that keeps that version
keeps the installed CLI), builds the WASM package in `wasm-build` and the app in
`web-build`, and copies the app into the NGINX image. `web-build` runs
`npm run build:image`, the same app type-check and bundle as `build:web` without
the test type-check, which CI runs, so `web/tests` stays out of the build
context. The two targets share only `rust-base` and `stubs`, so a server-only
edit runs no WASM step and a WASM-only edit no server step;
`docker compose build api` builds only the server image. `web-build` runs the
Node image that `ARG NODE_VERSION` names, whose default must equal
`.node-version`, and the `rust-base` tag must equal the version
`rust-toolchain.toml` pins; CI fails when either differs (see Validation).

## Layout and boundaries

| Path | Responsibility |
| --- | --- |
| `crates/core` | Dependency-free parser, compact state, rules, delta undo, strict replay |
| `crates/search` | Dependency-free push A*: one policy-driven engine over a reserved arena and transposition table, reachability, label assignment heuristic, and sound deadlock and sealed-corral pruning; exact proofs |
| `crates/search/examples/catalog.rs` | Reproducible native corpus: searches catalog puzzles and prints one JSON line per run, for the benchmarks and the parity check |
| `crates/wasm` | Thin wasm-bindgen wrappers; scalar commands and typed-array snapshots |
| `crates/server` | Axum/Tokio HTTP API behind NGINX, bounded and rate-limited native CPU jobs, replay-verified best routes in SQLx/PostgreSQL |
| `web/src/main.ts` | Entry: loads the catalog and the WASM game, then wires input, board, solver, playback, and progress |
| `web/src/board.ts` | `BoardView` canvas renderer |
| `web/src/input.ts` | On-screen buttons, board swipes, and keyboard bindings |
| `web/src/playback.ts` | Route replay state machine: idle, playing, paused |
| `web/src/solver-client.ts` | `SolverClient`: one search in the browser worker or on `/api/solve`, with a watchdog and cancellation; replays every route it receives and checks the proof against it |
| `web/src/solver.worker.ts` | Module worker that runs `WasmSearch` in short slices and posts throttled progress and routes |
| `web/src/verdict.ts` | The search status line: what is proven about the route on show, then how the search ended |
| `web/src/progress.ts` | `ProgressClient`: `/api/health` probing and server progress reads and saves |
| `web/src/storage.ts` | `localStorage` session, local best routes, and profile token |
| `web/src/transport.ts` | Validating decoders for worker replies, `/api` replies, and the WASM snapshot and metrics ABI; the `/api/solve` request encoder |
| `web/src/protocol.ts` | Constants and message types that mirror the Rust side |
| `web/src/scheduler.ts` | Injectable clock, so tests can drive time |
| `web/index.html`, `web/src/style.css` | Page markup and styles |
| `data/puzzles.json` | Reference catalog snapshot, shared by frontend, backend, and benchmark corpus |
| `migrations`, `deploy` | Database schema and NGINX configuration |
| `scripts` | npm helpers: toolchain lookup, WASM build, benchmark runner and gate, parity check, one-command validation |
| `benchmarks` | Reviewed benchmark baselines (see Validation) |
| `Dockerfile`, `compose.yaml` | Images and the full stack (see Full stack with Docker) |
| `.github/workflows/ci.yml` | CI (see Validation) |

Game state stays in Rust. Board geometry crosses the WASM boundary once per load;
small snapshots cross on moves. Search stays entirely in one WASM worker or native
blocking task. Only initial inputs, throttled telemetry, and improved verified
routes cross boundaries. Workers are terminated after each run to release the WASM
arena. HTTP is sufficient here; there is no multiplayer or continuous server
stream requiring WebSockets.

The search uses dense u16 cells, precomputed neighbors, fixed inline box arrays,
equal-label canonicalization (the start's label groups are sorted once; after
each push only the pushed box shifts within its group until the group is sorted
again), an arena-index transposition table, reusable flood buffers, reverse-push
distances, and a minimum-cost assignment per box label
(only the pushed box's label group is re-scored per child: a lookup for one box,
a re-solve for two, and a one-row dual repair of the parent's duals for groups
of 3 or more; pushes onto a cell with no reachable goal of the box's label are
dropped before the estimate).
Each edge is one push plus a shortest walk to its stand, the cell behind the
box. Keeper position remains part of state identity. Walks are reconstructed
only for reported routes.
Each arena record is 12 bytes plus its box cells, each queue entry one 8-byte
key packing `f`, `h`, and a 26-bit arena id, and each table slot a 4-byte arena
index.

The estimate is the assignment's push count. The start state also adds the
keeper's walk to its first push: the Manhattan distance to the stand of the
nearest statically legal push, one whose target cell and stand are free floor
and whose target is not a dead cell for that box. Every route from an unsolved
state opens with such a push, walls and other boxes only lengthen the walk to
it, and those walking moves are disjoint from the pushes the assignment counts,
so the start's estimate stays admissible. On a pushed child the same stand walk
only prunes: once a route is known, a child whose moves so far, estimate, and
walk reach its length is dropped, so queue keys and stored estimates stay
push-only.

All three modes run one engine; a `Policy` sets the queue weight and the goal
and reopen behavior. Fast is weighted A* (`g + 5h`) that stops at its first
route and never re-expands a closed node. Quality starts as Fast; from Fast's
first route it continues in the same arena at `g + 3h` with reopenings, keeping
the shortest verified incumbent until its queue empties or a limit hits. The
first time the arena fills, in either phase and with a route or without,
Quality empties it in place, keeping its route, if it has one, as the bound to
beat, and starts over at `g + 3h` in the freed arena, generating up to about
twice the state limit in the same memory. So the second phase never runs short
of memory just because Fast used most of it, and the route never ends longer
than it would have at that fill. Only a full arena restarts it, and at most
once: a time limit that hits first ends Quality in whichever phase it is in.
Neither mode ever proves anything. Optimal wraps the same engine in
the crate-private `ExactSearch`, reachable only as `Search` in `Mode::Optimal`:
admissible A* (weight 1) with reopenings, the only source of proofs. It reports
`optimal` when a goal pops or the frontier empties with a verified route, and
`unsolvable` only when the frontier empties without one. When a limit or
cancellation stops it with a route, it reports `optimal` if the frontier's
lower bound has reached the route's length, otherwise `bounded` (verified route
plus a sound lower bound and gap). A run stopped before any route carries no
proof. Every mode shares a sound post-push deadlock rule, the greatest freeze
fixpoint over the pushed box's component, where an axis holds a box when
either neighbor is a wall or frozen box or both are dead cells for its label.
On every expanded state it covers the reference's fully blocked 2x2 wall/box
squares and frozen-component fixpoints, and it only removes states from which
no solution exists. Every mode also skips the children of an expanded state
with a sealed corral: a region the robot cannot reach that holds a box off its
goal or an empty goal, where every push of one of the region's boxes from a
cell the robot reaches is dead under the same rule over the region's boxes
alone (a region with more than six such pushes is not checked). Boxes outside
the region count as floor, since they may still move, so no solution leaves
through such a state.
The objective is total remaining moves, not pushes. These are baseline algorithms:
the reference's advanced portfolio, PDB machinery and fuller corral search,
generators, and route-repair strategies are not yet ported; Grand Hall
performance parity is not claimed. Some techniques are rejected outright:

- PI-corral successor restriction and corral ordering: push-objective
  techniques that gave false proofs under the move objective in the sister
  ports and can lose routes in Fast and Quality.
- Corral analysis that treats boxes outside the corral as permanent blockers,
  which reported a board solvable in 3 moves unsolvable (Sokomind2's audit,
  F-001).
- Checking only the corrals of the boxes beside the robot after a push, or
  each empty pocket's corral as well: both are sound, but the first skips the
  costliest prunes, so for about 6% less time memory-bound runs generated up
  to 3.6 times as many states and lost routes, a proof and lower bounds, and
  the second was measured slower for little extra pruning.
- Tunnel macros, forced-push macros and the goal-commitment skip: the
  reference measured tunnel macros slower and ships them off, and none of the
  three prunes anything or tightens a bound under push A* with an exact
  keeper walk.
- Sokomind2's linear-conflict and interaction-boost terms, and goal cuts as a
  heuristic term: none is a lower bound, and the first two gave a false optimal
  proof and a false bound there.

Board text has one line per row: `O` is a wall, a space floor, `R` the robot,
`X` a box whose goals are `S`, and any other uppercase letter a box whose goals
are that letter in lowercase. `o`, `r`, `s` and `x` are reserved, because no box
carries `O`, `R` or `S` and `X`'s goal is spelled `S`; the parser rejects them
with their row and column, like any other unsupported symbol.
Boards are limited to 4096 cells and 32 boxes (all imported puzzles fit). Routes
are limited to 100,000 moves. Native requests cap at 30 seconds, 60,000,000 states
per arena (`sokomind_search::MAX_STATES`), 128 MiB accounted search storage, and
one concurrent CPU job by default (see `SOLVE_CONCURRENCY` under Configuration);
the search crate, and so the browser worker, accepts budgets up to 256 MiB. The
web app sends the full state cap and the budget its memory select names (16 to
128 MiB, 64 by default) to either engine. A state costs a 12-byte record, 2
bytes per box, an 8-byte queue entry, and 8 to 16 bytes of index table, so the
memory budget always binds before the state cap: on the catalog's boards 64 MiB
holds about 0.91-2.10M states, 128 MiB 1.83-4.19M, and 256 MiB 3.67-8.39M, and a
run that fills its arena reports a memory limit. The arena reserves the smaller
of the state cap and what the budget holds when the search starts, so a request
at the default cap reserves nearly its whole budget up front, even on a small
board: the memory budget is the bound a caller declares, and one that wants a
smaller footprint asks for less memory or fewer states. The search memory metric
is computed from reserved buffer sizes (arena, queue, table, and per-cell flood,
deadlock, corral, dead-cell, and distance buffers) plus a fixed allowance for the
longest route and scratch buffers, not process RSS, allocator overhead, WASM
runtime, or frontend memory.
Deadline checks occur between bounded expansion batches; setup/reconstruction can
add latency.

## HTTP

* `GET /api/health` — `{status: "ok", persistence: bool}`: API and persistence
  availability. `persistence` comes from a `SELECT 1` probe (900 ms deadline)
  whose answer is reused for 1 s and which takes no progress slot, so busy saves
  do not read as persistence being off. Requests that arrive while a probe runs
  answer from the previous probe instead of waiting, so those that race the
  server's first probe report `false` even with the database up; the web app
  asks again (below) and picks persistence up then.
* `POST /api/solve` — `{rows: string[], actions: "", mode: "fast"|"quality"|"optimal", time_ms: 5000, max_states: 60000000, memory_mib: 128}`.
  Only `rows` and `mode` are required; the values shown are the defaults. The
  limits start at 10 ms, 1 state, and 4 MiB and cap as under Layout and
  boundaries. The reply is `{status, route, moves, pushes, expanded, generated,
  reserved_bytes, elapsed_ms, proof, stats}`: `route`, `moves`, and `pushes`
  cover only the moves after `actions` and are null without a route; `proof` is
  null or `{kind, lower_bound, upper_bound}`, with no bounds for `unsolvable`;
  `stats` holds the search counters by name.
* `GET /api/progress/{id}` — one saved route, `{puzzle_id, moves, pushes, route}`.
* `POST /api/progress/{id}` — `{route: "UDLR..."}`; server replay counts moves and
  pushes and checks completion, then atomically keeps a better moves/pushes pair
  (fewer moves, then fewer pushes). The reply is `{saved: true, improved: bool}`,
  where `improved` says whether this route was stored.

Progress endpoints require `x-profile-id`, a browser-generated random 32-hex token.
This is an anonymous local profile capability, not an account/login system. Losing
browser storage loses the profile token. Custom puzzles stay local. No database
credentials or SQL cross into the frontend. Saved routes are capped at 10,000 moves.

The web app calls the progress endpoints only after `/api/health` has reported
`persistence: true`. Until it does, the app asks again after 2 s and doubles the
wait up to 5 minutes, so a server that was busy, restarting, or started after the
page was opened is picked up once it answers. A 404, or a 200 reply that is not
JSON (static hosting), stops the probing. Once the server has answered for a
route, by replying to its save or returning it as the stored best, the page does
not post that route for that puzzle again until it is reloaded, even when Replay
best solves the puzzle with it again; a save that failed or was rejected is
posted again on the next solve.

Every API error body is `{"error": "..."}` (NGINX's own 413 and 5xx pages are
not): 400 invalid input, a missing or malformed profile, a route that does not
replay or solve, out-of-range solve limits, a memory budget too small for the
board, or a solve position plus route over the 100,000-move replay limit; 404
unknown endpoint, catalog puzzle, or saved route; 405 wrong method; 408 a JSON
body not received within 10 seconds (on a direct run: NGINX reads each whole body
before it passes a request on); 413 a body over 128 KiB; 415 a missing or
non-JSON content type; 422 JSON of the wrong shape; 429 rate limited, or solver
or progress busy; 503 PostgreSQL not configured or unreachable, a database
operation that timed out (each gets 2.5 s) or deadlocked with a concurrent one
(both say to retry later), or a failed search allocation; 500 a server bug.
Both progress endpoints check in the same order: profile and request shape (400),
catalog puzzle (404), PostgreSQL (503), progress slot (429 busy); a save then
checks its rate limit (429) before it replays the route.

Rate limits are per client address (an IPv4 address or an IPv6 /64): 60 saves and
`SOLVE_RATE_PER_MINUTE` solves per minute. A solve that finds every
`SOLVE_CONCURRENCY` slot taken, or a progress request that finds every
`PROGRESS_CONCURRENCY` slot taken, gets 429 at once; nothing queues. A busy answer
does not count against the rate limit, nor does a save turned away before replay
(bad profile, over-long route, unknown puzzle, no PostgreSQL) or a solve with
out-of-range limits or an unknown mode. The binary has no
connection cap and no idle or header timeout (axum gives hyper no timer), and it
sends no security headers: NGINX, which must sit in front of it, bounds slow
clients and open connections and adds `X-Content-Type-Options: nosniff` and
`Referrer-Policy: same-origin` to every response. NGINX also limits `/api/` to 10
requests per second per address (burst 20) and answers excess with a JSON 429;
`/api/health` has its own budget of 2 per second (burst 10) instead, so polling it
never drains the `/api/` limit.

Stored progress is keyed by layout fingerprint (`puzzle-v1:{fnv1a}`, identical to
the reference): a changed catalog layout starts fresh records instead of returning
routes that no longer replay.

The binary serves only `/api`; any other path is a JSON 404. NGINX serves the web
app: unknown paths get the app shell, web-app files outside `/assets/` carry
`no-cache`, `/assets/` files carry `immutable` cache headers, and a missing
`/assets/` file is a 404 so a stale page cannot hang on a dead content hash after a
rebuild.

## Configuration

The server reads these environment variables. Empty counts as unset. The defaults
below are the server's and are written nowhere else: compose sets
`DATABASE_PASSWORD` and `TRUSTED_PROXIES` and passes the budget, rate and
retention settings through from `.env`, where they start commented out. The
settings with a range clamp an out-of-range value and replace a non-number with
the default, each with a warning. A negative value counts as a non-number, except
for the two retention settings, which clamp it. A value too large for the server
to parse also counts as a non-number. Every other invalid value, such as a
malformed `TRUSTED_PROXIES` entry or a `DATABASE_PORT` that is not a port
number, stops startup before the database wait.

| Variable | Default | Meaning |
| --- | --- | --- |
| `BIND_ADDR` | `127.0.0.1:3000` | Listen address (the Docker image sets `0.0.0.0:3000`) |
| `DATABASE_URL` | unset | Full `postgres://` URL; wins when set. A remote database can require TLS, e.g. `?sslmode=require`: sqlx's `tls-rustls-ring` feature trusts the bundled webpki roots, so the image needs no CA certificates |
| `DATABASE_PASSWORD` | unset | Without `DATABASE_URL`, enables persistence using the parts below; passed as-is, so any characters are safe. With neither set, persistence is off |
| `DATABASE_HOST` | `db` | Used with `DATABASE_PASSWORD` |
| `DATABASE_PORT` | `5432` | Used with `DATABASE_PASSWORD` |
| `DATABASE_USER` | `sokomind` | Used with `DATABASE_PASSWORD` |
| `DATABASE_NAME` | `sokomind` | Used with `DATABASE_PASSWORD` |
| `SOLVE_CONCURRENCY` | `1` | Concurrent native solves, 1..8; each reserves its own arena of up to 128 MiB accounted search storage, so up to 1 GiB at 8. Compose sets the API no memory limit; a host that sets one must allow for this on top of the process |
| `SOLVE_RATE_PER_MINUTE` | `20` | Solves per client address per minute, 1..600 |
| `PROGRESS_CONCURRENCY` | `4` | Concurrent progress reads and saves, 1..32; one more gets 429 at once |
| `DB_POOL_SIZE` | `PROGRESS_CONCURRENCY` + 1 | PostgreSQL connections, 1..33: one per progress slot plus a spare for the health probe and the retention sweep. A smaller pool logs a warning at startup; saves can then wait for a connection and fail with 503 after 1 s |
| `PROGRESS_RETENTION_DAYS` | `0` | 0 keeps progress forever; 1..36500 deletes records whose best route was stored longer ago (equal or worse saves do not refresh it), at startup and hourly |
| `PROGRESS_RETENTION_BATCH_SIZE` | `500` | Records deleted per retention batch, one short transaction each, 1..5000; a sweep runs at most 20 batches, so each API process deletes at most 20 times this many records per sweep (10,000 at the default, 100,000 at 5000). A sweep that runs all 20 full logs that it reached its cap; any expired records left wait for the next sweep |
| `TRUSTED_PROXIES` | empty | Comma-separated IPv4/IPv6 addresses or CIDRs whose `X-Forwarded-For` is believed; empty trusts none |

libpq's `PG*` variables (such as `PGSSLMODE`) also apply as defaults. Migrations run
at startup, before the request pool opens, on one connection of their own that is
closed afterwards. Request connections stop a statement after 1.5 s or a lock wait
after 500 ms; the migration connection sets both limits to 0, overriding role,
database and `PGOPTIONS` defaults, so a long migration, or a replica waiting for
another's migration lock, delays startup instead of failing it. At startup the API
retries only transient connection failures, for about 30 seconds; authentication
failures (SQLSTATE 28P01/28000), a missing database (3D000), and other
non-transient errors fail immediately.

When the peer is a trusted proxy, the client is the rightmost untrusted
`X-Forwarded-For` entry; an unparseable entry ends the walk at the last trusted
hop, and a chain of only trusted hops yields the leftmost. `::ffff:` addresses
count as IPv4. Compose trusts its own network, because NGINX is the API's only
peer and overwrites `X-Forwarded-For` with its own client address. It pins that
network to `172.16.57.0/24` rather than leave the range to Docker, and one YAML
anchor in `compose.yaml`, `x-trusted-subnet`, sets both the network and
`TRUSTED_PROXIES`, so the two cannot drift apart. `172.16.0.0/16` lies outside
Docker's default address pools, so no network Docker allocates by itself can take
it; if it clashes with a network on your host, set `TRUSTED_SUBNET` in `.env` to
another IPv4 CIDR, which the anchor then uses for both. If NGINX's range
and `TRUSTED_PROXIES` ever differ, every client shares one rate-limit bucket.

## Validation

CI (`.github/workflows/ci.yml`) runs these on pushes to `main`, on pull
requests, and on demand (`workflow_dispatch`); a push or pull request that
changes only `LICENSE` starts no run, but a README-only one does, because
`test:scripts` checks the values this README copies. The first five run on
Ubuntu and Windows and the rest, after `npm ci`, on Ubuntu with a PostgreSQL 18
service. Every job names its runner image exactly (`ubuntu-24.04`,
`windows-2025-vs2026`) rather than a `-latest` label, and those that run Node
use the version `.node-version` pins, which is also the `engines` floor, so no
separate floor job exists. It builds the WASM package once and checks parity
against the native records `bench:check` wrote. Dependabot
(`.github/dependabot.yml`) opens weekly update pull requests for the pinned
actions and the npm and Cargo dependencies, each release after a 7-day
cooldown, and CI checks them like any other; the Rust toolchain, Node version,
Docker base tags, `wasm-bindgen`, and `@types/node` beyond patches move by hand.
`Cargo.lock` contains sqlx's optional MySQL and SQLite drivers, though they
are never compiled, because the sqlx `migrate` and `macros` features the
server enables name them only through weak `dep?/feature` references, which
Cargo locks anyway (rust-lang/cargo#10801), so `cargo audit` may
false-positive on their crates.

```sh
npm run fmt:check
npm run lint:rust
npm run doc:rust
npm run test:rust
npm run test:release
npm run format:check
npm run test:web
npm run test:scripts
npm run check:scripts
npm run bench:check
npm run build
npm run test:parity
npm run bench:observe
npm run test:db
npm run test:browser
```

`npm run validate` runs the same steps locally, in CI's order and with its single
WASM build, then prints a line per step: PASS or FAIL with its duration, or SKIP
with the reason. It skips every step after a failure; `--keep-going` runs them
all. `--quick` skips `bench:observe`, `test:db` and `test:browser`, and `test:db`
is a SKIP, never a PASS, while `SOKOMIND_TEST_DATABASE_URL` is unset. It exits 1
when a step failed and 2 on an unknown option or argument, and must be started
through npm: `npm run validate -- --quick`. Windows PowerShell 5.1 drops a bare
`--`, so quote it there: `npm run validate '--' --quick`. It runs neither
`npm ci` nor CI's deploy job.

Every npm script that runs cargo, except `fmt:check`, passes `--locked`, as the
Dockerfile's builds do, so a `Cargo.lock` that no longer matches the manifests
fails the step instead of being silently rewritten, locally as in CI. After a
hand edit to a `Cargo.toml`, refresh the lock with
`node scripts/rust.mjs update --workspace` and commit it with the edit.
`lint:rust` runs Clippy on every target with warnings as errors, under the
workspace lints in `Cargo.toml`: unsafe code is denied, and an exported item or
crate root (tests and examples included) without a doc fails. `--keep-going`
lets one run report every failing target, not just the first. `doc:rust` builds
the workspace's own docs (`cargo doc --locked --workspace --no-deps`); CI and
`validate` set `RUSTDOCFLAGS=-D warnings`, so a broken intra-doc link, a link
from a public item to a private one, or any other rustdoc warning fails the
step, where a plain `npm run doc:rust` only prints it. `format:check` runs
Prettier (`prettier --check .`) over what `.prettierignore` leaves: the web app,
`scripts` (`*.mjs` and `tsconfig.json`), and the root config files, in the
style `.prettierrc.json` sets; `npm run format` rewrites them. Commits that only
reformat are listed in `.git-blame-ignore-revs`, which GitHub's blame view
skips; `git config blame.ignoreRevsFile .git-blame-ignore-revs` makes a local
`git blame` skip them too. `test:rust` runs every crate's tests except the WASM
adapter's, and `test:release` the core and search tests under the `release-test`
profile, which keeps release optimization without debug assertions but drops
cross-crate LTO and uses 16 codegen units, so it builds faster; shipped binaries
use `release`. `check:scripts` type-checks the helpers in `scripts` from their
JSDoc (`tsc -p scripts`, strict). It stays out of `check:web`, which needs the
WASM bindings in `web/wasm` and so runs later, inside `build:web`; the scripts
type-check without them, alongside the other cheap checks.
`test:web` (`web/tests`) and `test:scripts` (`scripts/*.test.mjs`) are Node unit
tests of the web modules, the benchmark gate, the toolchain's command
environment, the values copied by hand across languages, configs, CI and this
README (each copied limit, timeout, path, name or version must agree with its
original), and the validate script, whose step list must match `ci.yml`'s,
and need no WASM build. `bench:check` runs `crates/search/examples/catalog.rs` on
every catalog puzzle in every mode at the baseline's 20,000 states and 64 MiB
and compares the results with `benchmarks/catalog-baseline.json`. It fails on a
false proof or bound, a regression (rules in `scripts/bench-gate.mjs`), a route,
proof, status or bound improvement the baseline does not record yet (so that a
later loss back to the old value cannot pass), or a changed catalog, never on
timings; `npm run bench:update` rewrites the baseline after review.
`test:parity` builds the WASM package and runs the native corpus again, at the
catalog example's defaults of 20,000 states and 64 MiB (CI passes it the records
`bench:check` wrote instead), then checks that the WASM search matches each
native run (status, counters, bounds, proof, diagnostics, and route) and replays
each route.
`bench:observe` runs four hard catalog boards in every mode at a fixed 1,000,000
states and 64 MiB, three times each: an observation size, recorded in
`benchmarks/observe-reference.json` and far below the state cap so a run stays
short (`--states` and `--memory` override it). It fails only on a false proof
or bound, a nondeterministic run, or a crash. `test:db` runs the ignored
`live_postgres_*` server tests against `SOKOMIND_TEST_DATABASE_URL`, a dedicated, disposable
database that allows `CREATE SCHEMA`. `test:browser` runs the Playwright specs
in `web/tests/browser`, with `/api` stubbed, against the built app, so
`npm run build` comes first; install the browser once with
`npx playwright install chromium`. CI and `validate` run it with `CI=true`, so
a leftover `test.only` fails the run, and Playwright starts its own preview
server, failing if port 4173 is already taken rather than reusing that server.

CI's `deploy` job checks the deployment files, builds both images, and
smoke-tests the stack they make; it pushes nothing. The Dockerfile's
`NODE_VERSION` must equal `.node-version`, and its `rust-base` tag and
`Cargo.toml`'s `rust-version` the version `rust-toolchain.toml` pins;
`docker compose config` must accept `compose.yaml`, the web stage's NGINX image
must accept `deploy/nginx.conf` (`nginx -t`), and both image targets, `server`
and `web`, must build. It then starts the stack as Full stack with Docker
describes, with `.env` copied from `.env.example` and a `compose.override.yaml`
that runs the two images just built, and checks it through NGINX at
`127.0.0.1:8080`: `/api/health` reports persistence (the API reached PostgreSQL
and ran its migrations), an optimal solve of the ultra-tiny board returns its
one-push route, a progress save reads back, `/` serves the app shell, and the
WASM file is served as `application/wasm`. `docker compose down --volumes`
removes the stack afterwards, also after a failure, whose container states and
logs are printed first. The job runs when a push or pull request changes a file
the images, their configuration, or the smoke test come from (the Dockerfile,
`.dockerignore`, `compose.yaml`, `.env.example`, `deploy`, the workflow, and
the Rust and npm manifests, locks, and version pins; the workflow lists them
all), when the base to compare against is missing or can no longer be fetched
(the push that creates a branch, or a force push), on every manual run, and
weekly (Mondays at 06:17 UTC), when it is the only job, because base images
change upstream while the repository does not. A change to other sources alone
waits for the weekly or a manual run.

Rust tests live in `crates/core/tests` (`parse.rs`, `game.rs`) and
`crates/search/tests`, besides unit tests in the search and server sources; the
server's router tests are in `crates/server/src/tests/router.rs` and its live
PostgreSQL tests in `live_postgres.rs` beside it.
`boundary.rs` pins the search API's errors, limits, messages, stats order, and
proof kinds. `search.rs` checks every mode against a BFS oracle on fixed and
seeded random boards and holds the reference solver's frozen fixtures
(`fixtures::<name>`): 42 boards whose independent step-oracle optima, soundness
regressions, and one proven unsolvable the exact engine must all reproduce
exactly.

`SokomindSolver` was read as the behavior reference and left unchanged. Puzzle
data retains the original MIT license. The app deliberately omits React, PWA,
music, accounts, and cloud jobs, and the reference's editor, generator, journey,
daily challenge, achievements, stats, favorites, ratings, share links, progress
import, solver hints, solver lab, board zoom, and in-play deadlock warning.

Deliberate deviations from the reference: strict board parsing (empty rows and
carriage returns are rejected, where the reference's editor import drops blank
lines; one trailing newline tolerated); boards cap at 4096 cells and 32 boxes
(the reference core has no cap; its editor import takes 3x3 to 20x20); sessions
stop at the 100,000-move replay limit (the reference has no in-game cap);
pasted routes are uppercased and stripped of whitespace, which the reference's
route decoder rejects; undo is Z (the reference uses U or Ctrl+Z and binds Z to
Zen mode); touch input is one-finger swipes plus on-screen buttons, and a board
too big to fit pans instead of taking swipes (the reference also has tap-to-move).
