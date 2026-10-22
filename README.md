# Sokomind Rust

A small Rust/WASM port of `SokomindSolver`: its 57 catalog puzzles, labeled
box rules, keyboard/touch play, undo, restart, route replay, custom board import,
browser session/best-route storage, worker search, and optional native search.
No frontend framework and no JavaScript game-rule duplicate.

## Run locally

Requires Node 24.14+, Rust 1.98.1, and a native linker (Visual Studio C++ Build
Tools on Windows).
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

The server answers only `/api`, which the Vite dev server proxies to it.

This checkout also supports project-local tools in `.tools/cargo`, `.tools/rustup`,
and `.tools/bin`; the npm scripts discover them automatically. `.tools` is ignored.
`npm run wasm` stops unless `wasm-bindgen --version` matches the version that
`Cargo.lock` resolves (0.2.128 above), so reinstall the CLI when that version
changes, or put a prebuilt `wasm-bindgen` in `.tools/bin`. Use `npm run dev:web`
when the WASM package is already built. Rust changes require `npm run wasm` and a
page reload. `npm run build` creates `web/dist` for NGINX.

### Experiment knobs

These environment variables are unset by default and change native search
behavior for experiments; each is documented with its measurement in `docs/`:

- `SOKOMIND_DUAL_CACHE` — a directory of certified walk-term dual caches
  (docs/room-bound.md);
- `SOKOMIND_WALK_TERM_LOG` — prints one line per walk-term build, its pivots
  and whether the cache supplied the duals (docs/room-bound.md);
- `SOKOMIND_PORTFOLIO_ROOMS` — a comma list of kept-room indices composing
  a second, coarser bound configuration beside the first
  (docs/room-bound.md);
- `SOKOMIND_STRATEGIC_GUIDANCE` — steers the weighted searches toward
  routes that respect a single-door room's crossing order
  (docs/strategic.md);
- `SOKOMIND_POLISH_RUNS` — raises the polish's box-order cap
  (docs/route-polish.md);
- `SOKOMIND_POLISH_WORKERS` — races the polish's kick rounds on a
  native thread pool: never a longer route, and shorter ones where
  the polish's budget dies mid-chain (docs/parallel-polish.md).

The browser has no environment to read, so its walk-term dual cache is
shipped data: `npm run walk-caches -- --boards large,huge` runs the
native engine at the app's declaration and writes the records and a
rows-hash manifest into `web/public/walk-caches/`, which the worker
fetches for its optimal searches — the engine's own name, dimension and
objective checks decide whether a record is trusted, so a stale
manifest is a missed cache, never a wrong bound.

## The web app

The app is the ported face of the original SokomindSolver app: six hash-routed
pages over the engine.

- **Home** — the daily challenge with its run and seven-day history, the
  solving streak, the guided journey, the recently cleared, and the way back
  into the saved session;
- **Puzzles** — the catalog by tier and collection, with search, favorites,
  felt-difficulty marks and pagination;
- **Play** — the board with hints, deadlock warnings, zen mode, the move
  notation and timeline, the completion dialog, and share links that carry the
  attempt;
- **Editor** — local drafts, the paintable grid, the private playtest,
  share-by-URL, and the procedural generator whose rooms the engine itself
  certifies;
- **Stats** — the aggregates, the activity heat map, achievements and
  keepsakes, import/export, and the replay repository;
- **Solver Lab** — A/B search experiments over one room and position, watched
  step by step and compared side by side.

The theme menu mixes the procedural sound — a master switch, the quiet
soundscape, two volume sliders with previews — and `M` mutes or restores from
anywhere. The app installs as a PWA: the service worker keeps the shell
offline, and a new build offers itself through the update banner.

The shipped footprint is 192.7 kB JS (57.4 kB gzip) + 52.7 kB CSS + 422.5 kB
WASM ≈ 680 kB, all precached by the offline shell. The app adds no runtime
dependencies beyond the pre-port set: no framework, no state library, no audio
package — the synthesizer, the router and the IndexedDB layer are local
modules.

## Full stack with Docker

Run `docker compose up --build`. Open `http://127.0.0.1:8080`. To change a
budget or rate setting (see Configuration), copy `.env.example` to `.env` and
uncomment it there; compose passes one left commented or empty as an empty
value, which the server reads as unset, so its default applies. Only NGINX
publishes a port, and only on `127.0.0.1`. NGINX is required in front of the API binary: it serves the web
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

Compose no longer runs PostgreSQL: best routes live only in each browser. A
stack created before then still has a `db` container, which keeps running until
`docker compose up --remove-orphans` (or `down --remove-orphans`) removes it,
and a `postgres-data` volume that nothing reads. The volume keeps the old saved
routes until `docker volume rm` deletes it; `docker volume ls` shows its name,
which Compose prefixes with the project name.

