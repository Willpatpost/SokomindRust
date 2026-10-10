# Hall abstraction design notes

**This approach was built, proven sound, measured, and not kept.** The
code is not in the tree; this note records the design, the measurements
and the reason, so a stronger variant need not rediscover them.

The hall abstraction was an attempt to raise Optimal's lower bound over a
region too large for an exact table: Grand Hall's hall is one side of a
single door, 99 cells and 11 boxes, past the room cap of 62
cells and any exact table's state space. It promised to price the walking
across such a region that nothing else charges soundly: replaying Grand
Hall's 436-move
route (job 49466), 95 of the 178 moves between the route and S at the start
are the keeper walking between boxes in the hall, and 39 more turn one; the
hall part's charges pay 11 of the hall's 145 walk steps. The room bound's
own design is in `docs/room-bound.md`; the abstraction was to join its sum.

## The bound

Let H be a one-door region side that holds a start box or a goal and is past
the exact tables' cell cap. The kept tables' parts cover some of H's cells;
call the rest the zoneable cells. A **zone partition** cuts them into k
zones, bands of the zoned cells' longer extent, so a walk along it crosses
them all. The construction is fixed, so the build is deterministic and
native and WASM build the same table.

A **table state** is (counts, node):

- the count `counts[l][z]` of boxes of label l in each zone, for every label
  l in LAB, the labels with a start box or a goal in H;
- the keeper's zone, or one node for the whole floor outside the zones:
  the cells of kept parts, the door, and the floor beyond it alike.

The **projection** proj(s) of a real state keeps, for each label in LAB, how
many of its boxes stand on each zone's cells, and the keeper's zone when it
stands on one, the outside node otherwise. Boxes on part cells, on the door,
and outside H are dropped.

Each table move costs 1 when the real move it mirrors ends on a zone cell,
and 0 otherwise:

- **Step.** The keeper moves to an adjacent zone, or stays in its zone, or
  moves between its zone and the outside node. The abstract step graph
  allows a move between two nodes exactly when some cell of one is adjacent
  to some cell of the other, so a real step always maps to an abstract one.
  A step ending on a zone cell costs 1; one ending on a part cell, the door,
  or outside H costs 0 and leaves the outside node alone, as a step that
  stays outside a room's Q is no move at all.
- **Push inside the zones.** A LAB box moves from zone x to a free cell of
  zone t: the count at x drops, the count at t rises, and the keeper ends on
  x's node, at cost 1. The pushing keeper stands on a cell beside one of x,
  so its node is x's or adjacent to it, and a move from any other node is
  refused: every real push maps in, and the refusal keeps the keeper from
  teleporting.
- **Exit.** A LAB box in a zone is pushed onto the door, or onto a kept
  part's cells: it disappears from the counts, and the keeper ends on its
  old cell's node, at cost 1 for a zone cell, 0 for a part cell or the door,
  from a node beside the zone, as a push's.
- **Entry.** A box of label l in LAB is pushed off the door or off a kept
  part onto a free cell of a zone: the count rises, and the keeper ends on
  the outside node, at cost 0, from a node beside the outside, which the
  stand it pushed from is. An entry is allowed only while the zones
  hold fewer l boxes than the board has; the dropped boxes in parts and
  outside H can only make the true count larger, so no real entry is
  refused.
- **Push of a dropped box.** A box whose label is not in LAB, or that
  leaves and enters parts and the door: only the keeper's node moves, at
  the cost of the cell it ends on.

A **goal** state holds, for every l in LAB, exactly as many l boxes in the
zones as H's zone cells hold l goals.

The table's value R(u) is the exact least cost of a move sequence from u to
a goal state. The **part** P is every zone cell. The bound is one more term
of the room bound's sum: S = ΣR + R_abs + M_hall, with P joining the set C
of covered cells that M_hall makes free, and Optimal queues each state by
h = max(M, S) as before. One region per board gets a table in this first
cut, the side that qualifies best: a one-door side, not its door's largest,
as the one-door rooms' candidates are, at most 512 cells, whose cells no
kept part counts are more than the exact tables' cell cap of 62 and hold a
start box or a goal on one of them — chosen by the cells the abstraction
would actually zone, for a side the parts cover is nothing to zone.

