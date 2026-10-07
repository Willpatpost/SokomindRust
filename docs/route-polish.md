# Route polish

Quality mode's search shortens its route by finding a better one in the arena.
On large boards it stops short: on Grand Hall (`huge`) the staged route of 812
moves came back from the search unchanged, because the restart's arena fills
long before weight 3 reaches anything shorter. The polish takes it to 475
inside the production limit of 5 s, and to 436 given about 35 s (see
Measured). Much of such a route's length is walk:
the keeper crosses the board to push one box a step, then crosses back. The
polish attacks that directly. After the search, it rearranges when each box's
pushes happen, which no arena-bound search over whole states can afford on a
board this size.

The polish is route-only. It never bounds or proves anything, so Exact never
runs it, and Fast keeps its first route as found (`Policy::polish`, with const
asserts in `engine.rs`). The code is `crates/search/src/polish.rs`.

## When it runs

A Quality search polishes when it ends on its own with a route: its queue
empties, its arena fills at the state or memory limit, or a goal stops it. The
engine rebuilds and replays the incumbent's route, frees the arena's records,
queue and table (`Arena::release`), and hands the route and the status it
stopped on to the polisher. From then on, one pop of `Search::advance` is one
polish unit, and the status stays `Running` until the polish ends. It then
becomes the status the search stopped on.

A long search also polishes its route before it ends, in episodes. When the
incumbent last improved at `e` expansions and the search reaches
`e + max(e, 2^16)` (`EPISODE_FLOOR`) without improving it again, the engine
rebuilds the incumbent's route and hands it to a polisher, keeping the arena,
its queue and its table. Pops then go to the polish, as at the end, and once
the polish ends the search goes on where it stopped. An incumbent gets one
episode at most, and each new one schedules its own. The engine keeps the
episode's best route while it is shorter than the incumbent. It never bounds
the search, though, and the end polish still runs on the incumbent, with the
whole share. So, pop for pop, the search and its end polish run as they
would with no episode, and the result is the shorter of the best episode
route and the end polish's route, never longer. Only a time limit sees the
difference: an episode's time is time the search does not get. The trigger
counts expansions, not time, so an episode starts at the same point in any
slicing, natively and under WASM. The floor keeps episodes off short
searches: none starts in the catalog at CI's 20,000 states.

A caller's stop (`TimeLimit` or `Cancelled`) ends a running polish, an
episode or the end polish. The search keeps the shortest route found so far. A search stopped by the caller before it
ends on its own is not polished. The caller asked for the answer now, so the
polish does not run then.

## A job

A job frees one box, or two (see Pairs), and keeps every other box's pushes.
The runs free one box at a time, cycling through the slots in the run's order
(see Runs). A job for the freed boxes:

1. Replays the route and records every push of the other boxes as a kept event:
   the keeper's stand, the cell the box left and its new cell, and the
   direction. Let `m` be their number. The job also stores the other boxes'
   cells after each count of events, and the cells the freed boxes ended on,
   which are their goals.
2. Runs A* over nodes `(k, at, tag)`. Here `k` of the `m` kept events are done,
   the freed boxes stand on `at`, and the tag fixes the keeper. The keeper
   stands either where the last kept event left it (tag 0) or behind a freed
   box after that box's own push in one of four directions (tags 1 to 8).
3. From a node, two kinds of move follow, each after the shortest walk to its
   stand with every box in place:
   - Kept event `k`, unless it would push a box onto a freed box. It costs the
     walk plus 1.
   - One of a freed box's own pushes, onto a cell from which the freed boxes
     can still reach their goals (a finite push bound, see Heuristic). It also
     costs the walk plus 1.
4. The goal is `k = m` with the freed boxes on their goals. Two freed boxes of
   one label may also end on each other's goals: either box on either goal
   solves the board just as well. The bound is the current route's length,
   and any node with `g + h` at or above it is dropped. A goal pop is
   therefore a strictly shorter route.

