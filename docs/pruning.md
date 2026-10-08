# Sealed-corral pruning notes

The sealed-corral check (`crates/search/src/corral.rs`) removes states from
which no solution exists, in every mode, Optimal included. The module docs
there give its soundness argument. This file records how its scope was
chosen, the census that checked it against exact solvability before it was
built, what adopting it measured, and the deeper checks measured beside it in
Optimal and rejected. README's solver section describes the
deadlock rule this check builds on and lists the other prunes that were
measured and rejected.

## The check's choices

- Dead states only. Restricting a state's successors to one corral's pushes
  and ordering corrals are push-objective techniques that lose routes or
  proofs under the move objective, so neither is ported.
- Boxes outside a corral count as floor, never as blockers. Treating a box
  that may still move as permanent is the false proof Sokomind2's corral
  check made (audit F-001). On the outside-box fixture, a freeze over every
  box flags 15 of the 233 reachable states that have a solution; the check
  flags 44, none with a solution.
- A corral with more than six potential pushes is skipped, which bounds the
  freeze work per expansion. The reference caps boundary boxes at six
  instead; capping pushes is equally sound and bounds the work directly. The
  cap never fired on the room and catalog inputs below and fired on 55 of the
  402,795 generated-room states, and lifting it flags no more states on any
  of the three.
- A corral with no empty cell is skipped. Judging those too flags several
  times as many states but none more that the admit chain keeps, so the
  extra work prunes nothing (see the census).
- A corral whose boxes are all on goals still counts when it holds an empty
  goal cell: no box from outside can get in to fill it.

## The census

Before the check was written in Rust, two independent Python ports of it, a
reference and an independent rewrite, judged every state of three inputs
against exact solvability:

- (a) the 150 rooms that `flagged_pushes_leave_no_solution` draws from
  `Lcg(0x5eed)`: 108,590 states, 103,437 without a solution;
- (b) 8 catalog boards explored to at most 20,000 states each: 32,743 states,
  29,125 without a solution;
- (g) 60 generated rooms, seed 12345: 402,795 states, 382,422 without a
  solution;

plus the fixtures' 339 states. The two ports agreed on every state of every
input, for each detector below. Per detector and input: states flagged, and of
those, states the admit chain keeps, which only this check removes.

| Detector | (a) rooms | (b) catalog | (g) generated |
| --- | --- | --- | --- |
| P, the production check | 12,591 / 1,276 | 8,675 / 620 | 46,256 / 3,741 |
| P judging corrals with no empty cell | 97,988 / 1,276 | 23,084 / 620 | 363,870 / 3,741 |
| S, each empty pocket's corral | 12,421 / 1,283 | 8,315 / 765 | 44,735 / 3,588 |
| P or S | 12,611 / 1,295 | 9,091 / 789 | 46,525 / 3,794 |

None of these flags a state with a solution. S judges, for each component of
the empty cells the keeper cannot reach, that component and the boxes beside
it, with a stand allowed anywhere outside that component and its boxes. Read
with the stand excluded from every such component instead, S is unsound: it
flagged 90 solvable states on (b), the first gen-v2-310089 state 1076, and the
solvable F-001 board, because a box of another component can move first and
open the one judged. Read as one corral over all of them, it is sound but adds
less to P: the two together keep 1,286, 658 and 3,777.

P was adopted alone. S added kept flags on every input, most on the catalog
(789 against 620), but measured in Rust the combined check was slower for
little extra pruning. Checking only the corrals beside the keeper after a push
was measured too: about 6% faster, but memory-bound runs generated up to 3.6
times as many states and lost routes, a proof and lower bounds. README lists
both among the rejected prunes.

The tests in corral.rs build four fixture boards, and both ports found the
counts they assert: the sealed corral (5 reachable states, all flagged, none
with a solution), the F-001 board (10 states, none flagged, the exact search
still finds 3 moves), the outside box above, and the settled corral (flagged
and kept per cell of the second box: 9 and 0, 9 and 9, 0 and 9, 9 and 9, 9
and 0).

## Adoption

The check landed in commit `50fb413`, written without a build on the
authoring host. The counts it pinned came from the Python ports and their
replica of the engine:

- Optimal at 20,000 states: medium proves 34 moves with 16,110 states
  generated instead of 18,504, pruning 680; large's lower bound rises from 73
  to 84; huge's stays 215. No catalog start is flagged.
- Fast at 1,000 states finds 30 catalog routes instead of 28, and Quality
  rescues expert-maze.

`Corral::BYTES_PER_CELL` adds 6 bytes per cell to the reserved bytes and
nothing per state: at 32 boxes on 4,096 cells, 256 MiB holds 2,788,993
states instead of 2,789,285.

The tests no longer pin the engine counts. The Rust tree's own flag counts
were measured later; see the flag floors below.

### A/B

Job 48276 measured each prune's effect on the tree just before the prune
toggles were dropped (rev `0392828`): the catalog example as is and with each
prune off, 57 cases per mode at 1M states, 64 MiB and 10 s, three alternating
rounds on one c4-standard-32 node, counters from round 1 and the median time.
No run drifted between rounds. "Dead pair" is the deadlock rule's axis case
where both neighbors are dead cells for the box's label; the corral check's
freeze uses it too.

| Mode | Side | Routes | Proofs | Generated | ms |
| --- | --- | --- | --- | --- | --- |
| Fast | both on | 50 | 0 | 29,458,654 | 29,823 |
| Fast | no dead pair | 50 | 0 | 29,458,709 | 29,906 |
| Fast | no corral | 46 | 0 | 34,143,915 | 26,805 |
| Quality | both on | 50 | 0 | 73,640,510 | 68,550 |
| Quality | no dead pair | 50 | 0 | 73,641,200 | 68,261 |
| Quality | no corral | 46 | 0 | 76,134,190 | 56,895 |
| Optimal | both on | 25 | 25 | 31,960,413 | 15,588 |
| Optimal | no dead pair | 25 | 25 | 31,961,165 | 15,444 |
| Optimal | no corral | 25 | 25 | 31,971,161 | 12,967 |

- The dead-pair case changes 2, 5 and 6 cases, by 55, 690 and 752 generated
  states in all, and the time stays within noise.
- The corral check keeps four routes in Fast and Quality that are lost
  without it (huge, gen-v2-340053-466f0008, gen-v2-340114-a8b9d02a and
  gen-v2-350108-80cb0432), and shortens two Fast and three Quality routes.
- Optimal proves the same 25 cases either way, every proof agreeing, and runs
  17% faster without the corral check at these limits. The check stays in
  Optimal anyway, because it raises the lower bounds of the cases it does not
  prove, and at larger limits it proves more (below).

Job 48564 measured what Optimal loses without it: the tree against itself
with the check off in Optimal only, three alternating rounds on one
c4-standard-32 node. That tree also warm-started the assignment, which
changes no search decision and was later dropped (README lists it among the
rejected approaches). Every proof agreed and Fast and Quality were identical.