Compose pins its network to `172.16.57.0/24` (`TRUSTED_SUBNET` in `.env` changes
it), the range the API trusts (see `TRUSTED_PROXIES` under Configuration). After
upgrading a stack created before the pin, or changing `TRUSTED_SUBNET`, run
`docker compose down` once before `up` so Compose
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
| `crates/search` | Dependency-free push A*: one policy-driven engine over a budgeted arena and transposition table, reachability, label assignment heuristic, and sound deadlock and sealed-corral pruning; exact proofs and Optimal's room bound (`room_bound.rs`, `room_bound/`); the bounded stage ladder (`stage.rs`, `stage/`) that Fast and Quality hand a routeless search to |
| `crates/search/examples/catalog.rs` | Reproducible native corpus: searches catalog puzzles and prints one JSON line per run, for the benchmarks and the parity check |
| `crates/wasm` | Thin wasm-bindgen wrappers; scalar commands and typed-array snapshots |
| `crates/server` | Axum/Tokio HTTP API behind NGINX: bounded and rate-limited native CPU jobs |
| `web/src/main.ts` | The entry: theme boot, the `onPage` registrations, the `bind*` wiring, `start()` |
| `web/src/dom.ts` | The element getters and text setters every module shares |
| `web/src/play.ts` | The play core: the session over the WASM game, the solver, playback, progress and timer clients, board and zoom, paint, score, finish, zen and pause, the laboratory's log and result panels, the hint, tap, share, and the engine-shared sound wrapper every move's cue rides |
| `web/src/puzzles-pages.ts` | The puzzles route family: the tier grid, the collections, the filterable paginated list |
| `web/src/home-page.ts` | The home dashboard: the saved session, progress, streaks, the daily, the journey, the recently cleared |
| `web/src/stats-page.ts` | The stats route: aggregates, heat map, progress tools, and the progression showcase |
| `web/src/replay-study.ts` | The replay study dialog, its playback, and its bindings |
| `web/src/lab-page.ts` | The Solver Lab's UI over `lab.ts`'s model |
| `web/src/editor-page.ts` | The editor page: drafts, the paintable grid, the playtest, share-by-URL |
| `web/src/generator-page.ts` | The generator dialog: the room-forger over `generator.ts`'s model |
| `web/src/audio-page.ts` | The sound menu the theme dialog mixes |
| `web/src/offline-shell.ts` | The service worker registration and the update banner |
| `web/src/achievements.ts` | The achievement definitions and their unlock checks over the journal |
| `web/src/audio.ts` | The dependency-free Web Audio synthesizer and the move-cue translation |
| `web/src/board.ts` | `BoardView` DOM-grid renderer: tiles, labels, dead marks, the ghost keeper, the blocked ripple |
| `web/src/catalog.ts` | The catalog snapshot: entries, filtering, pagination, ratings, favorites, the daily pick |
| `web/src/celebrate.ts` | The solve celebration, sized by the tier the solve earned |
| `web/src/complete.ts` | The completion dialog's presentation: verdict, milestones, signed counts |
| `web/src/dialog.ts` | The dialog helpers over `<dialog>` |
| `web/src/editor.ts` | The editor's grid model: paint, flood fill, resize, drafts, the share codec |
| `web/src/frames.ts` | The board-frame cosmetics an achievement unlocks |
| `web/src/generator.ts` | The seeded room generator model: floor growth, scramble, tier reading |
| `web/src/idb.ts` | The small IndexedDB wrapper |
| `web/src/input.ts` | On-screen buttons, board swipes, and keyboard bindings |
| `web/src/journal.ts` | The solve journal: dates, streaks, the daily run, import/export |
| `web/src/journey.ts` | The guided journey's chapters and recommendations |
| `web/src/lab.ts` | The lab's run records and comparisons |
| `web/src/optimal.ts` | The certified-optimum cache the completion star reads |
| `web/src/playback.ts` | Route replay state machine: idle, playing, paused |
| `web/src/progress.ts` | `Progress`: the selected puzzle's session and replay-verified best route |
| `web/src/protocol.ts` | Constants and message types that mirror the Rust side |
| `web/src/replays.ts` | The replay repository and its replay-verified routes |
| `web/src/router.ts` | The hash router: parsing, navigation, subscriptions |
| `web/src/scheduler.ts` | Injectable clock, so tests can drive time |
| `web/src/shell.ts` | The page shell: `onPage` registration and the play-hash helpers |
| `web/src/solver-client.ts` | `SolverClient`: one search in the browser worker or on `/api/solve`, with a watchdog and cancellation; replays every route it receives and checks the proof against it |
| `web/src/solver.worker.ts` | Module worker that runs `WasmSearch` in short slices and posts throttled progress and routes |
| `web/src/stats.ts` | The journal's aggregates and their formatting |
| `web/src/storage.ts` | `localStorage` session and best routes |
| `web/src/theme.ts` | The stored preferences: theme family, appearance, motion, sound |
| `web/src/timer.ts` | The attempt clock: elapsed, pause, per-puzzle select |
| `web/src/traces.ts` | Route traces: frame-by-frame states for study and playback |
| `web/src/transport.ts` | Validating decoders for worker replies, `/api` replies, and the WASM snapshot and metrics ABI; the `/api/solve` request encoder |
| `web/src/verdict.ts` | The search status line: what is proven about the route on show, then how the search ended |
| `web/src/walk.ts` | The shortest-walk builder behind taps and playtests |
| `web/src/zoom.ts` | Pinch-zoom over the board, with clamped panning |
| `web/index.html`, `web/src/style.css`, `web/src/styles/` | Page markup, styles, and the theme token sheets |
| `web/public/` | The PWA face: the manifest, the brand icons, and `sw.js`, the revision-keyed service worker |
| `web/tests/` | The three test layers: Node unit tests over the modules, the Playwright browser specs, the type-check project |
| `scripts/asset-manifest.mjs` | The post-build digester: hashes the shell, lists the hashed assets, and stamps the worker's revision |
| `data/puzzles.json` | Reference catalog snapshot, shared by frontend, tests, and benchmark corpus |
| `deploy` | NGINX configuration |
| `scripts` | npm helpers: toolchain lookup, WASM build, benchmark runner and gate, parity check, one-command validation |
| `benchmarks` | Reviewed benchmark baselines (see Validation) |
| `docs` | Design notes for the stage ladder, the sealed-corral check, route polish, the room bound and its hall abstraction: why each looks the way it does, and the measurements behind it |
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
of 3 or more, read straight off the duals with no augmenting path when the push
brings the box closer to its matched goal; pushes onto a cell with no reachable
goal of the box's label are dropped before the estimate).
Each edge is one push plus a shortest walk to its stand, the cell behind the
box. Keeper position remains part of state identity. Walks are reconstructed
only for reported routes.
Each arena record is 12 bytes plus its box cells, each queue entry one 8-byte
key packing `f`, `h`, and a 28-bit arena id, and each table slot a 4-byte arena
index whose 4 spare bits hold a hash tag, so a probe skips most other states'
records unread.

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

