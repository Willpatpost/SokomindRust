# Route polish

Quality mode's search shortens its route by finding a better one in the arena.
On large boards it stops short: on Grand Hall (`huge`) the staged route of 812
moves came back from the search unchanged, because the restart's arena fills
long before weight 3 reaches anything shorter. The polish takes it to 485
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

The polisher works on one box at a time, cycling through the slots in order. A
job for box `b`:

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
gains nothing. The polish ends when a full cycle of jobs, one per box, has
gained nothing. A gain restarts that count, so the polish ends only at a local
optimum under single-box rescheduling.

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
before the polish. A job charges:

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
the fixed `ROUTE_ALLOWANCE` (two routes of `MAX_ROUTE` bytes). The polisher
never holds more than two of them at once. So `reserved_bytes` does not change.
On wasm32 linear memory never shrinks, and the allocator may not fit the
polish into the arena's freed blocks, so linear memory can grow by up to the
arena's share again (see `Search::reserved_bytes`).

## Slicing and determinism

A unit is one of:

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
  - 22 of the 57 Quality routes are shorter. None is longer, and the other 35
    are unchanged.
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
- **Grand Hall.** The route is 485 moves (226 pushes) at every budget
  measured:
  - 20,000 and 1,000,000 states at 64 MiB;
  - the production limits (60M states, 5 s) at 64, 128 and 256 MiB.
- **Timing at the production limits.**
  - The search ends at its memory limit at about 1.7, 2.3 and 3.9 s.
  - The polish ends about 0.3 s later, at 2.0, 2.65 and 4.13 s, inside the
    5 s limit.
  - `reserved_bytes` is identical with and without the polish.

485 is a local optimum under single-box rescheduling. The reference's
rescheduler alone took its Grand Hall route from 789 to 503 moves. Its best
route, 476 moves, also needed window rewrite, eight rounds and a chosen seed.
Those are the moves listed next.

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
  trade order.
- **Two-box rescheduling and goal swaps** within a label group: moves that
  single-box rescheduling cannot reach, since each job keeps its box's goal.
- **Diverse seeds**: polishing several first routes and keeping the best. In
  the reference, the choice of seed alone moved Grand Hall between 476 and 509
  moves.
