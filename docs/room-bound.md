# Room bound design notes

The room bound (`crates/search/src/room_bound.rs` and `room_bound/`) raises
Optimal's lower bound with exact tables for the board's single-door rooms. The
module docs say what each part does. This file records why the bound is sound,
how its build fits the search's memory and time, the measurements behind it,
with the SLURM job that produced each number, and the approaches it replaced.

On Grand Hall (`huge`) it raises the start's bound from 214 to 258, and the
lower bound at 30 s from 238 to 277 at 64 MiB, from 241 to 280 at 256 MiB and
from 244 to 283 at 1 GiB (see [Measurements](#measurements)).

## The bound

A door is a floor cell whose removal splits the keeper's floor, boxes ignored.
A room Q is one side of a door. Each room that holds a start box or a goal gets
one or two tables R(Q, P, LAB). A table is the exact cost of a relaxed problem
over Q that keeps only the boxes of the labels LAB, counting only the moves
that end on its part P ⊆ Q. Let C be the union of the parts. The hall part
M_hall is the assignment heuristic with every move that ends on C made free.
The room bound is

S = ΣR + M_hall,

and Optimal queues each state by h = max(M, S), where M is the assignment
estimate every mode queues by (see the README).

## Soundness

### Counting by end cell

Every move ends with the keeper on exactly one cell. That holds for a step, and
for a push, where the keeper ends on the box's old cell. For a set of cells P,
let cost_P(π) be the number of moves of route π that end on P. If P_1, ...,
P_k and H partition the board's cells, then

|π| = cost_P1(π) + ... + cost_Pk(π) + cost_H(π).

So a bound that never exceeds cost_Pi(π) for each i, plus one that never
exceeds cost_H(π), adds up to a bound on |π|. Each holds for every route, so
they hold for the same one.

### A table R(Q, P, LAB)

The door is the only cell outside Q next to Q, and the floor outside Q is
connected. A table state is (layout, node):

- the layout says which cell of Q holds which box of a label in LAB;
- the node is the keeper's cell when it is in Q, and one node for the whole
  floor outside Q otherwise.

Each table move costs 1 if it ends on a cell of P, and 0 otherwise:

- **Step.** The keeper moves to an adjacent floor cell that holds no LAB box.
  A step that stays outside Q is no move at all.
- **Push inside Q.** A LAB box moves from x ∈ Q to a free t ∈ Q, and the keeper
  ends on x.
- **Exit.** A LAB box on x ∈ Q is pushed onto the door. It disappears, and the
  keeper ends on x.
- **Entry.** A box of label l ∈ LAB is pushed from the door onto a free t ∈ Q,
  and the keeper ends on the door, at cost 0. An entry is allowed only while Q
  holds fewer l boxes than the board has.
- Every push and entry goes to a cell that is live for the box's label: a lone
  box there can still reach a goal of that label.

A **goal** state has every goal in Q of a label in LAB holding a box of that
label, and no other LAB box in Q.

The **projection** proj(s) of a real state keeps its LAB boxes on Q and the
keeper's node. It drops the boxes of other labels, and every box outside Q.

**Lemma 1 (simulation).** Let s → s' be a real move where s' still has a
solution. Then proj(s) → proj(s') is a table move or no move, and its table
cost is [the move's end cell ∈ P]. Case by case, with the keeper going from a
to b:

- A real step: b is free in s, so it holds no LAB box in proj(s).
- A push of a box the projection drops (its label is not in LAB, or it is
  outside Q before and after): the keeper's node goes from node(a) to node(b).
  That is a step, or no move, at the same cost. b held a dropped box, so it is
  free in the projection.
- A push of a LAB box from x ∈ Q: a push inside Q, or an exit when it lands on
  the door. The keeper ends on x.
- A push of a LAB box from the door into Q: an entry. The keeper ends on the
  door, outside Q, at cost 0.

The side conditions hold because s and s' both have solutions. A box on a cell
that is dead for its label can never reach a goal, since other boxes only make
that harder, so every box of s and s' is on a live cell. And no real state
holds more l boxes in Q than the board has.

**Lemma 2 (goal).** A solved state projects to a goal state. This needs equal
box and goal counts per label, so that every goal is filled, which the parser
enforces.

**Consequences.** Let R(u) be the exact table distance from u to a goal state.

- *Admissible.* For every route π from s, R(proj(s)) ≤ cost_P(π): project π
  move by move (Lemma 1), and it ends on a goal state (Lemma 2).
- *Consistent.* For every real move s → s', R(proj(s)) ≤ [end ∈ P] +
  R(proj(s')), since R is an exact shortest-path distance and the move maps to
  an edge of exactly that cost. If s' has no solution, any value works for it.
- *No identities.* proj(s) is a single table state, so a lookup needs no box
  identities and no minimum over ways to split the boxes.
- *Clamped.* A table stores at most `MAX_VALUE` (65,534). min(R, MAX_VALUE) is
  still a lower bound, and still consistent, since R(u) ≤ c + R(v) implies the
  same of the minima.

**Lemma 3 (pruned table).** A table holds only the states reachable from the
start's projection that reach a goal state. It drops the dead ones on the way:
a state is dead when Q holds more boxes of a label on its stuck cells than Q
has goals of that label. A stuck cell is one from which a lone box of that
label cannot leave Q over live cells. A stuck box can never leave Q, so it
must end on a goal in Q, and a dead projection means s has no solution. The
build also refuses states with a box frozen off a goal of its label (see
[A table's build](#a-tables-build)). They reach no goal state either, so the
set the table holds is the same.

The search reaches only states reachable from the start. If such a state has a
solution, so does every state on its path from the start, so Lemma 1 applies
to every move on that path, and the state's projection is in the table. So a
missing projection proves that s has no solution, and pruning that state is
sound, like a deadlock prune.

### Adding tables

Tables whose parts are pairwise disjoint add, whatever their rooms and label
sets: ΣR_i(proj_i(s)) ≤ Σcost_Pi(π) for every route π. The sum is consistent
too, since a move ends on at most one part. The rooms the build keeps are
pairwise disjoint, and the two tables of a split room have the room's two
halves as parts, so every part is disjoint from every other.

### The hall part M_hall

M_hall is the assignment heuristic M with every move that ends on C made free.
Call B the cells a box can stand on, those with a spot live for some label.
charge_C(t) is the fewest steps that end outside C on a walk from B to t, a
0-1 BFS where a step onto a C cell costs 0. A push from stand t costs
[end ∉ C] + charge_C(t), where the push ends on the box's old cell. M_hall(s)
is the matching over these costs, less charge_C of the keeper's cell. It keeps
M's keeper sides, since every real box path is a path in their graph, and
charge_C ≤ M's charge, so its feasibility equals M's.

- *The walks.* No state on a route has a box off B, since M_hall would be
  `None` there. A push leaves the keeper on its box's old cell, in B. So the
  keeper leaves B only by stepping, and a step onto b raises charge_C by at
  most [b ∉ C]. Before a push from t, the walk since the keeper last stood in
  B therefore holds at least charge_C(t) steps that end outside C, and before
  the route's first push at least charge_C(t) − charge_C(start keeper). These
  walks are disjoint for distinct pushes, as every push ends in B.
- *Admissible.* Each box's pushes cost at least its M_hall distance to its
  final goal, and each push's cost is matched by its own move and its walk,
  all outside C. Different boxes' pushes are different moves, the walks are
  disjoint, and the final placement is one assignment. So
  M_hall(s) ≤ cost_{outside C}(π).
- *Consistent.* A step onto b moves no box and leaves the keeper's region
  alone, so the matching holds, and M_hall drops by at most [b ∉ C]. A push
  from the keeper's cell t moves one box along one edge of cost
  [end ∉ C] + charge_C(t), and leaves the keeper in B at charge 0, so under
  the same assignment M_hall drops by at most [end ∉ C]. Only the start can
  have a keeper charge, so the engine subtracts it there alone, in `root`.

### The combined bound

By the end-cell count, the parts and the cells outside C partition the moves,
so S ≤ |π| for every route π. A move costs 1 and lowers at most the one term
of its class, by at most 1, so S is consistent. M is admissible and
consistent, so h = max(M, S) is both. At a goal state, S = 0.

### What the engine does with h

- **The node stores M.** Each node record keeps M, the value the incremental
  assignment repair starts from. The queue key uses max(M, S), with S clamped
  to `MAX_QUEUED_H` like any queued estimate. A cheaper duplicate reuses its
  record's M and has S priced again, since S depends only on the state.
- **g + h ≥ best.** `push::admit` prunes with M, as before. The room bound then
  prices the child: a missing projection counts as `pruned_assignment` (Lemma
  3), and g + S ≥ best as `pruned_bound`. Any admissible h may prune this way.
- **The stand-walk prune stays on M.** It adds to M the stand walk that M does
  not charge. The walk's cells may lie in C, where S has already counted them.
- **The start's key** is max(root_estimate(M), S). root_estimate adds a stand
  walk to M, under the same argument.
- **The PEA\* prefilter stays on M.** It bounds a child's f from the parent's
  stored h, which is M, so its bound g + M + excess is still a lower bound,
  and S(parent) − w − 1 would be an optional extra. Partial expansion itself
  is decided against the queued h = max(M, S), and a deferred child is
  re-queued with the least deferred f, still a lower bound. A child that may
  be solved has M = S = 0, so it is never deferred.
- **No reopening.** h is consistent, so a closed node never needs reopening.
  The start's walk term makes the start alone inconsistent, and it is popped
  first anyway.
- **The keeper is exact.** An exact-search node carries the keeper's cell, the
  pushed box's old cell or the start's, so a table's node is exact.
- **The lower bound** that `ExactSearch` reports reads the queue's least key,
  so the start's max(M, S) raises it from the first pop.

### Not claimed

- A table bounds only its part's moves under its relaxation. Boxes outside Q
  and labels outside LAB are invisible to it. On Grand Hall the room's true
  cost lies in [98, 117] against the tables' 93; the gap is their interaction.
- M_hall counts walking outside C only as stand charges, the steps from B to
  each push's stand. A walk that stays in B, such as the keeper crossing the
  hall between boxes, costs it nothing.
- Which rooms, parts and labels get tables is a heuristic choice. Any choice is
  sound, as long as the parts are disjoint and every table is built from the
  start's projection.

## The build

### Rooms

The candidates are, for each cell of the keeper's floor taken as a door, each
of its sides but the largest that holds a start box or a goal and has at most
62 cells. Parts are `u64` bit sets over a room's cells, and a node fits a `u8`.
A side's flood stops past 62 cells, so a door costs O(62) rather than O(floor).

The candidates are laminar: any two nest or are disjoint (`candidates` in
`room_bound.rs` proves it). So one cell of a room built earlier tells whether
it holds a candidate, lies inside it, or lies apart from it.

A table's label set is the labels with a start box or a goal on its part.

### A table's build

The build of one table (`room_bound/table.rs`) has four phases, each resumable
between work units:

1. A forward breadth-first search from the start's projection stores every
   reachable state, up to a cap. A state is a `u64` key, a code per cell of Q
   and the node above them, kept in fixed chunks of 65,536 that never move,
   with an open-addressing table of `u32` slots at most 3/4 full. A slot
   holds a state's id and 7 bits of its key's hash, so a probe reads a key,
   a likely cache miss, only when those bits match. The search refuses a push
   or an entry that leaves a box frozen off a goal of its label, on the
   box's new cell or next to it (`Rules::freezes`).
2. A scan of the states seeds the goal states at distance 0.
3. A backward 0/1 breadth-first search from them walks the moves in reverse,
   with a `u16` distance per state and two level worklists. It generates each
   predecessor from the room's inverse move lists (`pulls` and `unentries` in
   `room_bound/room.rs`) and keeps those the forward search reached. A
   predecessor test restates the forward move's predicate, so the backward
   search walks exactly the forward edges without storing them. Between
   levels, once the start has a distance, it fails the table as soon as the
   values found so far no longer fit the kept limit. Their count only grows
   and the kept tables stay put while a table builds, so such a table would
   fail at the end anyway. Before the start has a distance it goes on, since a
   start that reaches no goal ends the build with no table, not a failure.
4. The values of the states that reach a goal go into an open-addressing table
   of keys and `u16` values, 10 bytes a slot, with linear probing that wraps
   at the end. It has count · 8/7 + 1 slots, just under 7/8 full, rather than
   a power of two, so a key's first slot scales the top 32 bits of its hash to
   the slot count. That is about 11.4 bytes a value, against 13.3 to 26.7 at
   3/4 with a power of two. This is the only part the search keeps.

A unit expands, pops or scans one state, or scans 16 states while seeding or
inserting values. The forward search costs about 8 bytes a state for its key,
and 5 to 11 for its slot table, more while the table doubles. The backward
search adds 2 for the distance and up to 8 for the worklists.

**The freeze refusal.** A box is frozen when, on each axis, one side is a
wall, or neither side is a live cell for its label, or one side holds a box
that is frozen in turn with this one taken as a wall. No box of such a set can
move first, since each stays blocked while the others stay put, so none ever
moves. Pushes onto a cell that is not live are refused anyway, which is why
two such sides block an axis. The keeper never matters. The door is never a
wall and never holds a box, since boxes outside Q are not part of a state.
These are the blocking conditions of the search's post-push freeze rule
(`deadlock.rs`), restated for a table state, whose codes on Q indices have no
board occupancy map. A recursion from the placed box stands in for that rule's
fixpoint over the pushed box's component.

- *Sound.* A goal state holds every box of Q on a goal of its label, so a
  state with a box frozen off its goal reaches no goal state, and neither
  does any state it reaches. So no state that reaches a goal state has one,
  and the forward search takes every move along every path from the start to
  a goal state. The states the table keeps, their values and the start's
  value are the same; only the states reached drop. On the probe, every table
  that both builds finished matched, with 28% of the states reached (job
  49024).
- *Local.* The test checks only the box just placed and its four neighbors,
  the only boxes that may freeze anew. A box frozen further along a chain is
  left to the backward search, which never reaches it either.
- *Backward.* The predecessor test leaves the freeze test out. The backward
  search visits only states that reach a goal state, and the forward search
  never refuses a move onto one, so it still walks exactly the forward edges.
- *Cost.* The recursion keeps no memo, so its worst case grows exponentially
  with a cluster of touching boxes. In practice the wall and live tests settle
  most axes at once: the forward search's time per state rose 5 to 13% over
  the catalog and in Grand Hall's repeated runs, against 72% fewer states
  reached (job 49024).

### Order, replacement and skips

The build tries each candidate, narrowest door first, then smallest, unless:

- it lies inside a built room, whose parts already hold its cells;
- it encloses a room whose build failed behind a door no wider than its own,
  so its own would very likely fail too;
- or the tables of the rooms it encloses reached more than cap / 16
  (`ENCLOSED_SHARE`) states in their forward searches: its own search holds
  theirs, so it would very likely pass the cap and waste its work.

A door's width is the number of its neighbors in the room, 1 to 3. Boxes enter
a room behind a narrower door in fewer ways, so it may hold far fewer states
than a room it encloses behind a wider door. On Grand Hall the 25-cell room
behind door (12,7), of width 3, fails with both halves, while the 26-cell room
that encloses it behind door (11,7), of width 1, builds both its tables.
Smallest first, the build paid for the two failed halves, 23.1M units, before
it reached the 26-cell room. Narrowest first, it builds the 26-cell room first
and skips the 25-cell one, which lies inside it. That halves Grand Hall's
build, from 47.6M units to 24.5M at 256 MiB and from 55.4M to 24.5M at 1 GiB,
and lowers its peak at 1 GiB from 288.6 MiB to 172.3 MiB (job 49011).

The rooms nest or are disjoint, so one cell tells how two that meet relate: a
built room holds the candidate if it is larger and holds the candidate's first
cell. Otherwise it lies inside the candidate if the candidate holds its first
cell. The built parts therefore stay disjoint whatever the order, which is all
the sum needs.

A room that encloses no built room tries its whole-room table, then, if that
failed or its key needs more than 63 bits, the two halves of the room, split at
the door's line. It tries a half only when the half holds some, but not all, of
the room's labels. A half with none has no table. A half with all of them
differs from the whole room only in which moves count, so its key, forward
search and kept size are the whole room's, and it would very likely fail
again. In job 48991 all 13 such halves failed again, 12 in the forward search
and one out of work, and on gen-v2-370002 at 5 s and 64 MiB they took 2.4 of
the build's 4.8 s. It keeps whatever builds. A room that
encloses built rooms tries only its whole-room table, and replaces them if it
builds; otherwise they stay. A failure is a forward search past the cap, a
buffer the ledger or the allocator refuses, a table past the kept limit, or the
work budget running out. It marks the room, so every room that encloses it
behind a door at least as wide is skipped.

A replacement never lowers the tables' sum, so the build keeps the larger room
without comparing the two at the start. R(C, C, LAB_C) is at least the sum of
the enclosed rooms' tables R(E, P, LAB) in every state: each LAB lies in
LAB_C, each P in C, and the parts are disjoint. An optimal solution of C's
relaxed problem, cut down to E's labels and cells, is a solution of E's by
Lemma 1's projection, and it counts a move on P only where C's counts that
move. The hall part may drop, since the moves that end in C outside the
enclosed parts become free, so the larger room is the choice measured
stronger, not a proven gain. Only the whole-room table holds the enclosed
rooms' problems, so only it may replace them.

### Memory

The build's ledger charges every large buffer before it is allocated, and a
room's few small move lists right after, and refunds each when it is freed.

- Its limit is the budget less the fixed buffers and the arena holding the
  start. The hall part's bytes are set aside first, since it is built last.
- The kept tables may take at most a quarter of the budget less the fixed
  buffers (`KEPT_SHARE`), so they leave most of it to the arena.

When the build ends with tables, the engine sizes the arena again with
`Arena::recharge`, with the tables and the hall part counted among the fixed
buffers. No pop runs while the build does, so the arena still holds only the
start, which any limit fits. The new limit is never larger, so the re-size
allocates nothing, and a search that lost states to it reports `MemoryLimit`
rather than `StateLimit` when it fills. If even the first block no longer fits,
the search drops the tables and goes on with M alone.

`Search::reserved_bytes` is the whole budget while the build runs. Once it
ends, it is the fixed buffers, the kept tables and the arena at its new limit,
at most the old reservation. On wasm32, the arena's later blocks may not reuse
the build's freed buffers, so linear memory can grow by up to the arena's share
again.

### Time

A table's forward search may reach 16 states per node the arena may hold
(`STATES_PER_NODE`), clamped to 2^20..2^24 (`MIN_CAP`, `MAX_CAP`), so the
build scales with the search it serves. The whole build stops after 4 caps of
work units (`WORK_PER_CAP`) and keeps what it built by then.

The build runs inside `Search::advance` before the search proper, and a pop
slot pays for 64 of its units (`BUILD_UNITS_PER_POP`). So a time check between
calls bounds it like pops, and the build is deterministic, with no clock of its
own. A search stopped during the build reports the start's bound without the
tables.

A unit took 142 to 153 ns on job 49024's probe machine, where the build
without the freeze refusal took 128 to 157. The refusal saves far more units
than it costs: Grand Hall's build fell from 24.5M units to 7.5M, and took 1.06
to 1.15 s in that job's fifteen traced runs, against 3.1 to 3.9 s. So at 5 s
with 256 MiB or 1 GiB it finishes with about 4 s to spare, and the search
reaches 276. On a machine too slow to finish it, the search ends with the
start's bound, 214, below the 240 or 241 that M alone reaches.

### After the build

The start's queue entry, keyed by M's root estimate, is replaced by one keyed
by max(M, S). Its record keeps M, as every record does. The start is inserted
once, so generated and unique states count it once, and a capped search's
generated states stay within the state limit. With no table, or when the
re-size refuses, the search goes on with the entry it has, keyed by M.

## Measurements

### The measurement build (job 48775)

The bound was first measured on a separate build with the same tables and the
same engine rules. Its build differed: it stored each table's edges, tried the
largest room first, kept its tables outside the memory budget, and counted its
build in the time limit without slicing it. The runs compared Optimal with and
without the room bound at rev `ff664bc`.

Soundness: 27 proven optima, no proof that disagrees, no bound above a proven
optimum, no start's bound above an optimum, and no unsolvable proof. The Rust
tables match an independent Python build: on Grand Hall, the tables of labels
B, C and X and of A, D and X have start values 36 and 57, with 710,100 states
of the first reaching a goal in both.

The start's bound: on Grand Hall M is 213, the start's estimate 214, the hall
part 162 and S = 36 + 57 + 162 = 255. On `large` it rises from 54 to 81. Of
the catalog, 31 boards build tables, and 22 raise the start's bound, by 346
moves in all.

| Board | Limits | M alone | With the room bound |
| --- | --- | --- | --- |
| `huge` | 1M states, 256 MiB | 238 | 275 |
| `huge` | 11M states, 1 GiB | 243 (11.6 s) | 279 (39.9 s, 13.6 s of it the build) |
| `huge` | 30 s, 64 MiB | 238 | 275 |
| `huge` | 30 s, 256 MiB | 241 | 277 |
| `huge` | 30 s, 1 GiB | 244 | 278 |
| `huge` | 5 s, 64 MiB to 1 GiB | 238 to 241 | 255, all of it the build |
| `large` | 1M states, 256 MiB | 145 | proved 148 in 0.4 s |
| `large` | 11M states, 1 GiB | proved 148 in 1.54 s | proved 148 in 0.48 s |

On `large` the expansions fell from 838k to 202k. At 20k states and 64 MiB the
open catalog boards' summed bound rose from 3218 to 3456, 16 higher and none
lower. At 5 s and 64 MiB one more board was proved (320093) and the summed
bound rose from 3360 to 3417, but five boards fell, because the build counted
in the time limit: 320165 (71 to 54), 320081 (79 to 76), 340187 (99 to 82),
370002 (173 to 154) and 360372 (155 to 133). Expansion cost about 2.3 times as
much per state on Grand Hall.

That build's two blockers shaped the production one:

- Its build took up to 16.5 s, 10 to 13.6 s on Grand Hall, about 7 s of it
  wasted on the 27-cell room behind door (10,7), whose halves passed the cap
  before the 26-cell room nested in it was tried.
- It peaked at 493 MiB per Grand Hall table, with about 10 MiB kept each,
  outside the budget, against the web's 64 MiB default.

### The production build (job 49011)

The same parts on the c4-standard-32 probe machine compared Optimal at `main`
(`ff664bc`) with this build. Every catalog case ran at 20k states and 64 MiB,
and at 5 s and 64 MiB. Grand Hall, `large` and `open-field` also ran at 1M
states with 256 MiB, at 11M with 1 GiB, and at 5 s and 30 s with 64, 256 and
1024 MiB. Each run ran once.

Soundness: 27 proven optima, no proof that disagrees, no bound or start's bound
above a proven optimum, and no unsolvable proof. Clippy passed.

| Board | Limits | M alone | With the room bound |
| --- | --- | --- | --- |
| `huge` | 1M states, 256 MiB | 238 (0.8 s) | 275 (8.2 s) |
| `huge` | 11M states, 1 GiB | 243 (9.9 s) | 279 (31.8 s) |
| `huge` | 30 s, 256 MiB | 241 | 277 |
| `huge` | 30 s, 1 GiB | 244 | 279 |
| `huge` | 5 s or 30 s, 64 MiB | 238 | 238, its tables pass the cap |
| `huge` | 5 s, 256 MiB | 241 | 214, the build unfinished |
| `huge` | 5 s, 1 GiB | 240 | 214, the build unfinished |
| `large` | 1M states, 256 MiB | 145 | proved 148 in 0.35 s |
| `large` | 5 s to 11M states, 64 MiB to 1 GiB | proved 148 in 1.1 to 1.3 s | proved 148 in 0.3 to 0.4 s |

`open-field` has no candidate and matched `main` in every run. At 20k states,
29 catalog boards build tables, and 20 of them raise the start's bound, by 233
moves in all. There the open boards' summed bound rose from 3218 to 3376, 14
higher and none lower. At 5 s and 64 MiB it rose from 3360 to 3511, 13 higher
and none lower, with one more board proved (320093). At those limits the
build's peak was at most 45.6 MiB and its kept tables at most 12.5 MiB. Peak
resident memory at 64 MiB reached 65.9 MiB, as `main`'s did, the process's own
overhead above the budget.

At 5 s with 256 MiB or 1 GiB, Grand Hall's build of 24.5M units takes 4.9 to
6.3 s, so the search may stop inside it and report the start's bound, 214. A
copy of this build with probe-only tracing finished at 4.9 s at 256 MiB and
reached 269, and at 1 GiB the build ran past 5 s in both. These two runs are the
only ones where the room bound lowers a bound below M alone's. The slot tags
below fix both.

Two earlier probes of the production build led here:

- Job 48969 skipped every room enclosing a failed one. On Grand Hall the 25-cell
  room, which fails, then skipped the 26-cell room that builds, and no table was
  built.
- Job 48991 passed the skip only through a door at least as wide, and Grand
  Hall reached 275 to 279 as above. Smallest first, its build tried the 25-cell
  room's halves before the 26-cell room, 47.6M to 55.4M units in all (job
  49011's copy). On gen-v2-370002 at 5 s and 64 MiB, the halves that hold every
  label of their failed room left the bound at 170, against 173 for M alone.
- Job 48991 also measured `ENCLOSED_SHARE` at 4 and 8. They raised the open
  boards' sum at 20k states, where the 1M cap binds, by 12 and 7, but not at
  5 s and 64 MiB (3508 and 3507, against 3508 at 16), and matched 16 in every
  larger run. It stays 16.

Before narrowest first, the room bound added 89,729 bytes to the wasm module,
from 257,025 to 346,754 (jobs 48991 and 49011). Most of it is `RoomBuild::step`
and the sorts of the move lists (job 48992).

### Slot tags (job 49016)

The forward search's slots first held only a state's id, so a probe read the
key of every state it passed, a likely cache miss each. A slot now also holds 7
bits of its key's hash, as the arena's do, and a probe reads a key only when
they match. Job 49016 compared `main` (`3bc27d8`) with the tags in job 49011's
parts, and ran Grand Hall three more times each at 5 s with 256 MiB and 1 GiB
and at 1M states with 256 MiB. Copies of both builds with probe-only tracing
timed each table's phases.

The results were identical in all 133 runs that no clock stopped, and the
traced tables were identical in all 81 traced runs where both builds finished.
Soundness held as in job 49011, and clippy passed.

| Measure | `main` | With the tags |
| --- | --- | --- |
| A unit in Grand Hall's 24.5M-unit build | 214 to 266 ns | 161 to 195 ns |
| A unit over the catalog, 20k states / 5 s, 64 MiB | 200 / 209 ns | 167 / 169 ns |
| Grand Hall's build, 1M states, 256 MiB (4 runs) | 5.25 to 6.45 s | 3.95 to 4.78 s |
| Grand Hall's build, 5 s, 256 MiB or 1 GiB (8 runs) | unfinished | 4.04 to 4.33 s |
| Grand Hall's bound, 5 s, 256 MiB or 1 GiB | 214 | 272 to 273 |

Each 26-cell table on Grand Hall then reached 10,253,873 states. Its forward search
fell from 2.26 to 3.14 s (220 to 307 ns a state) to 1.76 to 2.22 s (172 to
216 ns), and its backward search from 129 to 193 ms to 108 to 138 ms. Every
other bound matched `main`, and the catalog's summed search time fell from 5.7
to 5.0 s at 20k states and from 58.9 to 56.1 s at 5 s and 64 MiB. Peak resident
memory rose only where the build now finishes and the search goes on, by
14.9 MiB at 256 MiB and 92 MiB at 1 GiB, within the budget. The wasm module grew
172 bytes, from 358,211 to 358,383.

### Freeze prune (job 49024)

The forward search first stored every reachable state, though most reach no
goal state: of the 10,253,873 that each of Grand Hall's 26-cell tables
reached, 710,100 did. Job 49024 compared `main` (`548f8a1`) with the freeze
refusal in job 49016's parts, on a Xeon Platinum 8581C with 16 cpus, with
traced copies of both builds.

Soundness: 27 proven optima, no proof that disagrees, no bound or start's
bound above a proven optimum, and no unsolvable proof. Each traced copy matched
its build in all 133 compared runs, and clippy passed. The 224 tables that
both builds finished were identical in their states that reach a goal, their
start values and their kept bytes, and reached 90.8M states in all against
324.8M. All 72 runs where both builds kept the same tables matched.

| Measure | `main` | With the refusal |
| --- | --- | --- |
| States reached by each Grand Hall 26-cell table | 10,253,873 | 2,691,825 |
| Grand Hall's build | 24.5M units | 7.5M units |
| Grand Hall's build time (15 traced runs) | 3.10 to 3.85 s | 1.06 to 1.15 s |
| Grand Hall's build peak | 172.3 MiB | 52.3 MiB |
| Grand Hall, 5 s, 256 MiB / 1 GiB | 274 to 275 / 274 | 276 / 276 |
| Grand Hall, 30 s, 1 GiB | 279 | 280 |
| Grand Hall, 5 s or 30 s, 64 MiB | 238, its tables pass the cap | 250 |
| Grand Hall, 1M states, 256 MiB | 275 in 5.7 s, 180 MiB resident | 275 in 3.1 s, 81 MiB |
| Open boards' summed bound, 20k states / 5 s, 64 MiB | 3376 / 3430 | 3403 / 3442 |

With fewer states, eight more tables build. Four
whole-room tables at 20k states replace their halves and raise the start's
bound: gen-v2-320041-027a9b04 from 52 to 66, 330146 from 73 to 86, 340114
from 80 to 82 and 350070 from 116 to 122. At 5 s and 64 MiB, two more
whole-room tables build on 330146 and 350070, and Grand Hall builds its table
of labels B, C and X. The five tables no longer built are all halves replaced
by their room's whole-room table. No bound fell. Failed tables still take most
of the spend at 64 MiB: 15.6M of 23.6M units at 20k states (18.0M of 25.1M
before), and 53.3M of 107.5M at 5 s (62.6M of 180.9M). Five tables, among
them Grand Hall's table of labels A, D and X at 64 MiB, finish the forward
search and fail in the backward one. Rooms on seven catalog boards, and Grand
Hall's at 20k states, still pass the cap.

The forward search's time per state rose from 120 to 136 ns at 20k states,
from 141 to 148 at 5 s and 64 MiB, and from 138 to 143 to 154 to 155 in Grand
Hall's repeated runs. The catalog's summed search time rose from 3.9 to 4.1 s
at 20k states and from 43.7 to 44.9 s at 5 s and 64 MiB, where the gained
tables cost build time; the proved boards' time matched. Peak resident memory
rose only where the search now runs longer, by up to 57 MiB at 256 MiB and
200 MiB at 1 GiB, within the budget. The wasm module grew 2,046 bytes, from
358,383 to 360,429.

### Compact values (job 49161)

The kept values had a power-of-two slot count at most 3/4 full, 13.3 to 26.7
bytes a value. At 5 s and 64 MiB five tables finished their forward search and
then failed the kept limit, 16.7 MB, after their backward one (job 49024): four
catalog tables, and Grand Hall's table of labels A, D and X in its 26-cell
room, beside the kept table of B, C and X. A size oracle, job 49127, built every
candidate's tables with no limit, and its sizes matched all 269 tables traced
in job 49024. At 3/4 with no padding, the four catalog tables would take 8.4 to
15.6 MB, and Grand Hall's two tables 18.9 MB together, or 16.2 MB at 7/8. Job 49161 compared `main`
(`4bebe0b`) with compact values at 3/4 and at 7/8, both with the backward
search's early failure, in job 49024's parts. It added 5 s at 32, 48, 96 and
128 MiB, 30 s at 48 and 96 MiB for Grand Hall, `large` and `open-field`, and
50k, 100k and 200k states at 64 MiB. It ran on a Xeon Platinum 8581C with 16
cpus, with traced copies of all three builds.

Soundness: 27 proven optima, no proof that disagrees, no bound or start's
bound above a proven optimum, and no unsolvable proof. Each traced copy matched
its build in all 534 compared runs. The tables that both `main` and a compact
build finished, 910 at 3/4 and 908 at 7/8, had the same states that reach a
goal and the same start values; only their kept bytes changed. The layout
changes neither which states a table keeps nor their values.

| Measure | `main` | 3/4 | 7/8 |
| --- | --- | --- | --- |
| Bytes a value | 13.3 to 26.7 | 13.3 | 11.4 |
| Kept bytes of the tables both built, against `main` | 1 | 0.778 | 0.667 |
| Tables gained / lost against `main` | | 10 / 3 | 16 / 5 |
| Backward failures at 64 MiB, 5 s or 30 s / 50k to 200k states | 6 / 3 | 3 / 1 | 0 / 0 |
| Backward failures, 32 to 128 MiB | 11 | 8 | 4 |
| Grand Hall's start bound, 64 MiB | 228 | 228 | 255 |
| Grand Hall, 5 s or 30 s, 64 MiB | 250 | 250 | 274 |
| Grand Hall, 200k states, 64 MiB | 248 | 248 | 272 |
| Grand Hall's kept tables, 96 MiB and up | 20.0 MiB | 18.1 MiB | 15.5 MiB |
| Open boards' summed bound, 5 s, 64 MiB | 3442 | 3457 | 3481 |
| The same, 5 s, 32 / 48 / 96 / 128 MiB | 3360 / 3413 / 3519 / 3537 | 3361 / 3422 / 3521 / 3540 | 3376 / 3434 / 3521 / 3547 |
| The same, 200k states, 64 MiB | 3531 | 3546 | 3570 |

At 7/8, every table that failed after its backward search at 64 MiB builds:
the four catalog tables, and Grand Hall's A, D and X table beside its B, C and
X one, which raises its start's bound from 228 to 255 and its bound at 5 s and
30 s from 250 to 274. At 128 MiB a 2.5M-value half
of gen-v2-340187 builds and raises that board from 98 to 105, and at 48 MiB
gen-v2-320081 rises from 78 to 90. Of the five tables lost, four are halves
replaced by their room's whole-room table. The fifth, a small table on
gen-v2-320041-027a9b04 at 32 MiB, no longer fits once the whole-room table
beside it builds, and that board's start's bound still rises from 52 to 67. One
start's bound fell, on the same board at 48 MiB, from 67 to 66: a gained table
worth 2 lowered the hall part by 3. Its bound at 5 s held. At 3/4, ten tables
are gained but neither of Grand Hall's, and one more start's bound fell, on
gen-v2-350070 at 32 MiB, from 115 to 114.

One bound fell below `main`'s in both compact builds: gen-v2-370002 at 5 s and
128 MiB, 173 against 174. No build keeps a table there, the builds' units
matched, and `main`'s traced copy also reached 173, so the clock decides it.

The fuller table costs the search nothing measurable. A probe passes about 4.5
slots on a hit and 32 on a miss at 7/8, against 2.5 and 8.5 at 3/4, but a miss
means a dead state, about one priced state in 70. On the 231 runs where 7/8
and `main` expanded the same states, the search less the build took 112.6 s
against 113.4 s, 4.5 ns less per priced state, within noise.

The early failure saves little, since the start, whose distance it waits for,
is usually among the last states the backward search reaches. Over the nine
backward failures that `main` and the 3/4 build shared, it saved 1.4% of their
units, 33.4M to 32.9M, and 12% on the one where it saved most. It costs one
comparison a level.

Grand Hall's build at 96 MiB and up took 1.01 to 1.04 s against 1.05 to 1.07
s (medians of the traced runs), with its peak at 50.1 MiB against 52.3. The
most reserved at 20k states fell from 13.7 MiB to 9.6. Peak resident memory
rose by at most 21.6 MiB, on gen-v2-340187 at 128 MiB where its gained table
is kept, within the budget. The wasm module shrank 61 bytes, from 360,429 to
360,368; 7/8 differs from 3/4 only in a constant.

### Hall stand charges (job 49398)

The hall part used to price a push at [end ∉ C] alone, so the keeper's walk
to each push's stand cost it nothing, even off C. Job 49398 compared `main`
(`701344b`) with the stand charges of [The hall part
M_hall](#the-hall-part-m_hall) (`b0ed347`) in job 49161's parts, on a Xeon
Platinum 8581C with 16 cpus, with traced copies of both builds.

Soundness: 27 proven optima, no proof that disagrees, no bound or start's
bound above a proven optimum, and no unsolvable proof. Each traced copy matched
its build in all 534 compared runs. The tables matched in all 324 runs where
both builds ended. M is unchanged, so the two builds also matched in all 247
compared runs without a room bound.

| Measure | `main` | Stand charges |
| --- | --- | --- |
| Grand Hall's start, hall part / S | 162 / 255 | 165 / 258 |
| The same at 48 MiB, with the B, C and X table alone | 192 / 228 | 196 / 232 |
| Grand Hall, 5 s or 30 s, 48 / 64 / 96 MiB | 250 / 274 / 275 | 253 / 277 / 278 |
| Grand Hall, 5 s, 128 MiB to 1 GiB | 276 | 279 |
| Grand Hall, 30 s, 256 MiB / 1 GiB | 277 / 280 | 280 / 283 |
| Grand Hall, 200k / 1M / 11M states | 272 / 275 / 279 | 275 / 278 / 282 |
| Open cases' summed bound, `ci` / `states` / `held` / `prod` | 3403 / 10435 / 14581 / 5314 | 3437 / 10536 / 14723 / 5364 |
| Open cases above `main`, the same parts | | 13 / 37 / 51 / 16 |
| Wasm module | 360,368 | 359,654 |

The start's hall part rose on 231 of the 298 starts with a room bound and fell
on none. A start's charge can outweigh what its matching gains when the keeper
starts far off B, but no start did. No bound fell, and no proof was gained or
lost. The largest rise at a start was 7, on gen-v2-350001, whose S rose from
148 to 155. Grand Hall rose by 3 at every limit where a table builds. At
32 MiB and at 20k to 100k states none does, and both builds reach 237 and 231
to 234.

The charges cost the search nothing measurable. Each hall part runs one more
0-1 BFS over the board in its build, and the charge of each cell was already
stored, as zeros, so the reserved bytes matched in every run where both builds
ended. On the 15 runs that both ended at the time limit, the charged build
generated 800,272 states a second against 805,443. On Grand Hall at 5 s it
expanded 1.6 to 5.4% fewer states but generated within 1.3% as many, with more
children an expanded state, 16.4 against 16.1 at 1 GiB. On the 71 runs with a
room bound where both builds expanded the same states, the search took 40.05 s
against 39.59 s, a ratio of 1.0117. On the 247 runs without one, where the two
builds run the same code, the ratio was 1.0174. Peak resident memory rose by
at most 1.2 MiB, on Grand Hall at 1 GiB. The wasm module shrank 714 bytes.

## Rejected approaches

- **A side split with box identities.** Splitting the room into two halves and
  taking the minimum over the ways to share its boxes between them fell to 50
  at Grand Hall's start. Capping each half's original boxes per label made it
  sound at 85, at the price of a minimum over partitions at every lookup. The
  label-subset tables give 93 with a single lookup and no identities.
- **The position model.** Tables over all boxes in a part, with boxes outside
  dropped and visitors deleted. Storing only the keeper's cell after a push is
  unsound as a table: after a push outside, or a visitor's push, the keeper is
  elsewhere. The move-level version is sound, but deleting visitors makes a
  part with no goals worth nothing, and lets a part take free entries from
  outside: its start values summed to 77, below the side split's 85.
- **A stored edge list.** The backward search used to walk the forward
  search's edges, stored. It peaked at 493 MiB on Grand Hall's largest table.
  The inverse move lists walk the same edges with no storage.
- **Largest room first.** Trying the 27-cell room behind Grand Hall's door
  (10,7) first wasted about 7 s on halves that passed the cap. With the skip
  and replace rules, the build never tries a room whose enclosed tables are
  already large.
- **Smallest room first.** Production's first order. On Grand Hall it paid
  23.1M units for the failed halves of the 25-cell room before it built the
  26-cell room around it (job 49011). Narrowest door first gives the same
  bounds at `big` and `prod` and halves that build. At 20k states it raises
  the open boards' summed bound by 6 more, three boards higher and one lower
  (gen-v2-320041-027a9b04, 73 to 69, still above M alone's 65).
- **Poison through any door.** The first production build skipped every room
  enclosing a failed one. On Grand Hall the failed 25-cell room then skipped the
  26-cell room around it, and no table was built (job 48969). Poison now passes
  only through a door at least as wide as the failed room's.
- **A chain skip.** Skipping every room after one fails at the cap would, on
  Grand Hall, have tried the 27-cell room in place of the 26-cell one.
- **Halves holding every label.** They repeat their failed room's forward
  search (see [Order, replacement and skips](#order-replacement-and-skips)).
  With them, gen-v2-370002 ended at 170 at 5 s and 64 MiB, against 173 for M
  alone (job 48991). Without them it reaches 173 (job 49011).
- **Tables outside the budget.** The memory budget bounds the search, so the
  tables and the build's buffers count against it. A search with too little
  memory for a room's tables builds only those that fit, and goes on with M
  where none does.
- **Skipping doomed forward builds.** Tables that pass the cap spend most of
  the failed units, so a cheap size estimate could skip them up front. The size
  oracle (job 49127) found no safe rule. No run in job 49024 ran out of work,
  so a doomed build costs only time, never another table. Sixteen tables on six
  catalog boards pass `MAX_CAP` at any limit, but a room's key bits and
  placement counts do not tell them from tables that build. Skipping a room
  inside a failed one behind a door no wider would skip gen-v2-320081's room
  behind door (8,5), 2.6M states, which builds at 1 GiB, to save about 1.75 s.
  A stronger dead-state rule in the forward search would cut them instead, but
  it needs its own soundness argument.
- **Tables over one part with disjoint labels.** The reference adds tables
  over the same cells for disjoint label sets, a sound sum of pushes. A table
  here counts every move that ends on its part, the keeper's steps included,
  so two tables over one part would count those steps twice. The reference
  measured them at +0 at Grand Hall's start.
- **More rooms.** Grand Hall has no further single-door room to table. Rooms
  with two doors, or a split of the hall, would each need a new soundness
  argument, and were not pursued.