Optimal's distances also count keeper sides. Between two pushes of a box the
keeper only walks around it, so it stays in one component of the floor without
the box's cell, and can push only from a stand there. Optimal therefore keeps
one distance per goal for each side of each cell, so a box whose next push
needs a side the keeper cannot reach pays for the pushes that bring it round.
Other boxes keep their sides through a push, so the estimate stays admissible
and consistent. Its tables take 283 bytes per cell at 32 boxes instead of 68.
Fast and Quality keep the per-cell distances. In their weighted searches the
sharper estimate misled the stage ladder: huge found no route at any limit
from 8,192 to 128,000 states.

Optimal's pushes also pay their stand's charge: the floor distance from the
nearest cell where a box of some label could still reach a goal. A keeper
pushing from a charged stand has walked at least that far since it last stood
on such a cell, and a push leaves the keeper on its box's old cell, so the walks
charged to different pushes do not overlap. The estimate less the keeper's own
charge therefore stays admissible and consistent; a keeper that has just pushed
has none. The start's stand walk counts only the part of its walk that the
stand's charge leaves out. The charge costs one byte per cell.

All three modes run one engine; a `Policy` sets the queue weight and the goal
and reopen behavior. Fast is weighted A* (`g + 5h`) that stops at its first
route and never re-expands a closed node. When Fast reaches its stage point
without a route, a bounded stage search (`crates/search/src/stage.rs`) takes
over in the same arena. The stage point comes once the arena holds 32,768
states, or when it fills, whichever is first, and the stage search needs a few
thousand states there. It may make 32 inserts per state at its stage point, at
most 2^20, or in Fast as many as the state limit allows states when that is
more. 32,768 states earn the first term's 2^20, and the stage search starts
from a copy of the start, so a later stage point would only lengthen the plain
search before it. On the catalog (job 49403, the work then capped at 2^20)
staging there kept every route and slowed no board, and Fast took 16.0 s in all
instead of 27.8 s at 64 MiB, where it staged at the arena's capacity before
(about 1M states); five boards that plain Fast routes after 146,000 to 1.2M
states take the stage search's route instead. Under a large limit Fast's stage
search runs until it routes, gives up or meets the caller's deadline: at 30 s
it routed three more catalog boards, at 6 to 17 s, and at 5 s it lost none,
though most give-ups there ran to the caller's deadline instead of ending on
memory (job 49443). From the start, it commits one goal-level gain at a
time: a small search looks for a state above the current level, and the first
such state that passes accept tests (open lanes into goals, sink-line order,
keeper mobility around sink-line boxes, keeper-aware reach matching and sealed
pockets) is committed. It backtracks when a level fails and ends at its first
route. It is heuristic and incomplete. When it gives up below the limit, Fast
starts over without it and runs as the plain search would; when it gives up at a
full arena, the search stops at the limit as before. `docs/stage-ladder.md`
records how its accept tests, stage point and budgets were chosen and measured.
Quality starts as Fast, but its stage search keeps the capped work (below);
from Fast's first route it continues in the same arena at `g + 2h` with
reopenings, keeping the shortest verified incumbent until its queue empties or a
limit hits. The first time the arena fills, in either phase and with a route or
without, or the stage search ends, Quality empties it in place, keeping its
route, if it has one, as the bound to beat, and starts over at `g + 3h` in the
freed arena, generating up to about twice the state limit in the same memory,
plus the stage search's inserts when it runs (32 per state at the stage point,
at most 2^20). So the second phase never runs short of memory just because Fast
used most of it, and the route never ends longer than it would have at that
fill. When the Fast phase reaches its stage point with no route, the stage
search runs first, and the restart keeps its route, if it finds one, as the
bound. Only a full arena or the stage search's end restarts it, and at most
once: a route that alone fills the arena ends it there instead, and a time limit
that hits first ends Quality in whichever phase it is in. When Quality ends on
its own with a route, at an empty queue or a limit, it polishes the route
before it reports (`crates/search/src/polish.rs`): one box at a time, a small
A* keeps every other box's pushes in their order and searches for a shorter
schedule of that box's own pushes among them, every walk as short as the boxes
then allow. A route is kept only when it replays shorter. A run ends once a
full pass over the boxes gains nothing, and where it ends depends on the order
it visits them in, so the polish makes up to eight runs from the search's
route, each in its own box order, and keeps the shortest. It then frees boxes
in pairs, those that end nearest each other first, so two boxes can trade
order, or goals when they share a label. Once neither gains, the end polish
kicks: it makes two boxes of one label trade goals even for a route up to 63
moves longer, descends from there the same way, and ends after a full round
of kicks finds nothing shorter than its best, or once the kicks have run 16
polish units for each state of the state limit. It runs in the arena's share of
the budget after the arena frees its records, so the reserved bytes do not
change. A long search that stops improving its route also polishes it in
episodes, in the part of that share the arena does not hold, and then goes on;
an episode's route never bounds the search, and the end polish still runs.
`docs/route-polish.md` covers its bounds, memory and what was left out.
Neither mode ever proves anything; a Quality search that a limit stops with
a route in hand still reports a sound lower bound — the floor its reopened
frontier sealed, capped by the route — while Fast reports none, since
without reopenings a closed node can hide the cheaper path a shorter route
would take. Optimal wraps the same engine in
the crate-private `ExactSearch`, reachable only as `Search` in `Mode::Optimal`:
admissible A* (weight 1) with reopenings, the only source of proofs. Its room
tables build only at a state budget of at least 1,000,000 states: below, the
search dies before their pruning repays their build. It reports
`optimal` when a goal pops or the frontier empties with a verified route, and
`unsolvable` only when the frontier empties without one. When a limit or
cancellation stops it with a route, it reports `optimal` if the frontier's
lower bound has reached the route's length, otherwise `bounded` (verified route
plus a sound lower bound and gap). A run stopped before any route carries no
proof.

