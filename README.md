# Sokomind Rust

A small Rust/WASM port of `SokomindSolver`: its 57 catalog puzzles, labeled
box rules, keyboard/touch play, undo, restart, route replay, custom board import,
browser session/best-route storage, worker search, and optional native search and
PostgreSQL persistence. No frontend framework and no JavaScript game-rule duplicate.

## Run locally

Requires Node 22.13+ (24 recommended), Rust 1.98.1, and a native linker (Visual
Studio C++ Build Tools on Windows). PostgreSQL is optional for local play/search.

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
Use `npm run dev:web` when the WASM package is already built. Rust changes require
`npm run wasm` and a page reload. `npm run build` creates `web/dist` for NGINX.

## Full stack with Docker

Copy `.env.example` to `.env`, choose a PostgreSQL password, then run
`docker compose up --build`. Open `http://127.0.0.1:8080`. The database uses a
named volume; only NGINX is exposed. NGINX is required in front of the API
binary, which serves only `/api`: NGINX serves the web app and supplies the
security and cache headers, gzip, and the bounds on slow clients and open
connections. For existing NGINX, serve `web/dist` and adapt `deploy/nginx.conf`'s
API upstream. Terminate HTTPS at your existing proxy before exposing this beyond
localhost.

Upgrading a database created before migration `0002` deletes every saved server
route: `0002` drops and recreates the `progress` table to key it by layout
fingerprint. Back up with `pg_dump` first if you need the old rows. Browsers keep
their profile token and local best routes, but the server only receives a route
when that browser solves the puzzle again; opening a puzzle and letting Replay
best play to the end re-uploads its local best.

## Layout and boundaries

| Path | Responsibility |
| --- | --- |
| `crates/core` | Dependency-free parser, compact state, rules, delta undo, strict replay |
| `crates/search` | Dependency-free push A*: one policy-driven engine over a reserved arena and transposition table, reachability, label assignment heuristic, and sound deadlock pruning; exact proofs |
| `crates/wasm` | Thin wasm-bindgen wrappers; scalar commands and typed-array snapshots |
| `crates/server` | Axum/Tokio HTTP API behind NGINX, bounded native CPU jobs, SQLx/PostgreSQL route verification |
| `web/src` | Vite/TypeScript, HTML/CSS, canvas renderer, cancellable module worker |
| `data/puzzles.json` | Reference catalog snapshot, shared by frontend and backend |
| `migrations`, `deploy` | Database schema and NGINX configuration |

Game state stays in Rust. Board geometry crosses the WASM boundary once per load;
small snapshots cross on moves. Search stays entirely in one WASM worker or native
blocking task. Only initial inputs, throttled telemetry, and improved verified
routes cross boundaries. Workers are terminated after each run to release the WASM
arena. HTTP is sufficient here; there is no multiplayer or continuous server
stream requiring WebSockets.

The search uses dense u16 cells, precomputed neighbors, fixed inline box arrays,
equal-label canonicalization, an arena-index transposition table, reusable flood
buffers, reverse-push distances, and a minimum-cost assignment per box label
(only the pushed box's label group is re-scored per child: a lookup for one box,
a re-solve for two, and a one-row dual repair of the parent's duals for groups
of 3 or more; pushes onto a cell with no reachable goal of the box's label are
dropped before the estimate).
Each edge is one push plus a shortest walk to its support cell. Keeper position
remains part of state identity. Walks are reconstructed only for reported routes.

All three modes run one engine; a `Policy` sets the queue weight and the goal
and reopen behavior. Fast is weighted A* (`g + 5h`) that stops at its first
route and never re-expands a closed node. Quality (`g + 3h`) keeps improving the
shortest verified incumbent until its queue empties or a limit hits. Neither
ever proves anything. Optimal wraps the same engine as `ExactSearch`:
admissible A* (weight 1) with reopenings, the only source of proofs. It reports
`optimal` when a goal pops or the frontier empties with a verified route, and
`unsolvable` only when the frontier empties without one. When a limit or
cancellation stops it with a route, it reports `optimal` if the frontier's
lower bound has reached the route's length, otherwise `bounded` (verified route
plus a sound lower bound and gap). A run stopped before any route carries no
proof. Every mode shares the reference's sound post-push deadlock pruning:
fully blocked 2x2 wall/box squares and frozen-component fixpoints, which only
remove states from which no solution exists.
The objective is total remaining moves, not pushes. These are MVP algorithms:
the reference's advanced portfolio, tunnel/corral/PDB machinery, generators,
and route-repair strategies are not yet ported; Grand Hall performance parity
is not claimed.

Boards are limited to 4096 cells and 32 boxes (all imported puzzles fit). Routes
are limited to 100,000 moves. Native requests cap at 30 seconds, 500,000 states,
64 MiB accounted search storage, and one concurrent CPU job by default (see
`SOLVE_CONCURRENCY` under Configuration). The search memory metric is computed
from reserved buffer sizes (arena, queue, table, and per-cell flood, deadlock,
dead-cell, and distance buffers), not process RSS, allocator overhead, WASM
runtime, or frontend memory. Deadline checks occur between bounded expansion batches;
setup/reconstruction can add latency.

## HTTP

* `GET /api/health` — API and persistence availability.
* `POST /api/solve` — `{rows: string[], actions: "", mode: "fast"|"quality"|"optimal", time_ms: 5000, max_states: 200000, memory_mib: 64}`.
* `GET /api/progress/{id}` — one saved route.
* `POST /api/progress/{id}` — `{route: "UDLR..."}`; server replay validates counters
  and completion, then atomically keeps a better moves/pushes pair.