Within this model, which keeps the other boxes' pushes and their order, a job
finds the shortest route. That route is rebuilt by re-flooding each step's
expansion and replayed under the game rules. It is then cut at its first solved
state, since a box can reach its goal before another box's last push. It
replaces the route only if it is shorter. A job that finds nothing, runs out of
queue, or cannot start (a freed box never moves, or the job does not fit in
memory) gains nothing. A run ends when a full cycle of single-box jobs, one per
box, has gained nothing. A gain restarts that count, so a run ends only at a
local optimum under single-box rescheduling.

A job's nodes go in a hashed store, keyed by `k`, the freed boxes' cells and
the tag, so a job holds only the nodes its A* reaches. A pair's node space is
the single box's times the second box's cells, and a dense array over it would
not fit on a large board.

## Runs

Where a run ends depends on the order it visits the boxes in. Each job
reschedules its box around the route the earlier jobs left, so another order
takes another path, to another local optimum. On Grand Hall, its 272 box
orders take the same 812-move route to anything from 458 to 557 moves (see
Measured).

So the polish makes several runs. Each starts from the route the search handed
over, the seed, and visits the slots in its own order: a first slot and a
stride, so the run's `i`-th job takes slot `first + i × stride`, modulo the
box count `n`. A stride coprime to `n` visits every slot once in any `n` jobs
in a row. Run 0 takes the slots in order (first slot 0, stride 1). Later runs
take the coprime strides in turn, then move the first slot on, so the first
`n × φ(n)` runs all differ. The polish makes `RUNS` (8) runs, or one per such
order on a board with fewer: one run for one box, two for two, six for three.

The polish keeps the shortest route any run reaches, and a later run's route
replaces it only when strictly shorter. It ends when every run has ended. A
caller's stop ends it sooner and keeps the best route so far, so under a time
limit the polish makes as many runs as fit.

## Pairs

Some gains need two boxes to move at once. A single-box job keeps every other
box's pushes, and their order, so it cannot let two boxes that block each other
trade order. It also keeps its box's goal, so two boxes of one label cannot
trade goals. Once every run has ended, the polish frees boxes in pairs.

The pair phase works on the best route any run reached. It takes every pair,
those whose boxes end that route nearest each other first, by Manhattan
distance, ties in slot order (`pairs`), so under a time limit the nearest
pairs, the likeliest to block each other, go first. The phase frees them in
that order, one pair job each, and cycles. Gains also come from pairs that end
far apart: five of the eight pairs that shortened Grand Hall's route ranked
beyond the nearest 16 (see Measured). A pair's node space is a single box's
times the second box's cells, so a pair job makes at most `PAIR_POPS` (2^20)
expansions, and gains nothing if it runs out first. Without that cap, one pair
job on a large board could run longer than all the single-box jobs together. On
Grand Hall no pair job reaches it. An expansion takes about a microsecond, so
the longest pair job there took about a second.

When a pair job shortens the route, every box may now be rescheduled better
around the new one. So a full cycle of single-box jobs runs before the next
pair, restarting on each gain, as in a run. The polish ends when a full pass
over the pairs, with any single-box cycles a gain sets off, has gained nothing.

## Heuristic

Write `d(p, g)` for the lone-box push distance from `p` to goal `g`, with the
keeper anywhere (`Heuristic::distance`). For one freed box on `p` with goal
`g`, let `D(at) = d(p, g)`. For two, on `p1` and `p2` with goals `g1` and `g2`,
let `D(at) = d(p1, g1) + d(p2, g2)`, or, when they share a label, the smaller of
that and `d(p1, g2) + d(p2, g1)`. Write `W_k(q)` for the walking distance from
`q` to kept event `k`'s stand, with the other boxes after `k` events in the way
but not the freed ones. Then let `T_k` be the sum, over the events from `k` on,
of `W_j` from where event `j - 1` left the keeper, with `T_m = 0`. The job's
heuristic at `(k, at)`, with the keeper on `q`, is:

```
h = (m - k) + max(D(at), W_k(q) + T_{k+1})     for k < m
h = D(at)                                      for k = m
```

**Admissible.** Every remaining kept event costs at least its push. Each push
moves one box, so the freed boxes need at least the sum of their lone-box push
distances to the goals they end on; with a shared label they end on one of the
two matchings, and need at least the cheaper. Between kept events the keeper
must get from where the last one left it to the next stand. Every move it makes
on the way, walk or own push, is a step in the grid with only the other boxes
as walls. So those moves sum to at least `W_k(q) + T_{k+1}`. Own pushes count
toward both terms, so they combine by max, not sum.

**Consistent.** Take an own push in direction `d`, after a walk of `w` from `q`.
It moves one freed box one cell, so each matching's sum drops by at most 1,
and so does the smaller of two sums: if the smaller sum before the push is
`s`, both sums after it are at least `s - 1`. The new keeper cell is at most
`w + 1` from `q` with the freed boxes ignored, so `W_k` drops by at most
`w + 1` by the triangle inequality. Now take kept event `k` after a walk of
`w`. `W_k(q) <= w`, and `T_{k+1}` is exactly the next node's walk term, so the
bound drops by at most `w + 1`. Each term of the max is consistent, so the max
is too. The walk layers ignore the freed boxes, so they are lower bounds
wherever those stand. The A* needs no reopenings, and its first goal pop is
the model's shortest route.

## Memory

The polish runs in the arena's share of the budget, the bytes
`Arena::reserved_bytes` charges beyond `fixed_bytes`, which the arena freed
before the polish. The seed and the best route come out of it first: each is
at most the seed's length, so a job's allowance is that share less twice the
seed's length, and in the pair phase less the pair list too. A job charges:

- its event list;
- the other boxes' cells per event count;
- one `u16` walk layer per event over every cell;
- the tails;
- the box scratch for its floods;
- 20 bytes per node in its store: an 8-byte key, a 4-byte cost (which also
  holds the tag of the node it was reached from, for the rebuild) and two
  4-byte slots, since the table is a power of two at most half full. The store
  doubles when it is half full, and its old table is still held while the new
  one fills, which the charge counts. It stops at 2^30 nodes, the most a queue
  key can name;
- 8 bytes per queue entry, and the queue's old buffer too while it grows.

A job that cannot hold its buffers, a store of `Nodes::MIN` (1,024) nodes and
`MIN_QUEUE` (1,024) queue entries does not start. A job whose store or queue
cannot grow within its allowance ends with no gain. Every allocation
is fallible (`try_reserve`), so an allocator refusal ends the job too, never
the process. The route, a job's found route and a solution's letters fit in
the fixed `ROUTE_ALLOWANCE` (two routes of `MAX_ROUTE` bytes). Besides the
seed and the best route, the polisher never holds more than two of them at
once. So `reserved_bytes` does not change.