Once its arena holds 90% of its node limit, Optimal expands partially (PEA*):
each expansion but the start's stores only the children whose f is at most the
node's queued f, and queues the node again at the least f it left out. That key
bounds every route through the children left out, so the frontier's lower bound
stays sound. A child that may be solved is always stored, and a node queued
again at the estimate's cap expands in full. Most children left out are never
built: a push moves one box by one edge of the graph the estimate measures, and
every other box keeps its cell and side, so the child's f is at least the
parent's g and estimate plus the walk to the stand less the stand's charge.
Partial expansion trades re-expansions for stored children, which pays only
once memory binds, so it starts late, and it counts records rather than bytes
so that native and WASM run one search. In probe 48731 (one c4-standard-32
node) it lowered no open case's bound at any budget. It raised Grand Hall's
from 235 to 238 at 1M states and 256 MiB, and from 240 to 243 at 11M states and
1 GiB, in 10.3 s instead of 4.9 s; large's from 141 to 145 at 1M states; and
open-field's from 90 to 92 at 11M. The open catalog cases' bounds summed 3218
instead of 3150 at the CI limit of 20,000 states, and at 5 s 3360 instead of
3292 at 64 MiB, 3467 instead of 3411 at 256 MiB and 3450 instead of 3449 at
1 GiB, while the 26 proofs took the same 1.18 s at 64 MiB. Expanding partially
from the start, as plain PEA* does, raised the 64 MiB sum to 3435, but took
3.97 s over those proofs and lowered 4 cases' bounds at 256 MiB and 8 at 1 GiB;
starting at half the limit lowered one at 1 GiB.

Before its search proper, Optimal builds the room bound (`docs/room-bound.md`).
Each room of at most five doors that holds a box or a goal gets exact
tables of the moves that end inside it, in a relaxed problem that keeps only
some labels' boxes in the room and lets them enter through a door for free;
the one-door rooms come from each floor cell taken as a door, the wider from
pairs of cells near a box or a goal and from the boundaries of the rooms
already found. Tables over disjoint
parts add, and so does the estimate with every move that ends in a part made
free. Optimal queues each state by the larger of that sum and the estimate,
which stays admissible and consistent, and prunes a state a table has no entry
for, since it has no solution. Fast and Quality skip it: it raises bounds, and
its build costs time a weighted search spends finding its route. The build is
paid for as pops, a fixed number of its work units each, and stops at a work
budget scaled to the state limit. Its buffers fit the budget the fixed buffers
and the arena leave, and the tables it keeps, at most a quarter of the budget
less the fixed buffers, are charged to it, so the arena is sized again, never
larger. In probe 49011 it raised Grand Hall's start bound from 214 to 255, and
its bound from 238 to 275 at 1M states and 256 MiB, from 243 to 279 at 11M
states and 1 GiB, from 241 to 277 at 30 s and 256 MiB, and from 244 to 279 at
30 s and 1 GiB. It proved large in 0.3 to 0.4 s, at 1M states, where the
estimate alone stopped at 145, and in the runs where the estimate alone took
1.1 to 1.3 s. The open catalog cases' bounds summed 3376 instead of 3218 at
20,000 states, and 3511 instead of 3360 at 5 s and 64 MiB, with one more case
proved and none lower. The build's forward search refuses a move that leaves
a box frozen off its goal, since no goal state follows it (probe 49024). On
Grand Hall that cuts each large table's states from 10.3M to 2.7M with the
same values, and the build from 24.5M units to 7.5M, about 1.1 s. So at 5 s
with 256 MiB or 1 GiB the bound reaches 276 against 241 and 240 with the
estimate alone, and 280 at 30 s and 1 GiB. At 64 MiB one of its two tables
then fit, and the bound reached 250 against 238. Eight more catalog tables
build, and the open cases' sum at 20,000 states rises to 3403. The kept tables
are just under 7/8 full, with no power-of-two padding, so they take two thirds
of the bytes they did, and a table too large to keep fails as soon as its
backward search shows it (probe 49161). At 64 MiB both of Grand Hall's tables
now fit: its start bound is 255, and its bound at 5 s is 274 against 250.
The open cases' sum at 5 s and 64 MiB, Grand Hall's included, rises from 3442
to 3481, none lower. The estimate added to the tables also charges the keeper's
walk to each push's stand from a cell a box can stand on, for the steps that
end outside the tables' parts (probe 49398). That raises Grand Hall's start
bound to 258 and its bound by 3 wherever a table builds, to 277 at 64 MiB, 279
at 5 s and 256 MiB or 1 GiB, and 283 at 30 s and 1 GiB. The open cases' sum at
20,000 states rises from 3403 to 3437, none lower, at no measurable cost. On a
machine slow enough that the build does not finish, the bound stays at the
start's 214. With the memory cap lifted in a build made for the run, Grand
Hall's bound reaches 284 at 2048 MiB, where memory ends the search at 89 s, and
286 at 4000 MiB, where 60M states end it at 151 s (job 49425). The arena's
ids stopped at 2^26 - 1, about 67.1M states, and at that ceiling 2,103,322
states sat queued at f = 298 with the arena holding its full 60M, so the 299
point read as beyond the search's reach (2026-10-11, the development host).
That was a packing limit, not a memory one: the record's parent field held
two spare bits past the id, and the queue key's h field held 18 bits where
the node's own storage ends at 16, so the id now fills 28 bits and the state
limit 256M, with every queued h clamped at the u16 width a stored estimate
already ends at. At 256M states and 32768 MiB the search holds its full
arena, expands 58.9M states in 19.1 minutes, and ends at f = 301: 299 and
300 fell to exhaustion, and the bound rose from 298 to 301 (2026-10-12, the
development host; `docs/room-bound.md` has the numbers).

