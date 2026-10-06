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

Two changes came after the probe. The stage point moved from a full arena
to `FILL_MIB`'s capacity, which is the same point at 64 MiB and below. And a
reseed on a sparse table now clears only the records bound since the last
clear, not the whole table (`UNBIND_RATIO` in `arena.rs`): the result is the
same (job 47932 found no mismatch), only the time changes.

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
runs plain first and hands over at its stage point: when its arena holds what
`FILL_MIB` (64 MiB) would hold for the board, or when it fills, whichever
comes first.

A fill alone would come later as the budget grows. Plain Fast at 1 GiB (job
47772) routes every catalog board it routes at all within 1,211,120 states,
below that board's 1,317,310-state capacity at 64 MiB. None of the other 17
boards routes within 8M states, 7.5 to 18.3 s of filling each. So staging at
the 64 MiB capacity loses no route plain Fast finds by 1 GiB, and the ladder
starts and runs the same at every budget from 64 MiB up.

Below `MIN_STATES` (4,096) states at the stage point the ladder declines, so a
small limit keeps the plain search's behavior. At 4,096 its work budget is
131,072 inserts.

## Budgets

### Work

`work(states) = min(32 * states, 2^20)` inserts for a search staged with
`states` records. At the gate (20,000 states) the solves of the production
set need at most 26.4 times the limit (gen-v2-340053-466f0008, 527,793). The
next are gen-v2-350070 (323,274, 16.2 times), gen-v2-340114 (196,557) and
`huge` (192,499, 9.6 times), the rest under 84,000 (P4b). So 32 keeps all 16
gate routes with margin. A search at 64 MiB stages with far more than 32,768
records, so the cap binds there, and the largest solve used 983,583, under
2^20.

The cap bounds a give-up at large limits, and the factor bounds it at small
ones. The probe ran the gate at 2^20, where six boards spent the whole budget,
0.83 to 1.43 s each and 7.1 s in all. At 32 times 20,000 the gate budget is
640,000, about 61% of that. The production gate give-ups take 0.18 to 0.91 s
each (P3).

With every probe test, keeping all 16 gate routes needed 29.5 times the limit
(P4a), so the formula belongs to this accept set.

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
(`CAPS`) and the rise, ban and backjump limits (`MAX_RISES`, `MAX_BANS`,
`MAX_BACKTRACKS`) are the probe's values. Every measurement here used them,
and none has been tuned since.

### Counter width

The search's counters are `u32`. Reseeds happen only at `begin`, at each
commit and at each restart. Rung-0 starts are `begin`, each commit and each of
at most `MAX_BACKTRACKS` (64) backjumps, and each climbs at most two rungs, so
reseeds are at most `3 * (1 + commits + 64) = 3 * commits + 195`. Commits are
at most 65 level rises per stretch between backjumps, times 65 stretches,
4,225, so reseeds are at most 12,870. A staged search's records are then two
arenas of at most `L + 1` records for a limit of `L` states, the ladder's
work, and one live copy of a root per reseed: at most
`2 * (L + 1) + work + 12,870`, about 121.1M at the 60M limit, far below
`u32::MAX`. The engine test `staged_generated_bound` checks the bound on
`huge`.

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