An episode runs beside the live arena. Its allowance is the arena's share
less what the arena holds (`Arena::held_bytes`: its table and its record
blocks, at most the node limit's) and less the episode route the engine
keeps. The search waits while the episode runs, so the arena does not grow
until the episode has freed its buffers. An episode whose allowance cannot
hold a job's smallest buffers (`Polish::fits`) does not start. Between
polishes, the kept episode route is one of `ROUTE_ALLOWANCE`'s two routes:
it is shorter than the incumbent, so a solution returns it without
rebuilding the incumbent's route, and a polisher's start rebuilds only the
other. While a polisher runs, the kept route is charged to its allowance.

On wasm32 linear memory never shrinks, and the allocator may not fit the
polish into the arena's freed blocks, so linear memory can grow by up to the
arena's share again (see `Search::reserved_bytes`). The arena's later blocks
may not reuse an episode's freed buffers either, so each episode can add up
to the share again.

## Slicing and determinism

A unit is one of:

- starting a run: one copy of the seed;
- starting a job: two replays of the route, plus its allocations;
- one walk layer, which is one flood (layers are built from the last event
  down, so each tail adds the next);
- queueing the root;
- one A* pop, which is one flood and up to `1 + 4 × freed` relaxations (five
  for one freed box, nine for two);
- the final rebuild, which is one flood per step.

Every unit floods for itself, so no flood outlives a unit. Queue keys pack the
bit-flipped f, then the bit-flipped h, then the node id. Two entries never
share a key, so the pop order is fixed. A polish ends the same in slices of one
pop as in slices of 2^20, and natively as under WASM. An episode is due at an
expansion count and checked after each expansion, and `Search::advance`
returns as one starts, so episodes start and end at the same points in any
slicing too.

## Measured

Job 48673, on a c4-standard-32 node. It compares aa68600 (no polish) with
8e855e8, in Quality mode, using the catalog example, which replays every route.
The search is the same up to the handoff, so expanded and generated counts are
identical. The base run's moves and time are the route and time before the
polish.

- **Catalog, at the gate's 20,000 states and 64 MiB.**
  - 50 of the 57 puzzles have a Quality route at these limits. 22 of those
    routes are shorter, none is longer, and the other 28 are unchanged.
  - The largest gains:
    - huge: 812 → 485;
    - gen-v2-370002: 491 → 377;
    - gen-v2-350070: 309 → 231;
    - gen-v2-340053-466f: 175 → 123;
    - gen-v2-330146: 178 → 128.
  - Total Quality search time rose from 7.98 s to 8.87 s (+11%). The largest
    increases are gen-v2-370002 (+0.34 s) and huge (+0.29 s).
  - Push counts move both ways (huge 268 → 226, gen-v2-350108-80cb 99 → 103).
    The objective is moves, so a job may trade a push for a shorter walk.
- **Grand Hall.** A single run (the polish before restarts) ends at 485 moves
  (226 pushes) at every budget measured:
  - 20,000 and 1,000,000 states at 64 MiB;
  - the production limits (60M states, 5 s) at 64, 128 and 256 MiB.
- **Timing at the production limits, one run.**
  - The search ends at its memory limit at about 1.7, 2.3 and 3.9 s.
  - The polish ends about 0.3 s later, at 2.0, 2.65 and 4.13 s, inside the
    5 s limit.
  - `reserved_bytes` is identical with and without the polish.

Eight runs take Grand Hall to 475 (see Runs).

485 is a local optimum under single-box rescheduling. The reference's
rescheduler alone took its Grand Hall route from 789 to 503 moves. Its best
route, 476 moves, came from eight rounds of the same rescheduling on a chosen
916-move seed. Its window rewrite gained nothing on Grand Hall once
rescheduling had run.

**Box order.** Job 48683, on a c4-standard-32 node, polished Grand Hall's
812-move route in 119 box orders: first slots 0 to 16 with strides 1, 2, 3, 5,
7, 15 and 16, through a throwaway build that set the order (Quality, 60M
states, 64 MiB, no time limit). The runs ended between 462 and 547 moves, with
a median of 503. Slot order gave 485, two orders gave 462 (226 pushes), and 25
of the 119 ended below 485. Each run took 0.3 to 0.65 s after the search. The
same build polished three of the reference's routes as seeds, in 34 orders
each: its 503-move route reached 497 in every order, its 626-move route 465 to
509, and its 893-move route 472 to 551. This spread is why the polish makes
several runs (see Runs).

**Runs.** Job 48684, on a c4-standard-32 node, ran every box order through a
throwaway build that lifted the run cap and printed each run's end (Quality, no
time limit). Every run starts from the same seed and is deterministic, so the
best of the first K runs of one uncapped polish is what a cap of K returns.

- **Catalog, at the gate's 20,000 states and 64 MiB.** The 50 routes total
  5,358 moves after one run, 5,284 after 4, 5,242 after 8, 5,221 after 16 and
  5,170 after every order (up to 342 runs). Eight runs shorten 15 routes, by
  116 moves in all; every order shortens the same 15, by 188. The polish time,
  summed over the 50, grows from 0.73 s for one run to 5.5 s for 8, 10.8 s for
  16 and 126 s for every order.