The three-door cuts — every triple of pool cells within a 62-cell box, on
budgets of their own beside the pairs — reach the regions no pair isolates.
On Grand Hall the two wings table at last, behind the cuts {(1,6),(5,4),(5,5)}
and its mirror: the bound at 60M states rises from 290 to 292 at 4000 and
32768 MiB, and at 1M states and 256 MiB from 278 to 285, while 64 MiB cannot
hold the wings' searches and the gate's numbers stand. Across the catalog the
pass raises six bounds at the gate's config, theme-parking's by 5 and
gen-v2-320118's by 9 (2026-10-11, the development host; `docs/room-bound.md`
has the numbers).

The wide cuts — four doors as two adjacent pairs in order of the cut's span,
five as a four-door candidate's doors with one pool cell more — lift Grand
Hall again, to 298 at 60M states and 4000 or 32768 MiB and 290 at 1M states
and 256 MiB, through a stronger wing table the reshuffled candidates build.
Grand Hall's 57-cell middle band sits behind its five-door cut at last, but
no key width tables it: under u128 keys its build fails past the forward
search's cap, and the wider keys' cost un-builds tables elsewhere (measured,
not kept; `docs/room-bound.md` has the numbers).

The wide candidates draw their build work from a floor of their own, one cap
beyond the rooms' four with the rooms' leftover flowing in, so the budget the
rooms spend cannot starve them (2026-10-11, the development host). On Grand
Hall the share builds the band's 20-cell sub-region tables under u64 keys for
the first time — the largest reaches 7.34M states — and the keep test drops
every one by 31 to 35: the hall part gives up the pushes of the boxes that
merely cross the band on their way to the wings' goals, more than a table
that charges an entry free and an exit one move can charge back. So the
start's S stays 270, the bound at 60M states stays 298, and the band keeps no
table by the keep test rather than the key width; the floor's cost is build
time, Grand Hall's Optimal build at 1M states and 64 MiB growing from about
2.5 s to 7.9 s on tables that drop. Across the catalog the floor raises six
Optimal bounds at the gate's config, Grand Hall's own 262 to
266 among them, and lowers none (`docs/room-bound.md` has the numbers).

Every mode shares a sound post-push deadlock rule, the greatest freeze
fixpoint over the pushed box's component, where an axis holds a box when
either neighbor is a wall or frozen box or both are dead cells for its label.
On every expanded state it covers the reference's fully blocked 2x2 wall/box
squares and frozen-component fixpoints, and it only removes states from which
no solution exists. Every mode also skips the children of an expanded state
with a sealed corral: an area the robot cannot reach that holds a box off its
goal or an empty goal, where every push of one of the corral's boxes from a
cell the robot reaches is dead under the same rule over the corral's boxes
alone (a corral with more than six such pushes is not checked). Boxes outside
the corral count as floor, since they may still move, so no solution leaves
through such a state.
The objective is total remaining moves, not pushes. Not ported from the
reference: its advanced portfolio, its region pattern database with exit cap
(the whole-board variant below was measured instead), its push-admissible
restricted linear conflict and interaction boost (each would be a gated
heuristic change), generators, and Quality route repair other than the box
rescheduling and two-box repair that the polish ports. On Grand Hall (`huge`),
which plain Fast and Quality do not route in 8M states even at the 1024 MiB cap
(about 7.5 s; job 47772), the stage search finds an 812-move route (268 pushes)
in about 0.37 s at 64 MiB and 0.39 s at 256 MiB, in both Fast and Quality, on a
c4-standard-32 node (job 49403): every budget stages at the same point and
generates the same states first. The reference's own benchmark reports about
5.3 s for 893 moves on its development machine (Node on Windows), so the two
times come from different machines. Quality's search does not shorten it
before its memory limit, which a larger budget only moves later; the polish
does. At the production limit of 5 s Quality returns 468 moves at 64 and
256 MiB, and at 30 s 436 moves (222 pushes) (job 49403). Run for up to an hour
before kicks, Quality reached the same 436-move route at about 24 s at every
budget from 64 MiB to the 1 GiB cap, and at 2048 and 4000 MiB in a build with
the cap lifted; each search then ended at its memory or state limit within 34
to 103 s, so neither more time nor more memory shortened it (job 49425). That
is where the polish's jobs stop gaining, and the kicks start there: with no
time limit Quality now returns 429 moves (224 pushes) at 64 MiB, about 404 s
after the start (job 49484), and the kicks' work cap leaves that run alone
at the 60M-state limit (job 49509). At 256 MiB the polish goes further on its
own, to 424 moves (220 pushes; 1024 MiB gives the same), no time limit — the
memory the deeper jobs need, not the kicks' starting points; a pool of optima
to kick from was measured against it and kept nothing, losing at every memory
tier, 428 against 424 at the sweep's own 1024 MiB (2026-10-11 and 2026-10-12,
the development host; `docs/route-polish.md`). The shortest route found so far is 422 moves
(220 pushes), by probes that kicked from the polish's other local optima
(jobs 49472 and 49487); its route was never stored, and the same kicks from
the current base find nothing better, so it stays unverified.
`docs/route-polish.md` has the catalog-wide numbers and the polish's earlier
measurements.
The sealed-corral check covers the reference's corral deadlock check, at the same
six-push cap; `docs/pruning.md` records how its scope was chosen and checked,
and what it measured. These were rejected, for soundness or by measurement:

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
- The reference's 9x9 pattern windows, sound but eligible on few cells: on
  random walks over the 34 capped Optimal boards they flagged 0.003% of the
  pushes the current prunes admit, all on one board (0.185% there). A
  whole-board pattern database per label group (2 to 5 boxes, at most 2^20
  entries) fits 11 capped boards, beat the assignment estimate by 2 at the
  start of only one, and gained under 0.5 moves on average along random walks
  on every board.