- At 20k states and 64 MiB (the benchmark's limits) it proves the same 23
  cases, 15% faster, but 15 lower bounds fall: large from 84 to 73, the
  other 14 by 1.
- At 1M states, 64 MiB and 10 s it proves the same 25 cases, 15% faster, but
  15 lower bounds fall: large from 116 to 108, the other 14 by 1 or 2.
- Large in Optimal at 1 GiB: with the check it proves the 148-move optimum
  after 2,756,466 generated states, so a 4M-state run proves it too; without
  the check the 4M run stops at a lower bound of 138, and an 11M run proves it
  only after 4,552,497 states, 30% slower. Huge and open-field keep the same
  bounds at 1M, 4M and 11M states.

So the exact search keeps the check: removing it trades lower bounds and, at
larger limits, proofs for speed.

### Flag floors

The floor tests in corral.rs assert that the check still flags at least
about as many states as it does now, on the rooms of input (a) and on the
explored catalog of (b), so a check that goes silent fails. Each flagged
state is also checked to have no solution. The counts come from the tree
itself (jobs 48293, 48294 and 48299, rev `a7ef446`): each floor raised so its
assert prints the counts, on the tree and on weakened rules. Flagged / kept:

| Variant | Rooms | Catalog |
| --- | --- | --- |
| as is | 12,591 / 1,276 | 8,675 / 620 |
| no dead-pair case | 11,971 / 1,276 | 8,624 / 667 |
| no frozen-neighbor case | 11,651 / 941 | 7,878 / 648 |
| cap of three pushes | 12,067 / 1,276 | 8,638 / 620 |
| no empty-goal clause | 12,573 / 1,276 | 8,664 / 620 |
| cap lifted | unchanged | unchanged |

The tree's counts equal the census's on both inputs. Every weakened variant
fails at least one floor.

## The ladder's sound checks in Optimal

The stage ladder's accept tests include three sound dead-state checks
(`docs/stage-ladder.md`, Accept): sink-line order, keeper-aware reach
matching and sealed pockets. Job 49446 measured each as an Optimal prune, run
in the expansion's prologue after this check, on a probe branch
(`exp/optimal-ideas`, `f89f44a`, on `main` `363fa80` with the memory cap
lifted to 4000 MiB) that was never merged. Optimal ran on the catalog at the
benchmark's limits (20k states, 64 MiB) and at 1M states and 256 MiB, and on
Grand Hall at 1 GiB and at 4000 MiB with 60M states, one run each. With the
checks off every record matched `main`, in all three modes at the benchmark's
limits.

The runs bore out their soundness: the 24 proofs at 20k states and the 27 at
1M were the same in every variant, and no bound or route passed a proven
optimum. None proved a board the others did not.

| Prune | Catalog at 1M: bounds raised, time | Grand Hall at 1 GiB: bound, time | Grand Hall at 4000 MiB |
| --- | --- | --- | --- |
| none | 0, 69.3 s | 283, 40.4 s | 286, 145.4 s |
| sink lines | 0, 69.8 s | 283, 41.2 s | 286, 146.2 s |
| sealed pockets | 10, 94.8 s | 283, 51.8 s | 286, 178.0 s |

- Sink lines pruned 0 states on Grand Hall and 43,686 on the catalog at 1M
  states, where they moved no bound. At 20k states they raised one bound by 1
  (gen-v2-320041-e16f5a47, 74 to 75).
- Sealed pockets raised 10 catalog bounds at 1M states by 1 to 3
  (gen-v2-350070-6be05c02 156 to 159, gen-v2-340053-466f0008 71 to 74,
  gen-v2-340053-6d913910 64 to 67), and 12 at 20k states by 1 or 2. They took
  37% more time on the catalog and 22 to 28% more on Grand Hall, where they
  pruned 4.6% of their checks (716,373 of 15.6M at 4000 MiB, about 2.3 µs a
  check) and left the bound where it was. A prologue prune keeps the dead
  state's record and the 60M limit counts records, so that run expanded only
  0.4% more states (14.20M to 14.26M).
- Sink lines and sealed pockets together measured the same as sealed pockets.
- Keeper-aware reach matching was false on 0 of 16.0M expanded states, and
  needs no prune: Optimal's estimate uses the same relaxed push graph, is None
  exactly when the matching fails, and a child with no estimate is never
  queued.

This check also costs Optimal time, but proves more at larger limits (above).
Neither of these proved a board or raised Grand Hall's bound, so neither was
adopted (2026-10-08). Grand Hall's gap, 286 against a 436-move route, is not
in dead states.

## Replicas

Python replicas of the engine and its components were written during this
work to predict counts before the Rust code existed, and to check that two
readings of a rule agree. They are not kept in the repository. Their
conclusions that still matter:

- The census above, and the fixture counts the corral tests assert, which
  both ports found.
- The Quality restart: the engine replica with the corral check found that at
  1,000 states Fast misses 27 catalog boards, and that the restarted Quality
  run still finds expert-maze, 65 moves after 2,000 records in all, where Fast
  given 10,000 states needs 2,920 records for a 79-move route. Extended with
  Quality's phases and restart, it had Fast then Quality fill the arena on 14
  of the 30 boards Fast routes, and Quality start over carrying each route,
  shortening 2. These counts are a record of that measurement, not a test:
  the engine tests check only that Quality never ends longer than Fast and
  that its route replays.
