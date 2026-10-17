# Parallel polish

The polish's kick rounds can race on a native thread pool. On the
searches whose polish dominates a big budget — Grand Hall's
60M-state Quality runs chief among them — the raced rounds cut the
wall time by 2.2× at the exact sequential route; on smaller
budgets, where the sequential's kick budget dies mid-chain, the
raced rounds find routes the sequential never reaches. The knob is
`SOKOMIND_POLISH_WORKERS`, unset by default; wasm has no threads
and no environment, so it never races.

## The round, raced

A kick round, run sequentially (docs/route-polish.md), tries each
same-label pair's goal trade in turn: a kick perturbs the best
route, the single-box and pair jobs descend the kicked route, and
the first descent that beats the best resets the round from the next
pair. The racing version dispatches the whole round at once — every
pair's kick and descent as one task on a pool of
`SOKOMIND_POLISH_WORKERS` threads, each task with its own flood
scratch and job buffers, over a board clone and a freshly built
heuristic shared through `Arc`s. Only the weighted policies polish,
so the heuristic never counts keeper sides and is cheap to build.

The polish's step contract is preserved: a step polls one
completion without blocking, so the engine's slicing — the worker
yields, the time limits, the cancellation — behaves as before. A
poll that finds nothing spends no kick unit: the wait is not work.

The merge, when the last completion is in, applies the first
improver in dispatch order — the one the sequential path would have
found first — and the next round starts from the pair after it. The
trials past the improver ran from the base the improvement
replaced, so the sequential path never runs them: they are the
round's speculation, spent in wall time but not budget. The kick
budget charges exactly what the sequential path would have spent to
reach the same place — every trial up to and including the first
improver, the whole round when nothing improved — which is why the
raced polish walks the same improvement chain to the same route and
ends at the same budget death.

## The measurement (2026-10-17)

Grand Hall, quality mode, the standing 60M-state / 256 MiB
configuration. The sequential baseline: **424 moves in 1227.8 s**
(690,653 expansions, bit-identical in every run below — the racing
touches only the polish).

| Configuration | Wall | Speedup | Route |
| --- | ---: | ---: | --- |
| Sequential | 1227.8 s | — | 424 |
| 16 workers, every trial charged | 294.3 s | 4.2× | 429 |
| 16 workers, sequential-equivalent charge | 562.8 s | **2.2×** | **424** |
| 55 workers, sequential-equivalent charge | 578.2 s | 2.1× | **424** |

Charging every trial's units — the accounting the first measurement
tried — burns the kick budget on the speculation and stops the
improvement chain a round early: the route lands five moves worse.
Charging only the sequential equivalent restores the route exactly,
at the measured wall cost.

The worker curve is flat past sixteen: a round finishes when its
slowest trial finishes, and the trials are memory-bound — a kick's
A* floods and table growths — so more threads only contend for the
same bandwidth. Sixteen workers is the documented setting; the pod's
remaining cores stay free for other work, board-level parallelism
among it.

The wall breakdown: the kick phase compresses 1110 s to about
480 s; the search (about 50 s) and the runs and pairs (about 45 s)
are sequential and untouched. Grand Hall's run goes from 20.5
minutes to 9.5.

## The corpus (2026-10-17)

The 57-board catalog, quality mode at 1M states / 64 MiB, the knob
off versus 16 workers: **8 better, 0 worse, 42 same** (7 boards
did not solve at the budget, both runs alike).

- huge 436 → 432, gen-v2-340053 115 → 107, gen-v2-340272 255 →
  245, gen-v2-350250 274 → 262, gen-v2-360372 297 → 285,
  gen-v2-320081 106 → 105, gen-v2-340106 155 → 154,
  gen-v2-370002 358 → 356.

These are the leapfrog the accounting makes possible: at the
16M-unit kick budgets these polishes die mid-chain, and the raced
rounds' uncharged trials find the improvements the dying budget
never reaches — the merge applies the improver, the charge caps at
whatever budget remains, and the polish ends with the gain kept.
The wall cost is real: the sweep took 898 s against the
sequential's 514, the speculation being work on small boards. The
knob therefore trades wall for depth at small budgets, and buys
speed at no cost on the big ones.