## Soundness

The argument mirrors the room tables' lemmas
(`docs/room-bound.md`, Soundness), with the zoneable cells for Q and the
zones for its cells.

**Lemma A1 (simulation).** Let s → s' be a real move where s' still has a
solution. Then proj(s) → proj(s') is a table move or no move, and its table
cost is [the move's end cell is a zone cell]. Case by case, with the keeper
going from a to b and the pushed box, if any, from x to t:

- A real step: b is free in s. Its zone is the keeper's new node, adjacent
  to or equal to the old one, so the abstract step exists, and its cost is
  [b ∈ P].
- A push of a dropped box, or of a LAB box whose x and t both lie off the
  zones: only the keeper's node goes from node(a) to node(b), an abstract
  step or no move at the same cost. b held a box, so if b is a zone cell
  the count there drops — the state changed, but the move is still the
  keeper's step; the count is part of the state, not the move.
- A push of a LAB box from a zone cell x: a push inside the zones when t is
  a zone cell, an exit when t is the door or a part cell. The keeper ends
  on x, a zone cell, at cost 1. The entry cap holds: s' has a solution, so
  H holds at most the board's l boxes in all, and the zones' count is at
  most that.
- A push of a LAB box onto a zone cell t from the door or a part: an entry,
  at cost 0, allowed because the zones held fewer than the board's l boxes
  before it, since s' has a solution and the entering box was outside them.

**Lemma A2 (goal).** A solved state projects to a goal state. The parser
enforces as many goals as boxes per label, every goal holds a box of its
label, and every box stands on a goal of its label, so for each label the
boxes on zone cells number exactly the zone cells' goals of that label.

**Consequences.** R(proj(s)) ≤ cost_P(π) for every route π from s: project
π move by move (Lemma A1) and it ends on a goal state (Lemma A2). R is an
exact distance, so R(proj(s)) ≤ [end ∈ P] + R(proj(s')) for every real move
s → s', which is consistency. A step onto b costs [b ∈ P] and moves the
keeper alone; a push costs [x ∈ P] and moves one count, so a move lowers R
by at most its own cost, and S stays consistent as a sum of terms whose
move classes partition the board's cells by their end cell. h = max(M, S)
stays admissible and consistent, and a goal state still has S = 0, since
every zone goal is filled and the hall part's charges vanish on C.

**Lemma A3 (pruned table).** The table holds only the states reachable from
the start's projection that reach a goal state. Unlike an exact table's
build, no move is refused — every push and entry is allowed, since refusing
nothing can only relax — so every real move of a state with a solution maps
to an available table move (Lemma A1), and a state with a solution projects
onto a path to a goal state. A missing projection therefore proves that s
has no solution, and pruning that state is sound, like the room tables'
Lemma 3.

**Adding.** The zones are disjoint from every kept part and from the floor
outside H, so the parts P of all tables remain pairwise disjoint and the
end-cell count still partitions |π|. R_abs joins ΣR, and M_hall makes P's
cells free, so S ≤ |π| for every route π, and the sum is consistent.

## The build

The build runs after the exact rooms', whose kept parts fix the zoned
cells, and before M_hall is built, so its part joins C. It is eligible once
the rooms' candidates are tried or their work budget is spent — either way
the parts they kept are final — and it runs on its own quarter of the
build's work budget, beyond the rooms', so a build the rooms would starve
still zones the largest region; a board with no region spends none of it.
A search too short to pay even the rooms' budget never reaches the
abstraction: the build is paid for as pops, and a small state limit on a
big board ends the search before the rooms' budget is spent. On Grand Hall
that bars the abstraction below a few million states; the exact rooms
consume the whole 67-million-unit budget at any larger one.

The zones are bands of the zoned cells' longer extent, at most one band to
a coordinate, so a walk along it crosses them all. The kept parts can cut
a band apart and punch holes in it, which costs strength, never soundness,
for the nodes' adjacency is read off the cells themselves.

It has the same four phases as an exact table's (`room_bound/table.rs`),
resumable between work units and charged to the same ledger:

1. A forward breadth-first search from the start's projection stores every
   reachable abstract state. A state is a key: the count digits in a mixed
   radix, one per label and zone, above the keeper's node, which fits a
   u128 however many labels and zones the board has. The states a real
   search reaches are few — the reachable counts are far below the closed
   form below — so the search's cap is the work budget, not the space.
2. A scan seeds the goal states at distance 0.
3. A backward 0-1 breadth-first search from them walks the moves in
   reverse, with a u16 distance per state, and fails the table as soon as
   the values found no longer fit the kept limit, as an exact table's does.
4. The values of the states that reach a goal state go into the same
   open-addressing table of keys and u16 values, clamped like an exact
   table's to MAX_VALUE.

The abstract move set is generated from the zone adjacency graph and the
label counts: a keeper step for each adjacent node pair, a push for each
label, zone, and adjacent zone with a count to move, and an exit or entry
for each label at the door and each kept part. Its inverse, for the
backward search, restates each move's predicate, as `pulls` and `unentries`
do for a room.

The zone count k is the largest in 2..=8 whose closed-form state space —
the product over LAB of the ways to spread the board's boxes of that label
over k zones, times k + 1 keeper nodes — fits the kept share the ledger
offers, so a small budget builds fewer, wider zones and a large one more.

## Memory

The values table costs about 11.4 bytes a state, as an exact table's does,
and the forward search about 20 a state while it runs, all charged to the
ledger and refunded at its phases' ends. The kept table is charged to the
same quarter-of-budget share as the exact tables', and a table past that
limit fails, leaving the bound without its term rather than past its
budget. At search time a lookup builds the state's key from its box cells'
zone indices and the keeper's, one pass over the boxes, as an exact
table's projection does.

## Why it was not kept

The keep test — the same one a multi-door room's tables face, comparing S
at the start with the table and without it — dropped the abstraction on
every board it was tried on, Grand Hall included.

The cause is the granularity of a count. Owning the zone cells is what the
end-cell partition demands: every move that ends on them is the
abstraction's to count and no one else's, so the hall part gives up its
charges there. What it gives up is per-push: M_hall charged each push that
ends on a zone cell, and a box pushed many times within one zone, or
pushed out through a door or a part and back in, is nearly invisible to
counts — a within-zone push maps to a self-loop at cost 1 that no shortest
path takes, and an exit plus an entry teleports a box between any two
zones for the exit's single cost. What the abstraction charges back is per
zone boundary crossed. On Grand Hall the eleven boxes' within-hall pushes
are worth far more to M_hall than the boundaries their routes cross are
worth to the table, so the sum drops and the table goes.

The measurements, all on a 96-core x86-64 host, dated 2026-10-10, with the
code this note describes:

- Soundness: 200 generated boards with a 72-to-99-cell hall behind a
  one-cell door, all three modes against a BFS oracle at 200,000 states,
  bounds, proofs and routes agreeing; the abstraction built 24 tables and
  every one was dropped by the keep test. The search never saw one.
- Grand Hall (`huge`), Optimal at 60M states and 32768 MiB: bound 290,
  expanded 14,316,598, exactly the run without the abstraction. The exact
  rooms consume the whole 67-million-unit work budget; the abstraction,
  on its own reserved quarter, built and was dropped.
- Grand Hall at 20,000 states and 64 MiB (the gate's config) and at 1M
  states: the build's pop supply ends before the rooms' budget does, so
  the abstraction never even runs there.

What would strengthen it, in rough order of promise, none measured:

- Charge pushes the way the room tables do, at per-cell granularity, over
  a partition of the region into sub-regions small enough for exact
  tables — which is the multi-door rooms' own route, and the reason they
  win where the counts lose.
- Zone the door cell too, so an exit-and-entry teleport costs two pushes
  rather than one, and the keeper pays for crossing the threshold.
- Finer zones, where the state space allows, and per-zone lower bounds on
  the pushes a box must make to leave or to reach its goal band.
- A per-state comparison in the keep test rather than one at the start,
  which could keep tables the start undersells — at the cost of the
  gate's hard rule that an Optimal bound never drops.

The mechanism itself — the projection, the four-phase build over u128
keys, the ledger and work accounting — is recorded above in full, and the
oracle harness that exercised it (a generated-board generator and a state
limit high enough for the build to finish) was straightforward to write;
a stronger variant can plug into the same seams.
