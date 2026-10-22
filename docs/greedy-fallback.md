# Greedy fallback

Could a constructor that never searches route the boards every search
leaves routeless? The audit behind the routeless-finish panel asked it
of the two catalog boards no mode routes — gen-v2-340048-b5d312a6
("False Promise: Chambers 7") and gen-v2-350001-a996dcbc ("False
Promise: Chambers 4"), both state-limit in Fast, Quality and Optimal —
because "anytime fallback" in the app's worker would have to mean
exactly that: no arena, no state table, no budget beyond its own push
count. The measured answer is no. A matching-score hill climb descends
most of the way on the smaller board and freezes, and barely descends
at all on the larger one; neither routes within hundreds of thousands
of restarts. The panel's steering message stands as the honest answer:
the budgets that route these boards are the app's only route to them.

## The constructor

The shape under test, `crates/search/examples/greedy.rs`, is the
cheapest constructor that could plausibly route a board: flood the
player from the live position; list every legal push whose target cell
is free and live for the box's label — live meaning some push chain
still reaches a goal of that label, other boxes aside, which one
breadth-first flood of reverse pushes per goal decides and doubles as
the matching distances; score each push's resulting position by a
slot-order greedy matching, each box claiming its nearest unclaimed
same-label goal and the score summing their push distances; take one
of the best-scoring pushes at random — ties only, so each restart
explores a different corridor — and end the attempt when no push
improves the score or its push cap (four times the initial score, plus
64) circles. Seeds are fixed per attempt, so a record printed once
prints again; a route that reaches the solved position is replayed
strictly before it is reported.

## The measurement (2026-10-22)

Development host, release build, 60-second walls,
`--attempts 400000`. Chambers 7 (12 boxes, matching distance 58 at the
start):

| Restarts | Push decisions | Best score | Routed |
| ---: | ---: | ---: | --- |
| 2,000 | 42,589 | 9 | no |
| 400,000 | 8,581,190 | 9 | no |

The climb is deterministic in its ceiling: every block of restarts
reaches a matching distance of 9 and freezes there — the boxes sit
wedged where the last pushes need a reshuffle no improving push can
start. Chambers 4 (22 boxes, matching distance 142 at the start) never
gets close: the wall stops it at 286,425 restarts, 7,735,207 push
decisions, best score 112. The maze's pushes are too entangled for a
one-push lookahead to descend — most attempts die within a handful of
pushes.

The solver's own counters say why: these boards prune 80,447 and
355,344 deadlock states respectively in a 20,000-state Quality run —
they are built out of frozen configurations, the exact thing a greedy
constructor cannot undo. Routing them takes the search's state
deduplication and reopening, which is what the budgets the panel
steers toward buy.
