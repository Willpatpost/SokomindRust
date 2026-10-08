# Stage ladder design notes

The stage ladder (`crates/search/src/stage.rs` and `stage/`) is the bounded,
incomplete search that Fast and Quality hand a routeless search to. The module
docs there say what each part does. This file records why the design looks the
way it does: what accept may claim, where each budget comes from, and the
measurements behind both, with the SLURM job that produced each number.

## The probe

The ladder was designed and measured as a standalone Rust prototype, the P4b
stage probe, built on the solver at main `6358eb5` and kept outside the
repository. The production port makes the probe's decisions in the probe's
order, so the measurements below hold for it. Where a code comment says "as in
the probe" or names a probe function, it means that prototype: the choice keeps
the measured behavior.

The port was checked against the probe. On the benchmark gate (job 46982, rev
`eb49e49`), the 16 boards the ladder routes in Fast and Quality have the probe's
moves, pushes and ladder work, and no Optimal record changed. On Grand Hall
(`huge`) at 64 MiB the port finds the probe's 812-move, 268-push route with the
same 617,192 units of work (P3, job 47024).

The port plan listed 30 differences from the probe. Nearly all restate the
probe exactly: an O(cells) floor structure instead of per-cell floods, `u32`
goal masks, fixed preallocated buffers, goal columns in the heuristic's order,
pocket discovery on Corral's stamps, and the like. These change more:

- Accept drops three of the probe's tests (see [Accept](#accept)).
- The work is cut into resumable units, so a caller can stop between any two.
  Only the slicing changes, not the decisions or their order.
