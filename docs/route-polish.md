# Route polish

Quality mode's search shortens its route by finding a better one in the arena.
On large boards it stops short: on Grand Hall (`huge`) the staged route of 812
moves came back from the search unchanged, because the restart's arena fills
long before weight 3 reaches anything shorter. The polish takes it to 475
(see Measured). Much of such a route's length is walk:
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

A caller's stop (`TimeLimit` or `Cancelled`) ends a running polish. The search
keeps the shortest route found so far. A search stopped by the caller before it
ends on its own is not polished. The caller asked for the answer now, so the
polish does not run then.

## One box's job

The polisher works on one box at a time, cycling through the slots in the
run's order (see Runs). A job for box `b`:

1. Replays the route and records every push of the other boxes as a kept event:
   the keeper's stand, the cell the box left and its new cell, and the
   direction. Let `m` be their number. The job also stores the other boxes'
   cells after each count of events, and the cell `b` ended on, which is its
   goal.
2. Runs A* over nodes `(k, p, tag)`. Here `k` of the `m` kept events are done,
   `b` stands on `p`, and the tag fixes the keeper. The keeper stands either
   where the last kept event left it (tag 0) or behind `b` after `b`'s own push
   in direction `tag - 1`.
3. From a node, two kinds of move follow, each after the shortest walk to its
   stand with every box in place:
   - Kept event `k`, unless it would push a box onto `p`. It costs the walk
     plus 1.
   - One of `b`'s own pushes, onto a cell from which `b` can still reach its
     goal (a finite lone-box push distance). It also costs the walk plus 1.
4. The goal node is `(m, goal)`. The bound is the current route's length, and
   any node with `g + h` at or above it is dropped. A goal pop is therefore a
   strictly shorter route.

Within this model, which keeps the other boxes' pushes and their order, a job
finds the shortest route. That route is rebuilt by re-flooding each step's
expansion and replayed under the game rules. It is then cut at its first solved
state, since a box can reach its goal before another box's last push. It
replaces the route only if it is shorter. A job that finds nothing, runs out of
queue, or cannot start (the box never moves, or the job does not fit in memory)
gains nothing. A run ends when a full cycle of jobs, one per box, has gained
nothing. A gain restarts that count, so a run ends only at a local optimum
under single-box rescheduling.

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

## Heuristic

Write `D(p)` for `b`'s lone-box push distance from `p` to its goal, with the
keeper anywhere (`Heuristic::distance`). Write `W_k(q)` for the walking
distance from `q` to kept event `k`'s stand, with the other boxes after `k`
events in the way but not `b`. Then let `T_k` be the sum, over the events from
`k` on, of `W_j` from where event `j - 1` left the keeper, with `T_m = 0`. The
job's heuristic at `(k, p)`, with the keeper on `q`, is:

```
h = (m - k) + max(D(p), W_k(q) + T_{k+1})     for k < m
h = D(p)                                      for k = m
```

**Admissible.** Every remaining kept event costs at least its push, and `b`
needs at least `D(p)` more pushes. Between kept events the keeper must get from
where the last one left it to the next stand. Every move it makes on the way,
walk or own push, is a step in the grid with only the other boxes as walls. So
those moves sum to at least `W_k(q) + T_{k+1}`. Own pushes count toward both
terms, so they combine by max, not sum.

**Consistent.** Take an own push in direction `d`, after a walk of `w` from `q`.
`D` drops by at most 1. The new keeper cell `p` is at most `w + 1` from `q`
with `b` ignored, so `W_k` drops by at most `w + 1` by the triangle
inequality. Now take kept event `k` after a walk of `w`. `W_k(q) <= w`, and
`T_{k+1}` is exactly the next node's walk term, so the bound drops by at most
`w + 1`. Each term of the max is consistent, so the max is too. The walk
layers ignore `b`, so they are lower bounds wherever `b` stands. The A* needs
no reopenings, and its first goal pop is the model's shortest route.

## Memory

The polish runs in the arena's share of the budget, the bytes
`Arena::reserved_bytes` charges beyond `fixed_bytes`, which the arena freed
before the polish. The seed and the best route come out of it first: each is
at most the seed's length, so a job's allowance is that share less twice the
seed's length. A job charges:

- its event list;
- the other boxes' cells per event count;
- one `u16` walk layer per event over every cell;
- the tails;
- the live-cell maps;
- one `u32` per node id, (m + 1) × live cells × 5;
- 16 bytes per queue entry. That is an 8-byte key, doubled because a growing
  queue briefly holds its old and new buffers.

A job that cannot hold its buffers plus `MIN_QUEUE` (1,024) entries does not
start. A job whose queue reaches its cap ends with no gain. Every allocation
is fallible (`try_reserve`), so an allocator refusal ends the job too, never
the process. The route, a job's found route and a solution's letters fit in
the fixed `ROUTE_ALLOWANCE` (two routes of `MAX_ROUTE` bytes). Besides the
seed and the best route, the polisher never holds more than two of them at
once. So `reserved_bytes` does not change.
On wasm32 linear memory never shrinks, and the allocator may not fit the
polish into the arena's freed blocks, so linear memory can grow by up to the
arena's share again (see `Search::reserved_bytes`).

## Slicing and determinism

A unit is one of:

- starting a run: one copy of the seed;
- starting a job: two replays of the route, plus its allocations;
- one walk layer, which is one flood (layers are built from the last event
  down, so each tail adds the next);
- queueing the root;
- one A* pop, which is one flood and up to five relaxations;
- the final rebuild, which is one flood per step.

Every unit floods for itself, so no flood outlives a unit. Queue keys pack the
bit-flipped f, then the bit-flipped h, then the node id. Two entries never
share a key, so the pop order is fixed. A polish ends the same in slices of one
pop as in slices of 2^20, and natively as under WASM.

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

## Left out, for now

These come from the reference's Quality route repair, or were considered here:

- **Polishing earlier.** Polishing the first route as soon as Fast finds it,
  or polishing when a time limit stops the search. The second needs the
  caller to leave time for it, and transports do not decide search behavior.
  Polishing during the search needs memory beside a live arena. Both are
  candidates once the single-box polish is measured.
- **An h1-only fallback** (push distance only, with no walk layers) for jobs
  whose layers do not fit. It is weaker, and is worth adding only if
  measurement shows jobs being skipped at the budgets that matter.
- **Window rewrite**: re-searching short windows of the route with every box
  free. It catches what single-box rescheduling cannot: two boxes that must
  trade order. In the reference it gained nothing on Grand Hall once
  rescheduling had run.
- **Two-box rescheduling and goal swaps** within a label group: moves that
  single-box rescheduling cannot reach, since each job keeps its box's goal.
- **Diverse seeds**: polishing several first routes and keeping the best. In
  the reference, the choice of seed alone moved Grand Hall between 476 and 509
  moves. Runs vary the box order instead, on the one route the search has.