- The reference's wall-side region check: floods of up to nine cells along the
  walls, each holding two or three boxes and solved on its own, drop a push
  into a region that can no longer fill its goals. It is sound and pruned on
  49 of the 171 catalog runs, yet its capped Optimal lower bounds summed 0
  moves higher over 36 catalog and observed runs and 2 higher over 32
  production runs, with no new proof or route, while its scan on every push
  cost 8 to 13% more time in production and about 15% on the observed boards,
  mostly the huge and large ones where it never prunes.
- Tunnel macros, forced-push macros and the goal-commitment skip: the
  reference measured tunnel macros slower and ships them off, and none of the
  three prunes anything or tightens a bound under push A* with an exact
  keeper walk.
- Sokomind2's linear-conflict and interaction-boost terms, and goal cuts as a
  heuristic term: none is a lower bound, and the first two gave a false optimal
  proof and a false bound there.
- A table over an abstraction of a region past the exact tables' cell cap —
  Grand Hall's 99-cell hall — with its cells cut into zones, a state the
  per-zone count of each label's boxes and the keeper's zone, and moves that
  mirror the real ones at the cost of the cell they end on: sound by the
  room tables' own lemmas, and it built and passed the oracle checks, but
  freeing its zones from the hall part cost more per-push charge than the
  zone crossings it charged back — a box pushed many times within one zone
  or teleported out and in pays it little — so the keep test dropped it on
  every board tried, Grand Hall included (`docs/hall-abstraction.md`).
- Starting each expansion's assignment solve from the goal potentials its
  label group's last solve left, instead of from zero: the estimates stay
  exact and every counter matched, but it was slower in every mode (job
  48561, one c4-standard-32 node, alternating rounds): Optimal by 1.5% on the
  catalog at 1M states, 64 MiB and 10 s, and by 3.3% on the huge, large and
  open-field runs at 1M states and 256 MiB; Fast and Quality by 0.8 to 2.3%.
- Queueing a popped node again at its g and estimate plus the least walk
  excess over its pushes (the walk to the stand less the stand's charge), the
  reference's Tier 3.5a: sound, but partial expansion subsumes it. Every
  child's f is at least that key, so partial expansion queues the node again
  no lower. With partial expansion from the start, Grand Hall's bound at 1M
  states and 256 MiB rose from 235 to 241; with the recheck, to 236 (job
  48723).
- The stage ladder's sound accept checks as Optimal prunes (job 49446). Sink
  lines pruned nothing on Grand Hall and moved no catalog bound at 1M states.
  Sealed pockets raised 10 of those bounds by 1 to 3 but cost 37% more time on
  the catalog and 22 to 28% on Grand Hall, whose bound stayed 283 at 1 GiB and
  286 at 4000 MiB (cap lifted). Keeper-aware reach matching is already in the
  estimate, which is None exactly when the matching fails. `docs/pruning.md`
  has the numbers.
- Charging each box's turns, or any walk steps credited to a box, on top of
  the push count. Pushes of other boxes can carry the robot around a box:
  just after box i reaches y in direction d, the robot on y - d can push a box
  off y - d - d' in direction -d', then a box off y - d' in direction d, and
  stand behind i for direction d' without a walk step. Those moves are
  pushes that M already counts, so the charge counts them twice. Only
  splitting moves by the cell they end on, as the room tables do, prices
  walking soundly. On Grand Hall's 436-move route (job 49466) the gap to S at
  the start is 178: 134 is robot walking in the hall that the hall part does
  not charge (95 between boxes, 39 turning one), 20 is push slack and 24 is
  in the tables, and the route's goal assignment is the matching's.

Board text has one line per row: `O` is a wall, a space floor, `R` the robot,
`X` a box whose goals are `S`, and any other uppercase letter a box whose goals
are that letter in lowercase. `o`, `r`, `s` and `x` are reserved, because no box
carries `O`, `R` or `S` and `X`'s goal is spelled `S`; the parser rejects them
with their row and column, like any other unsupported symbol.
Boards are limited to 4096 cells and 32 boxes (all imported puzzles fit). Routes
are limited to 100,000 moves. Native requests cap at 30 seconds, 256,000,000 states
per arena (`sokomind_search::MAX_STATES`), 1024 MiB accounted search storage, and
one concurrent CPU job by default (see `SOLVE_CONCURRENCY` under Configuration),
and take 128 MiB when they name no budget. The search crate accepts budgets up
to 4095 MiB in the browser worker, the wasm32 ceiling, and 32768 MiB natively,
so the web app sends the full state cap and the budget its memory select names
(16 to 1024 MiB, 64 by default) to either engine. A state costs a 12-byte
record, 2 bytes per box, an 8-byte queue entry, and 8 to 16 bytes of index
table, so every budget the server, the worker and the web app can name binds
before the state cap: on the catalog's boards 64 MiB holds about 0.91-2.10M
states, 128 MiB 1.83-4.19M, and 1024 MiB 14.68-33.55M, and a run that fills its
arena reports a memory limit. A native budget past what holds the full state
cap — 32768 MiB holds it at every box count — instead ends a full arena at the
state limit, and the room bound's tables take what such a budget leaves. The
arena holds at most the smaller of the state cap and what the budget holds. Records and their queue entries grow in blocks of
2^16 as the search fills them, and the index table has two sizes: it starts at
2^16 slots (256 KiB), or its full size when that is smaller, and grows straight
to its full size once more than half full. None passes the budget, so a small
board allocates little of it. The grow fills the whole new table, once and early
in the search: at 1024 MiB it takes about 16 ms on Grand Hall natively, and
about twice that on a one-box board, whose table is twice as large. So once per
search a WASM worker slice runs that much longer or more, and a native solve
holds its blocking thread that long. WASM linear memory never shrinks and keeps
what a run frees: a run that grows its table can end up to 256 KiB past the
budget, for the freed first table, and one that ran the stage search up to
200,000 bytes (the route allowance) more, for the stage search's freed buffers.
The allocator's slack comes on top. It grows linear memory by a whole request,
rounded up to 64 KiB, even when the free space at the top of the heap falls just
short of it, so each growth can leave up to 64 KiB more there. A chunk's
record, box, and queue blocks can each miss before that space holds one, so up
to one record block plus 192 KiB can stay unused. Where that slack ends depends
on every earlier allocation, and a few hundred bytes more elsewhere can cost a
whole block: Grand Hall at 16 MiB ends 369 KiB past its ceiling, 128 KiB past the
budget.
An allocation refused below the budget also ends the run with a
memory limit. The search memory metric is that ceiling, computed from buffer
sizes at the arena's full limit (arena, queue, table, and per-cell flood,
deadlock, corral, dead-cell, and distance buffers) plus a fixed allowance for
the longest route and scratch buffers, not the bytes allocated so far, process
RSS, allocator overhead, WASM runtime, or frontend memory.
Deadline checks occur between bounded expansion batches; setup/reconstruction can
add latency.

