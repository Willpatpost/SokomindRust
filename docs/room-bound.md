# Room bound design notes

The room bound (`crates/search/src/room_bound.rs` and `room_bound/`) raises
Optimal's lower bound with exact tables for the board's single-door rooms. The
module docs say what each part does. This file records why the bound is sound,
how its build fits the search's memory and time, the measurements behind it,
with the SLURM job that produced each number, and the approaches it replaced.

On Grand Hall (`huge`) it raises the start's bound from 214 to 255, and the
lower bound at 30 s and 1 GiB from 244 to 278 (see
[Measurements](#measurements)).

## The bound

A door is a floor cell whose removal splits the keeper's floor, boxes ignored.
A room Q is one side of a door. Each room that holds a start box or a goal gets
one or two tables R(Q, P, LAB). A table is the exact cost of a relaxed problem
over Q that keeps only the boxes of the labels LAB, counting only the moves
that end on its part P ⊆ Q. Let C be the union of the parts. The hall part
M_hall is the assignment heuristic with every push that ends on C made free.
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
must end on a goal in Q, and a dead projection means s has no solution.

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

M_hall is the assignment heuristic M with different reverse-push costs: a push
costs 1 when the keeper ends outside C, that is, when the box's old cell is
outside C, and 0 otherwise. It has no stand charges. A charge stands for a
walk whose cells may lie in C, where the tables already count them. It keeps
M's keeper sides, since every real box path is a path in their graph, so its
feasibility equals M's.

- *Admissible.* Each box's pushes that end outside C cost at least its M_hall
  distance to its final goal. Different boxes' pushes are different moves, and
  the final placement is one assignment. So M_hall(s) ≤ cost_{outside C}(π).
- *Consistent.* A step moves no box. A push moves one box along one edge of
  cost c = [end ∉ C], and under the same assignment M_hall drops by at most c.

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
- M_hall bounds pushes only; it does not count walking outside C.
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
`room_bound.rs` proves it). So the build tries them smallest first, and one
cell of a room built earlier tells whether a candidate encloses it.

A table's label set is the labels with a start box or a goal on its part.

### A table's build

The build of one table (`room_bound/table.rs`) has four phases, each resumable
between work units:

1. A forward breadth-first search from the start's projection stores every
   reachable state, up to a cap. A state is a `u64` key, a code per cell of Q
   and the node above them, kept in fixed chunks of 65,536 that never move,
   with an open-addressing table of `u32` ids at most 3/4 full.
2. A scan of the states seeds the goal states at distance 0.
3. A backward 0/1 breadth-first search from them walks the moves in reverse,
   with a `u16` distance per state and two level worklists. It generates each
   predecessor from the room's inverse move lists (`pulls` and `unentries` in
   `room_bound/room.rs`) and keeps those the forward search reached. A
   predecessor test restates the forward move's predicate, so the backward
   search walks exactly the forward edges without storing them.
4. The values of the states that reach a goal go into an open-addressing table
   of keys and `u16` values, at most 3/4 full, 10 bytes a slot. This is the
   only part the search keeps.

A unit expands, pops or scans one state, or scans 16 states while seeding or
inserting values. The forward search costs about 8 bytes a state for its key,
and 5 to 11 for its slot table, more while the table doubles. The backward
search adds 2 for the distance and up to 8 for the worklists.

### Order, replacement and skips

The build tries each candidate, smallest first, unless:

- it encloses a room whose build failed, so its own would very likely fail too;
- or the tables of the rooms it encloses reached more than cap / 16
  (`ENCLOSED_SHARE`) states in their forward searches: its own search holds
  theirs, so it would very likely pass the cap and waste its work.

A room that encloses no built room tries its whole-room table, then, if that
failed or its key needs more than 63 bits, the two halves of the room, split at
the door's line, when neither is empty. It keeps whatever builds. A room that
encloses built rooms tries only its whole-room table, and replaces them if it
builds; otherwise they stay. A failure is a forward search past the cap, a
buffer the ledger or the allocator refuses, a table past the kept limit, or the
work budget running out. It marks the room, so every room that encloses it is
skipped.

A replacement never lowers the tables' sum, so the build keeps the larger room
without comparing the two at the start. R(C, C, LAB_C) is at least the sum of
the enclosed rooms' tables R(E, P, LAB) in every state: each LAB lies in
LAB_C, each P in C, and the parts are disjoint. An optimal solution of C's
relaxed problem, cut down to E's labels and cells, is a solution of E's by
Lemma 1's projection, and it counts a move on P only where C's counts that
move. The hall part may drop, since the pushes that end in C outside the
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

When the build ends with tables, the engine clears the arena, which holds only
the start, and sizes it again with `Arena::recharge`, with the tables and the
hall part counted among the fixed buffers. The new limit is never larger, so
the re-size allocates nothing, and a search that lost states to it reports
`MemoryLimit` rather than `StateLimit` when it fills. If even the first block
no longer fits, the search drops the tables and goes on with M alone.

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

### After the build

The start is seeded again in the re-sized arena, queued with max(M, S). The
second seed counts the start again in generated and unique states, as a
restart's does, so the two still differ by the improvements alone. With no
table, the search goes on with the start it seeded first.

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
  (10,7) first wasted about 7 s on halves that passed the cap. Smallest first,
  with the skip and replace rules, builds the nested 26-cell room's tables
  first and never tries a room whose enclosed tables are already large.
- **Tables outside the budget.** The memory budget bounds the search, so the
  tables and the build's buffers count against it. A search with too little
  memory for a room's tables builds only those that fit, and goes on with M
  where none does.
