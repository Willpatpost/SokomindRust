# Sokomind Rust

A small Rust/WASM port of `SokomindSolver`: 57 original catalog puzzles, labeled
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

For server persistence, export `DATABASE_URL` before starting the server. Migrations
run automatically. Example PowerShell:

```powershell
$env:DATABASE_URL = 'postgres://sokomind:your-password@127.0.0.1:5432/sokomind'
npm.cmd run server
```

This checkout also supports project-local tools in `.tools/cargo`, `.tools/rustup`,
and `.tools/bin`; the npm scripts discover them automatically. `.tools` is ignored.
Use `npm run dev:web` when the WASM package is already built. Rust changes require
`npm run wasm` and a page reload. `npm run build` creates `web/dist`.

## Full stack with Docker

Copy `.env.example` to `.env`, choose a PostgreSQL password (URL-encode special
characters in connection URLs), then run `docker compose up --build`. Open
`http://127.0.0.1:8080`. The database uses a named volume; only NGINX is exposed.
For existing NGINX, serve `web/dist` and adapt `deploy/nginx.conf`'s API upstream.
Terminate HTTPS at your existing proxy before exposing this beyond localhost.

## Layout and boundaries

| Path | Responsibility |
| --- | --- |
| `crates/core` | Dependency-free parser, compact state, rules, delta undo, strict replay |
| `crates/search` | Dependency-free incremental search, arena, transposition table, reachability, assignment |
| `crates/wasm` | Thin wasm-bindgen wrappers; scalar commands and typed-array snapshots |
| `crates/server` | Axum/Tokio HTTP, bounded native CPU jobs, SQLx/PostgreSQL route verification |
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
buffers, reverse-push distances, and minimum-cost label-compatible assignment.
Each edge is one push plus a shortest walk to its support cell. Keeper position
remains part of state identity. Walks are reconstructed only for reported routes.

Fast uses weighted A* and returns its first route. Quality continues weighted
search, preserving the shortest verified incumbent. Optimal uses admissible A*
with reopenings and only marks a route proven when its goal is popped from the
global queue. Limits and cancellation never establish optimality. The objective
is total remaining moves, not pushes. These are MVP algorithms: the reference's
advanced portfolio, tunnel/corral/PDB machinery, generators, and route-repair
strategies are not yet ported; Grand Hall performance parity is not claimed.

Boards are limited to 4096 cells and 32 boxes (all imported puzzles fit). Routes
are limited to 100,000 moves. Native requests cap at 30 seconds, 500,000 states,
64 MiB accounted search storage, and one concurrent CPU job by default. Change
`SOLVE_CONCURRENCY` intentionally; each job reserves its own arena. The search
memory metric covers major reserved structures, not process RSS, allocator
overhead, WASM runtime, or frontend memory. Deadline checks occur between bounded
expansion batches; setup/reconstruction can add latency.

## HTTP

* `GET /api/health` — API and persistence availability.
* `GET /api/puzzles` — shared catalog.
* `POST /api/solve` — `{rows: string[], actions: "", mode: "fast"|"quality"|"optimal", time_ms: 5000, max_states: 200000, memory_mib: 64}`.
* `GET /api/progress` — best-route summaries.
* `GET /api/progress/{id}` — one saved route.
* `POST /api/progress/{id}` — `{route: "UDLR..."}`; server replay validates counters
  and completion, then atomically keeps a better moves/pushes pair.

Progress endpoints require `x-profile-id`, a browser-generated random 32-hex token.
This is an anonymous local profile capability, not an account/login system. Losing
browser storage loses the profile token. Custom puzzles stay local. No database
credentials or SQL cross into the frontend. Native solve overload returns 429;
persistence without a database returns 503.

## Small validation surface

```sh
npm run check:rust
npm run test:core
npm run build
```

`SokomindSolver/` was read as the behavior reference and left unchanged. Puzzle
data retains the original MIT license. The MVP deliberately omits React, PWA,
music, accounts, cloud jobs, elaborate editor tooling, and extensive test/CI setup.