## HTTP

* `GET /api/health` — `{status: "ok"}` whenever the API answers. The web app
  asks once at startup and shows "Native solver connected" when that reply
  arrives; otherwise "Browser mode" stays on show. Either way, both engines
  remain selectable.
* `POST /api/solve` — `{rows: string[], actions: "", mode: "fast"|"quality"|"optimal", time_ms: 5000, max_states: 256000000, memory_mib: 128}`.
  Only `rows` and `mode` are required; the values shown are the defaults. The
  limits start at 10 ms, 1 state, and 4 MiB and cap as under Layout and
  boundaries. The reply is `{status, route, moves, pushes, expanded, generated,
  reserved_bytes, elapsed_ms, proof, stats}`: `route`, `moves`, and `pushes`
  cover only the moves after `actions` and are null without a route; `proof` is
  null or `{kind, lower_bound, upper_bound}`, with no bounds for `unsolvable`;
  `stats` holds the search counters by name.

Progress never reaches the server: the browser keeps each puzzle's session and
best route in `localStorage`, and replays a stored best before it shows or
compares it, so a route altered in storage cannot claim counts it does not reach.

Every API error body is `{"error": "..."}` (NGINX's own 413 and 5xx pages are
not): 400 invalid input, `actions` that do not replay, out-of-range solve
limits, a memory budget too small for the board, or a solve position plus route
over the 100,000-move replay limit; 404 unknown endpoint; 405 wrong method; 408 a JSON
body not received within 10 seconds (on a direct run: NGINX reads each whole body
before it passes a request on); 413 a body over 128 KiB; 415 a missing or
non-JSON content type; 422 JSON of the wrong shape; 429 rate limited or solver
busy; 503 a failed search allocation; 500 a server bug. A solve checks its request
shape (400), then takes a solve slot (429 busy), then charges its rate limit
(429), and only then parses the board and replays `actions` (400).

The rate limit is per client address (an IPv4 address or an IPv6 /64):
`SOLVE_RATE_PER_MINUTE` solves per minute. A solve that finds every
`SOLVE_CONCURRENCY` slot taken gets 429 at once; nothing queues. A busy answer
does not count against the rate limit, nor does a solve with out-of-range limits
or an unknown mode. The binary has no
connection cap and no idle or header timeout (axum gives hyper no timer), and it
sends no security headers: NGINX, which must sit in front of it, bounds slow
clients and open connections and adds `X-Content-Type-Options: nosniff` and
`Referrer-Policy: same-origin` to every response. NGINX also limits `/api/` to 10
requests per second per address (burst 20) and answers excess with a JSON 429;
`/api/health` has its own budget of 2 per second (burst 10) instead, so polling it
never drains the `/api/` limit.

The binary serves only `/api`; any other path is a JSON 404. NGINX serves the web
app: unknown paths get the app shell, web-app files outside `/assets/` carry
`no-cache`, `/assets/` files carry `immutable` cache headers, and a missing
`/assets/` file is a 404 so a stale page cannot hang on a dead content hash after a
rebuild.

## Configuration

The server reads these environment variables. Empty counts as unset. The defaults
below are the server's and are written nowhere else: compose sets
`TRUSTED_PROXIES` and passes the budget and rate settings through from `.env`,
where they start commented out. The settings with a range clamp an out-of-range
value and replace a non-number with the default, each with a warning. A negative
value, or one too large for the server to parse, counts as a non-number. A
malformed `TRUSTED_PROXIES` entry stops startup.