- **Grand Hall, 60M states and 64 MiB, all 272 orders.** The best route is 485
  moves after 1 to 4 runs, 484 after 6, 475 after 8 to 64, 462 after 128 and
  458 after all 272 (run 197: first slot 12, stride 6). A run takes about
  0.24 s, and the median run ends at 502.
- **Grand Hall at the production limits** (60M states, 5 s, three samples at
  each budget). The 5 s limit left room for 17 runs at 64 MiB, 15 at 128 MiB
  and 10 at 256 MiB, and every sample reached 475 moves (228 pushes) at its
  eighth run.

So `RUNS` is 8: about 60% of the all-orders catalog gain for 7.5 times one
run's time, and 475 on Grand Hall at every production budget, where the next
gain needs 128 runs.

**Eight runs against one.** Job 48685, on a c4-standard-32 node, compares
1413d43 (one run) with b1d0636 (eight), in Quality mode, using the catalog
example.

- **Catalog, at the gate's 20,000 states and 64 MiB.** 15 routes are shorter,
  none is longer, and the other 35 are unchanged. Total Quality time rose from
  7.32 s to 12.14 s (+66%). The largest increases are huge (+1.6 s, 485 →
  475), gen-v2-370002 (+1.6 s, 377 → 362) and open-field (+0.5 s, no gain).
- **Grand Hall.** 475 moves (228 pushes) at 20,000 and 1,000,000 states and at
  the production limits, two samples at each budget. The search before the
  polish is identical: same expanded and generated counts, same
  `reserved_bytes`.
- **Timing at the production limits.** With one run the polish returned at
  about 1.6, 2.1 and 3.1 s at 64, 128 and 256 MiB. With eight it returns at
  about 3.2, 3.7 and 4.7 s, inside the 5 s limit, and a run with no time
  limit ends at the same point. A time limit stops the polish between units
  and returns its best route so far, so a slower machine gets fewer runs and
  a longer route, not a late answer.

**Pairs.** Jobs 48687 and 48688, on c4-standard-32 nodes, ran throwaway
builds that set the pair count and `PAIR_POPS` and printed each pair job
(Quality, using the catalog example). The search before the polish is
identical in every variant: same expanded counts, same `reserved_bytes`.

- **Catalog, at the gate's 20,000 states and 64 MiB.** Against the runs alone,
  every pair at 2^20 shortens 9 of the 50 routes, by 60 moves in all (5,242 →
  5,182), and lengthens none. The nearest 16 pairs at 2^18 shortened 3, by 10
  moves. 2^18 and 2^20 give the same routes here. Total Quality time rose
  from 10.3 s to 21.4 s. The largest increases are gen-v2-370002 (+2.5 s, no
  gain), huge (+2.3 s, no gain) and gen-v2-360372 (+1.1 s, 319 → 303). At
  20,000 states the arena's share is small, and every pair job on huge runs
  out of memory within 32,501 expansions.
- **Grand Hall, 60M states and 64 MiB, no time limit.** The runs end at 475.
  At 2^20, pair gains then take the route to 470, 468, 458, 456, 455, 451 and
  448, and the polish ends at 436 moves (222 pushes), 35.3 s after the
  search. The eight pairs that gained ranked 9th, 17th, 44th, 99th, 127th,
  7th, 15th and 34th of the 136 by distance. At 2^16 the polish ends at 463,
  at 2^18 at 457, where 29 of 235 pair jobs hit the cap. 2^22 gives the same
  route as 2^20: no pair job reaches 2^20, and the most any took was 958,053
  expansions, about 1.1 s.
- **Grand Hall at the production limits** (60M states, 64 MiB, two samples
  each). Every pair at 2^20 returns 475 at 5 s, since the first pair gain
  comes later, 458 at 15 s and 436 at 30 s. The nearest 16 at 2^18 ended at
  471 about 6.1 s in.