Progress endpoints require `x-profile-id`, a browser-generated random 32-hex token.
This is an anonymous local profile capability, not an account/login system. Losing
browser storage loses the profile token. Custom puzzles stay local. No database
credentials or SQL cross into the frontend. Saved routes are capped at 10,000 moves.

Every API error body is `{"error": "..."}` (nginx's own 413 and 5xx pages are
not): 400 invalid input, a missing or malformed profile, a route that does not
replay or solve, or a solve position plus route over the 100,000-move replay
limit; 404 unknown endpoint, catalog puzzle, or saved route; 405 wrong method;
408 a JSON body not received within 10 seconds; 413 a body over 128 KiB; 415 a
missing or non-JSON content type; 422 JSON of the wrong shape; 429 rate limited
or solver busy; 503 PostgreSQL not configured or unreachable, or a failed search
allocation; 500 a server bug.

Rate limits are per client address (an IPv4 address or an IPv6 /64): 60 saves and
`SOLVE_RATE_PER_MINUTE` solves per minute. A solve that finds every
`SOLVE_CONCURRENCY` slot taken gets 429 at once; nothing queues. The binary has no
connection cap and no idle or header timeout (axum gives hyper no timer), and it
sends no security headers: nginx, which must sit in front of it, bounds slow
clients and open connections and adds `X-Content-Type-Options: nosniff` and
`Referrer-Policy: same-origin` to every response. nginx also limits `/api/` to 10
requests per second per address (burst 20), answers excess with a JSON 429, and
exempts `/api/health`.

Stored progress is keyed by layout fingerprint (`puzzle-v1:{fnv1a}`, identical to
the reference): a changed catalog layout starts fresh records instead of returning
routes that no longer replay.

The binary serves only `/api`; any other path is a JSON 404. nginx serves the web
app: unknown paths get the app shell, web-app files outside `/assets/` carry
`no-cache`, `/assets/` files carry `immutable` cache headers, and a missing
`/assets/` file is a 404 so a stale page cannot hang on a dead content hash after a
rebuild.

## Configuration

The server reads these environment variables. Empty counts as unset. The three
numeric settings clamp an out-of-range value and replace a non-number with the
default, each with a warning; every other invalid value stops startup before the
database wait.

| Variable | Default | Meaning |
| --- | --- | --- |
| `BIND_ADDR` | `127.0.0.1:3000` | Listen address (the Docker image sets `0.0.0.0:3000`) |
| `DATABASE_URL` | unset | Full `postgres://` URL; wins when set. A remote database can require TLS, e.g. `?sslmode=require`: sqlx's `tls-rustls-ring` feature trusts the bundled webpki roots, so the image needs no CA certificates |
| `DATABASE_PASSWORD` | unset | Without `DATABASE_URL`, enables persistence using the parts below; passed as-is, so any characters are safe. With neither set, persistence is off |
| `DATABASE_HOST` | `db` | Used with `DATABASE_PASSWORD` |
| `DATABASE_PORT` | `5432` | Used with `DATABASE_PASSWORD` |
| `DATABASE_USER` | `sokomind` | Used with `DATABASE_PASSWORD` |
| `DATABASE_NAME` | `sokomind` | Used with `DATABASE_PASSWORD` |
| `SOLVE_CONCURRENCY` | `1` | Concurrent native solves, 1..8; each reserves its own arena |
| `SOLVE_RATE_PER_MINUTE` | `20` | Solves per client address per minute, 1..600 |
| `PROGRESS_RETENTION_DAYS` | `0` | 0 keeps progress forever; 1..36500 deletes records whose best route was stored longer ago (equal or worse saves do not refresh it), at startup and hourly |
| `TRUSTED_PROXIES` | empty | Comma-separated IPv4/IPv6 addresses or CIDRs whose `X-Forwarded-For` is believed; empty trusts none |

libpq's `PG*` variables (such as `PGSSLMODE`) also apply as defaults. Migrations run
at startup. The API retries only transient database failures, for about 30 seconds;
authentication failures (SQLSTATE 28P01/28000), a missing database (3D000), and
other non-transient errors fail immediately.

When the peer is a trusted proxy, the client is the rightmost untrusted
`X-Forwarded-For` entry; an unparseable entry ends the walk at the last trusted
hop, and a chain of only trusted hops yields the leftmost. `::ffff:` addresses
count as IPv4. Compose trusts `172.16.0.0/12`, Docker's bridge range, because nginx
is the API's only peer and overwrites `X-Forwarded-For` with its own client
address. If Docker gives your networks another range, set that instead; otherwise
every client shares one rate-limit bucket. Behind another proxy, such as TLS on the
host, also enable the commented `real_ip` block in `deploy/nginx.conf` so both
limits see real clients.

## Small validation surface

```sh
npm run check:rust
npm run test:rust
npm run build
```

`test:rust` includes the one search test binary, `crates/search/tests/search.rs`,
with the reference solver's frozen fixtures (`fixtures::<name>`): 42 boards
whose independent step-oracle optima, soundness regressions, and one proven
unsolvable the exact engine must all reproduce exactly.

`SokomindSolver/` was read as the behavior reference and left unchanged. Puzzle
data retains the original MIT license. The MVP deliberately omits React, PWA,
music, accounts, cloud jobs, and CI, and the reference's editor, generator, journey,
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
too big to fit pans instead of taking swipes (the reference also has tap-to-move);
`Cargo.lock` contains rsa and sqlx's other optional drivers because
Cargo locks all-target resolution even when they are never compiled, so
`cargo audit` may false-positive on rsa.