| Variable | Default | Meaning |
| --- | --- | --- |
| `BIND_ADDR` | `127.0.0.1:3000` | Listen address (the Docker image sets `0.0.0.0:3000`) |
| `SOLVE_CONCURRENCY` | `1` | Concurrent native solves, 1..8; each reserves its own arena of up to 1024 MiB accounted search storage, so up to 8 GiB at 8. Compose sets the API no memory limit; a host that sets one must allow for this on top of the process |
| `SOLVE_RATE_PER_MINUTE` | `20` | Solves per client address per minute, 1..600 |
| `TRUSTED_PROXIES` | empty | Comma-separated IPv4/IPv6 addresses or CIDRs whose `X-Forwarded-For` is believed; empty trusts none |

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
Ubuntu and Windows and the rest, after `npm ci`, on Ubuntu. Every job names its runner image exactly (`ubuntu-24.04`,
`windows-2025-vs2026`) rather than a `-latest` label, and those that run Node
use the version `.node-version` pins, which is also the `engines` floor, so no
separate floor job exists. It builds the WASM package once and checks parity
against the native records `bench:check` wrote. Dependabot
(`.github/dependabot.yml`) opens weekly update pull requests for the pinned
actions and the npm and Cargo dependencies, each release after a 7-day
cooldown, and CI checks them like any other; the Rust toolchain, Node version,
Docker base tags, `wasm-bindgen`, and `@types/node` beyond patches move by hand.

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
npm run bench:appscale
npm run test:browser
```

`npm run validate` runs the same steps locally, in CI's order and with its single
WASM build, then prints a line per step: PASS or FAIL with its duration, or SKIP
with the reason. It skips every step after a failure; `--keep-going` runs them
all. `--quick` skips `bench:observe` and `test:browser`. It exits 1
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
false proof or bound, a hard regression (a longer or lost route, a lost proof,
or an Optimal status or bound that drops; rules in `scripts/bench-gate.mjs`),
or a changed catalog, never on timings. Soft findings (counts past tolerance,
more reserved bytes, a Fast or Quality status drop) and improvements the
baseline does not record yet are reported without failing; `npm run
bench:update` rewrites the baseline after review, so later runs are held to
the gains.
`test:parity` builds the WASM package and runs the native corpus again, at the
catalog example's defaults of 20,000 states and 64 MiB (CI passes it the records
`bench:check` wrote instead), then checks that the WASM search matches each
native run (status, counters, bounds, proof, diagnostics, and route) and replays
each route. Two Fast cases at the server's default 128 MiB, each in a fresh WASM
instance, check the same against native runs of their own, where the stage
ladder starts before the arena fills: on gen-v2-340048-b5d312a6 it gives up and
the plain search starts over, and on `huge` it finds Fast's route. Two last
cases, each in a fresh WASM instance, check that a search's memory grows with
its use, not its budget, and that one that fills its arena grows by at most its
memory ceiling plus 2 MiB.
`bench:observe` runs four hard catalog boards in every mode at a fixed 1,000,000
states and 64 MiB, three times each: an observation size, recorded in
`benchmarks/observe-reference.json` and far below the state cap so a run stays
short (`--states` and `--memory` override it). It fails only on a false proof
or bound, a nondeterministic run, or a crash. `bench:check`'s 20,000 states lie
below Fast's stage point, so a gate run that stages does so at its full arena;
`bench:observe`'s runs stage before the arena fills, as production's do.
`bench:appscale` runs a board set in every mode at the app's own declaration —
the 256,000,000-state ceiling the page always sends, with its default 64 MiB
and 5 s budgets — one corpus run per board, each under a wall budget. Machine
speed changes which routes and bounds a time-capped run reaches, so it judges
only what speed cannot: every search answers its deadline (a run past four
times its budget, or a corpus run that never finishes, fails — the walk term's
old unsliced LP once held a 70 ms solve for minutes at exactly these limits
while every fixed-size gate stayed green), every status is one the wire names,
and the false-proof invariants hold at scale. `--full` runs every catalog
board, as the weekly CI job does.
`test:browser` runs the Playwright specs
in `web/tests/browser`, with `/api` stubbed, against the built app, so
`npm run build` comes first; install the browser once with
`npx playwright install chromium`. CI and `validate` run it with `CI=true`, so
a leftover `test.only` fails the run, and Playwright starts its own preview
server, failing if port 4173 is already taken rather than reusing that server.
One spec leaves `/api` real: the native-engine solve runs against the suite's
second web server, the Rust server on 3000 the preview's proxy targets, whose
health endpoint is its readiness probe.

CI's `deploy` job checks the deployment files, builds both images, and
smoke-tests the stack they make; it pushes nothing. The Dockerfile's
`NODE_VERSION` must equal `.node-version`, and its `rust-base` tag and
`Cargo.toml`'s `rust-version` the version `rust-toolchain.toml` pins;
`docker compose config` must accept `compose.yaml`, the web stage's NGINX image
must accept `deploy/nginx.conf` (`nginx -t`), and both image targets, `server`
and `web`, must build. It then starts the stack as Full stack with Docker
describes, with `.env` copied from `.env.example` and a `compose.override.yaml`
that runs the two images just built, and checks it through NGINX at
`127.0.0.1:8080`: `/api/health` answers `ok`, an optimal solve of the
ultra-tiny board returns its one-push route, `/` serves the app shell, and the
WASM file is served as `application/wasm`. `docker compose down` removes the
stack afterwards, also after a failure, whose container states and
logs are printed first. The job runs when a push or pull request changes a file
the images, their configuration, or the smoke test come from (the Dockerfile,
`.dockerignore`, `compose.yaml`, `.env.example`, `deploy`, the workflow, and
the Rust and npm manifests, locks, and version pins; the workflow lists them
all), when the base to compare against is missing or can no longer be fetched
(the push that creates a branch, or a force push), on every manual run, and
weekly (Mondays at 06:17 UTC), when it and the app-scale sweep are the only jobs, because base images
change upstream while the repository does not. A change to other sources alone
waits for the weekly or a manual run.

Rust tests live in `crates/core/tests` (`parse.rs`, `game.rs`) and
`crates/search/tests`, besides unit tests in the search and server sources; the
server's router tests are in `crates/server/src/tests/router.rs`.
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