- **Larger budgets.** At 5 s the routes are 485 at 256 MiB, where the search
  ends late enough that only 7 runs finish, and 812 at 1024 MiB, where the
  search has not ended by 5 s, so nothing is polished (see When it runs).
  Pairs do not change these. Episodes, below, do.

**Episodes.** Job 48691, on c4-standard-32 nodes, ran a throwaway build
(289089e) that set when an episode starts, whether its route bounds the
search, and whether the search instead ends at that point, in Quality mode
with the catalog example, one sample each. Its base (every switch off) is
the pairs build.

- **Catalog, at the gate's 20,000 states and 64 MiB.** No episode starts, and
  every variant returns base's routes (5,182 moves in all).
- **Catalog, at 60M states and 5 s.** Episodes at `e + max(e, 2^16)`, with
  no bound, shorten 13 routes by 344 moves at 256 MiB and 15 by 976 at
  1024 MiB, and lengthen none. At 64 MiB they shorten 3 by 9 and lengthened
  one in that build, gen-v2-350070 (208 → 217): an episode started with no
  room left beside the full arena, did nothing, and marked the incumbent
  polished, so the end polish, which reaches 208, was skipped. Here an
  episode that cannot fit a job does not start, and the end polish always
  runs.
- **Grand Hall, at 60M states.** At 64 MiB the arena fills at 415,767
  expansions, before the first episode is due at 583,846, so the routes are
  base's: 470, 458 and 436 moves at 5, 15 and 30 s. At 256 MiB the route at
  5 s goes from 484 to 475; at 15 and 30 s it stays 458 and 436. At 1024 MiB
  it goes from 812 to 475 at 5 s, 468 to 458 at 15 s and 451 to 436 at 30 s.
  No episode on Grand Hall ends within 30 s, since its pair phase takes about
  30 s, so there the search never resumes.

Rejected variants:

- **The episode's route as the search's bound**, starting after `e/2` or `e`
  more expansions: at 64 MiB they lengthened 7 routes by 347 moves and 2 by
  11. The tighter bound pruned the search's own later improvements, which
  polish to shorter routes than the episode's.
- **Waiting `4e` more**: 3 routes shorter, by 19, at 256 MiB and 4, by 28, at
  1024 MiB, a small part of the gain after `e` more.
- **Ending the search there and polishing**: 6 routes longer, by 31, at
  64 MiB and 5, by 23, at 256 and 1024 MiB: routes the search still improved
  long after that point.

So the pair phase takes every pair, with `PAIR_POPS` at 2^20: no pair job on
Grand Hall reaches it, and the catalog's routes and time are the same as at
2^18.

## Left out, for now

These come from the reference's Quality route repair, or were considered here:

- **Polishing earlier.** Polishing the first route as soon as Fast finds it,
  or polishing when a time limit stops the search. The second needs the
  caller to leave time for it, and transports do not decide search behavior.
  Episodes polish during the search, but only once it stalls, and only in the
  memory the arena does not hold.
- **An h1-only fallback** (push distance only, with no walk layers) for jobs
  whose layers do not fit. It is weaker, and is worth adding only if
  measurement shows jobs being skipped at the budgets that matter.
- **Window rewrite**: re-searching short windows of the route with every box
  free. Where a job frees one or two boxes over the whole route, a window
  frees every box over a short stretch of it. In the reference it gained
  nothing on Grand Hall once rescheduling had run.
- **Goal swaps beyond a pair.** A pair job lets its two boxes trade goals
  when they share a label, but a box never takes a goal held by a box the job
  keeps. A three-box or label-wide reassignment would need a larger job.
- **Diverse seeds**: polishing several first routes and keeping the best. In
  the reference, the choice of seed alone moved Grand Hall between 476 and 509
  moves. Runs vary the box order instead, on the one route the search has.