- The ladder runs inside the arena of a plain Fast search that has reached
  its stage point (see [When the ladder runs](#when-the-ladder-runs)), and
  its counters add to the plain phase's. The probe started it on a fresh
  engine. Its first step clears the arena down to a live copy of the start,
  exactly as a fresh engine holds it, so its run depends only on the board,
  the limit and the search's weight, not on how far the plain search got.
- A checks budget bounds the units that are not expansions. It changes a run
  only when it binds, and on the measured runs it never does (see
  [Budgets](#budgets)).

Three changes came after the probe. The stage point moved from a full arena to
the arena's capacity at 64 MiB, the same point at 64 MiB and below, and then
to 32,768 states (`STAGE_AT`, job 49403; see [When the ladder
runs](#when-the-ladder-runs)). A reseed on a sparse table now clears only the
records bound since the last clear, not the whole table (`UNBIND_RATIO` in
`arena.rs`): the result is the same (job 47932 found no mismatch), only the
time changes. And in Fast the work, and with it the backjump limit, grows with
the caller's state limit (job 49443; see [Work](#work)).

## Accept

A stage commits the first rise, a state above its frame root's level, that
passes accept. Accept is a heuristic filter, not a necessary condition. Three
of its checks are sound, and two are not:

| Check | Probe test | Rejects a rise when | Sound |
| --- | --- | --- | --- |
| Ban | | it holds a box on a cell, or has a signature, that the ladder banned after a failed stage | not a solvability claim; the ladder's memory of its own failures |
| Lane | lane | a goal it leaves unfilled had an open lane at the frame root and has none now | no |
| Sink | sink | the boxes on a sink line no longer fit its goals in order | yes: `deadlock/goals.rs` |
| Line | e | an unsettled box has no push the keeper can make while settled boxes, sink-line boxes and the box itself block its walk | no |
| Matched | f | the boxes cannot each reach a distinct goal of their label, keeper-aware | yes: `deadlock/goals.rs` |
| Pockets | g | a pocket's member boxes can neither let the keeper in nor fill their goals | yes: `corral.rs` |

Lane compares the rise with the frame root, so it rejects the states between a
route closing every lane into an unfilled goal and reopening one. Line rejects
pushes 78 to 80 of a known 503-move route of `huge`, which
`checks::tests::route_replay` pins. So the ladder serves Fast and Quality only,
and nothing it finds may feed a proof, a bound or a prune of the exact search.
The three sound checks were measured as Optimal prunes on their own and
rejected (job 49446, `docs/pruning.md`).

How the checks were measured on known routes, where a rejected state is a
false reject:

- On the 40 catalog routes plain Fast finds at 64 MiB (P4a, job 46757), Sink,
  Matched, Pockets and Line rejected no state. Lane depends on the frame root,
  so a fixed route cannot test it.
- On three known routes of `huge` (503, 626 and 893 moves), Sink (job 46746),
  Matched (job 46750) and Pockets (job 46751) rejected nothing. Line rejected
  the 3 states above, all on the 503-move route (job 46747).

### Choosing the set

P4a (job 46757) removed one test at a time from the probe's full set on the 17
catalog boards plain Fast does not route at 64 MiB. The full set routed 9 of
them:

| Set | Routes |
| --- | --- |
| all | 9 |
| without Lane | 10 (+3, -2) |
| without Sink | 9, every run bit-identical |
| without relaxed push reach (a) | 9 |
| without keeper mobility, sink lines ignored (b) | 9, every run bit-identical |
| without pull coverage (c) | 9 |
| without Line (e) | 8, loses `huge` |
| without Matched (f) | 7 |
| without Pockets (g) | 7 |
| Sink, Matched and Pockets only | 6 |
| none | 3 |

Single removals do not prove a joint set, so P4b (job 46758) ran six candidate
sets together on those 17 boards at 64 MiB and on the 23 boards plain Fast
misses at the benchmark gate's 20,000 states. Per set: routes, total ms on the
routed boards, total ms on the routeless ones, and the most work any solve
used.

| Set | 64 MiB | Gate |
| --- | --- | --- |
| all | 9 / 3,051 / 10,052 / 983,583 | 16 / 2,594 / 8,179 / 589,817 |
| Lane, e, f, g | 10 / 3,664 / 9,080 / 983,583 | 16 / 1,849 / 8,086 / 527,793 |
| e, f, g | 7 / 2,169 / 14,007 / 815,495 | 16 / 5,548 / 7,285 / 941,182 |
| **Lane, Sink, e, f, g** | **10 / 3,609 / 7,905 / 983,583** | **16 / 1,818 / 7,239 / 527,793** |
| Lane, a, e, f, g | 9 / 3,074 / 10,914 / 983,583 | 16 / 2,498 / 8,151 / 589,817 |
| Sink, e, f, g | 7 / 2,174 / 13,083 / 815,495 | 16 / 5,383 / 6,303 / 941,182 |

The production set, Lane, Sink, Line, Matched and Pockets, routes the same
boards as the set without Sink at both limits, the most of any set. It spends
less time than that set where it finds none, and at 64 MiB the least of any
set. Sink lets a dead root fail fast: the ladder gives up on gen-v2-340048 in
430 ms at 64 MiB instead of spending its whole work budget, 1,492 ms. Without
Lane the ladder loses `huge`, gen-v2-340053-466f0008, gen-v2-350108-80cb0432
and gen-v2-370002 at 64 MiB, and two boards at the gate. It routes
gen-v2-340046 at both limits and gen-v2-340053-6d91 at the gate, which no set
with Lane routes.

Three of the probe's tests were dropped:

- Relaxed push reach (a) and pull coverage (c) added no route on any board.
  Of the tests checked on plain Fast's routes, they were the only ones to
  reject a state, on five boards (theme-kitchen, large, theme-parking,
  expert-maze and gen-v2-320050-4eb88a2d). With (a) in the set, `huge` gets a
  shorter route (738 moves), but the ladder loses gen-v2-340114 at 64 MiB.
- Keeper mobility that ignores sink lines (b) rejects a subset of what Line
  rejects, and dropping it left every run bit-identical.

## When the ladder runs

From the start, with every probe test (P4a, job 46757), the ladder routed 46
of the 57 catalog boards at 64 MiB, plain Fast 40, both 37. It missed 3 boards
plain Fast routes: on theme-kitchen and gen-v2-310081 the start dies at the
last rung, and on large the backjumps run out. So a staging search
runs plain first and hands over at its stage point: when its arena holds
`STAGE_AT` (32,768) states, or when it fills, whichever comes first.

A fill alone would come later as the budget grows, so the stage point was
first the arena's capacity at 64 MiB (`FILL_MIB`): plain Fast at 1 GiB (job
47772) routes every catalog board it routes at all within 1,211,120 states,
below that board's 1,317,310-state capacity at 64 MiB, and none of the other
17 within 8M states, 7.5 to 18.3 s of filling each. But the ladder's first
step clears the arena to the start, so under a given limit its run depends on
the stage point only through its work budget, and 32 per state reaches 2^20,
the most its states earn, at 32,768 states; above that only the limit raises
it, in Fast (see [Work](#work)). From 32,768 states on, a later stage point
only adds plain search before the ladder on every board that stages either
way: about 1M states at 64 MiB, half a second on `huge`.

Job 49403 (exp/fast-stage against main d3f6b52, probe-only knobs) ran Fast on
all 57 catalog boards at 60M states and 30 s, at 64 and 256 MiB, three times
each, with the stage point at the 64 MiB capacity (then production) and at
32,768, 131,072 and 524,288 states. With no knob set the branch matched main
in all 171 compared runs. Times are each board's median, summed:

| Fast, 57 boards | 64 MiB capacity | 32,768 | 131,072 | 524,288 |
|---|---|---|---|---|
| routed / staged / ladder-routed | 50 / 17 / 10 | 50 / 22 / 15 | 50 / 22 / 15 | 50 / 19 / 12 |
| moves, routed boards | 6,590 | 6,635 | 6,635 | 6,566 |
| time at 64 MiB | 27.8 s | 16.0 s | 17.7 s | 24.4 s |
| time at 256 MiB | 60.7 s | 41.5 s | 43.4 s | 50.4 s |

- At 32,768 no route was lost or gained, and no board was slower by more
  than 10% and 20 ms; 19 boards were faster by more than 10% at 64 MiB, 29 at
  256 MiB. Each of the 34 board and budget cases that staged under both stage
  points ran the identical ladder. `huge` took 365 ms instead of 896 ms at
  64 MiB, with the same 812-move route.
- Five boards that plain Fast routes after 146,000 to 1.2M states (gen-v2
  320041-027a, 320050-4eb8, 320050-7831, 320050-ece6 and 320081-b59c) stage
  and take the ladder's route instead, in 22 to 39 ms instead of 141 to
  1,043 ms: 141, 230, 181, 170 and 153 moves instead of 143, 173, 188, 187
  and 139.
- Staging at 524,288 was slower than at the capacity on the 7 boards whose
  ladder gives up, which pay both plain searches, by up to 0.5 s each.
- Quality at 5 s kept every route and lost no move on any board, at both
  budgets: 5,050 moves instead of 5,058 at 64 MiB, 5,143 instead of 5,156 at
  256 MiB (better on `huge` and gen-v2-360372), and its first routes summed
  to 3.2 s instead of 15.7 s at 64 MiB. `huge` at 30 s ends at 436 moves
  under every stage point, its first route at 373 ms instead of 900 ms.

The cost falls on a board whose plain route lies past 32,768 states, and
before the old stage point, and whose ladder gives up: it pays the ladder
first, 0.3 to 1.3 s on the 7 give-ups at 64 MiB. No catalog board is one.

Fast's time now lies in the ladder and in the give-ups, not in plain search.
At 32,768 and 64 MiB, with the work capped at 2^20, the 50 routed boards took
3.1 s, 2.7 s of it in their ladders, and the 35 that plain Fast routes took 29
ms together, none more than 14 ms (before, the 40 it routed took 2.7 s, up to
1.0 s each). The 7 give-ups took 12.9 s, 6.4 s of it in their ladders and the
rest filling an arena that routes none of them. So a plain-search speedup,
such as the tunnel and goal-room macros the reference tried, has at most 0.4 s
to gain on the routed boards, and it does not route a board the ladder gives
up on.

Below `MIN_STATES` (4,096) states at the stage point the ladder declines, so a
small limit keeps the plain search's behavior. At 4,096 its states earn it
131,072 inserts.

## Budgets

### Work

`work(states, limit) = max(min(32 * states, 2^20), limit)` inserts for a
search staged with `states` records under a limit of `limit` states in Fast.
Quality passes a limit of 0, which keeps the first term (see the end of this
section). At the gate (20,000 states) the solves of the production set need at
most 26.4 times the limit (gen-v2-340053-466f0008, 527,793). The next are
gen-v2-350070 (323,274, 16.2 times), gen-v2-340114 (196,557) and `huge`
(192,499, 9.6 times), the rest under 84,000 (P4b). So 32 keeps all 16 gate
routes with margin. Under any limit above 32,768 a search stages at
`STAGE_AT`, 32,768 states, where 32 per state reaches 2^20, and the largest
solve at 64 MiB used 983,583, under 2^20. Up to a limit of 2^20 the first term
is the larger, unless the memory budget fills the arena with fewer states than
the limit, so the gate and the observe runs (1M states) keep the work they had
when 2^20 was a cap.

The factor bounds a give-up at small limits. At large ones the cap bounds it
in Quality, and in Fast the caller's deadline does. The probe ran the gate at
2^20, where six boards spent the whole budget, 0.83 to 1.43 s each and 7.1 s
in all. At 32 times 20,000 the gate budget is 640,000, about 61% of that. The
production gate give-ups take 0.18 to 0.91 s each (P3).

With every probe test, keeping all 16 gate routes needed 29.5 times the limit
(P4a), so the formula belongs to this accept set.

More work does route more boards, but slowly. Job 49403 ran Fast on the 7
boards whose ladder gives up at 64 MiB, `huge` and gen-v2-340114, staged at
32,768 states at 256 MiB and 1 GiB, with the work budget at 2^20, 2^22 and
2^24 and the backjump cap (then `MAX_BACKTRACKS`, 64) at 64 and 512. 2^22
routed nothing more. 2^24 routed gen-v2-350108-b138 (304 moves, its ladder 5.7
to 5.8 s, using 7.9M inserts) and, with 512 backjumps, gen-v2-370193-8887 (379
moves, 16.0 to 16.4 s, 13.5M inserts and 249 backjumps). On gen-v2-340048-b5d3
the ladder backjumps to the start and fails its last rung there after 286,444
inserts, whatever its budget; on 340053-6d91 it does so after 3.1M with 512
backjumps. gen-v2-340046-c9f5, 350001-a996 and 360313-1615 spent all 2^24
inserts, 14 to 25 s, and gave up, where at 2^20 each gave up within 1.4 s. So
no fixed cap above 2^20 was taken: two routes are not worth a ladder ten to
twenty times as long on every board it cannot route, whatever time the caller
allows.

Instead the second term lets the work grow with the caller's state limit. The
search has no clock, so under a large limit a ladder runs until it routes,
gives up or meets the deadline its transport sets (5 s by default in the
server and the web). Job 49443 (perf/ladder-limit 3005e26 against main
26cd2df) ran the catalog at 60M states, with that work in both modes:

- At the gate (114 runs) and in the observe runs (8), every record matched
  main's.
- Fast at 5 s routed the same 50 boards with the same moves at 64, 128 and
  256 MiB. Its give-ups ran to the deadline instead of ending on memory after
  1.3 to 4.6 s, except 340048-b5d3 at 64 and 128 MiB, which ended on memory
  as before: 35.0 s in all instead of 18.3 s at 64 MiB. At 256 MiB five of
  main's seven give-ups already ran to the deadline.
- Fast at 30 s routed 53 boards instead of 50 at every budget:
  gen-v2-350108-b138 (304 moves, 5.9 to 6.1 s), 360313-1615 (569 moves, 15.9
  to 16.3 s) and 370193-8887 (379 moves, 17.1 to 17.3 s). 340046-c9f5 and
  350001-a996 ran to 30 s, and 340053-6d91 ended on memory, as before but
  later.
- Quality at 30 s, on the 7 give-ups and `huge`, routed 4 boards instead of 1
  at 64 MiB and 5 instead of 2 at 256 MiB: b138 in 232 moves, 1615 in 277 and
  8887 in 245, at 6.4 to 18.2 s.
- Quality at 5 s lost gen-v2-340053-6d91 at 128 and 256 MiB. Under the cap its
  ladder gives up early and the restart routes the board in 95 and 96 moves
  within 3.1 s; with the larger work the ladder ran to the deadline.

So only Fast's work grows with the limit (`long_ladder` in the engine's
`Policy`). At the production defaults, 128 MiB and 5 s, Fast gains no route
(b138 needs about 6 s) and loses none, and a caller who allows more time gets
the three routes. Quality passes a limit of 0 and keeps the capped ladder and
its route on 340053-6d91, and gives up its 30 s gains.

### Checks

Expansions are bounded by work, since every queued record comes from a
counted insert or a reseed. Every other unit (rank, order, filter, movers,
accept, commit and fail, each O(cells) or one reseed) costs one check out of
`CHECK_FACTOR * work`, so the ladder's whole time is bounded at every board
size. The probe had no such bound: its accept checks and pocket pops were not
counted. The largest ratio measured is 0.575 checks per unit of work
(gen-v2-340053-466f0008 at the gate; `huge` 0.136), on the catalog at 64 MiB
and the gate (P3, job 47024), so a factor of 4 never binds there.

### Caps and limits

The focus sizes (`FOCUS`), the movers limit (`MAX_MOVE`), the rung caps
(`CAPS`) and the rise and ban limits (`MAX_RISES`, `MAX_BANS`) are the
probe's values. Every measurement here used them, and none has been tuned
since. So is the backjump limit, 64 (`MIN_BACKTRACKS`), at a work of 2^20 or
less. Above that, which only Fast's work reaches, a ladder may backjump once
per 16,384 inserts of its work (`WORK_PER_BACKTRACK`), so the limit grows with
the work rather than ending a long ladder first (see [Work](#work)).

### Counter width

The search's counters are `u32`. Reseeds happen only at `begin`, at each
commit and at each restart. Rung-0 starts are `begin`, each commit and each of
at most `B` backjumps, and each climbs at most two rungs, so reseeds are at
most `3 * (1 + commits + B)`. Commits are at most 65 level rises per stretch
between backjumps, times `B + 1` stretches, so reseeds are at most
`198 * (B + 1)`: 12,870 at the 64 backjumps of a work of 2^20 or less, and
725,274 at the 3,662 of Fast's work under the 60M limit. A staged search's
records are then two arenas of at most `L + 1` records for a limit of `L`
states, the ladder's work, and one live copy of a root per reseed: at most
`2 * (L + 1) + work + 198 * (B + 1)`, about 180.7M for Fast at the 60M limit,
far below `u32::MAX`. The checks budget, 4 per insert of work, is at most 240M
there. The engine test `staged_generated_bound` checks the bound on `huge`.

## Cost

From P3 (job 47024, rev `e4172fe`):

- `Ladder::new` takes at most 108 µs on the catalog (gen-v2-360313, 546
  cells), and 3.1 to 3.3 ms on a board of 4,096 cells and 32 boxes, the most
  the solver accepts. It runs once per staged search.
- Every unit but a reseed takes at most 35 µs at 64 MiB and 55 µs at the
  gate.
- A reseed then filled the whole table: 0.6 to 1.03 ms at 64 MiB, about 2.0 ms
  at 128 MiB and 5.6 ms at 256 MiB. The targeted clear that followed walks
  only the bound records on a sparse table. A walk costs 20 to 25 ns per bound
  record and a fill 0.1 to 0.25 ns per slot, so they break even at 165 to 226
  slots per bound record (job 47932). On `huge` at 1 GiB, a table of 2^25
  slots, the slowest reseed takes 2.9 ms instead of a fill's 8.2.
- `huge` gets 812 moves (268 pushes) at 64, 128 and 256 MiB, with work
  617,192 and 83,696 checks.

## Reproducing

The probes and their logs are not in the repository. The figures above are
from the 57-board catalog, one thread, on the jobs named. To recheck a claim
on the current tree, run the catalog example
(`crates/search/examples/catalog.rs`) at the stated limits. The benchmark gate
(`benchmarks/catalog-baseline.json`) records the 16 gate routes.
