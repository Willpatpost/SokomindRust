# Strategic guidance

The doorway guidance steers the weighted searches — Quality and Fast,
never the exact proof search — toward routes that respect the order a
room's single door imposes. It is the original Sokomind's strategic
analyzer idea, re-derived for this engine's architecture
(`crates/search/src/strategic.rs`).

## The order a door imposes

A room with one door serializes its traffic: every box that must leave
crosses ahead of every box that must enter. With `e` exports (surplus
boxes whose goals lie outside) and `i` imports (goals inside whose
boxes lie outside), flow-balance arithmetic says at least
`min(e, e − i + 1)` exports complete before the first import, or the
door's traffic deadlocks against itself. The guidance counts each
room's exports and imports per label, derives the minimum, and charges
a soft penalty to states that jump it: one unit per import beyond the
allowance, and one unit for a box parked on a gate that still owes
crossings.

The penalty is guidance, never a prune. It shifts the queue's
priorities only, following the same split the room bound already uses
(the queue may carry max(M, S) while the record keeps M): the stored
h, every admissible prune, and the exact search's key never see it.
Rooms the detection cannot pay its scratch ledger for leave the
guidance empty rather than failing the search, and rooms without
crossings in both directions impose no order and are dropped, so the
per-state walk stays proportional to the rooms that can actually be
mis-sequenced.

## The measurement (2026-10-16)

The corpus, paired at 1M states / 64 MiB, quality mode, the knob off
versus on: 57 boards — **2 better, 0 worse, 48 unchanged** (7 boards
did not solve at the budget, both runs alike). Both improvements hold
at 4M states / 256 MiB:

- `gen-v2-340272-fd819d0e`: 255 → 251 moves;
- `gen-v2-350070-6be05c02`: 217 → 209 moves.

Grand Hall itself is unchanged — 424 at the standing 60M / 256 MiB
configuration — and instructively so: its best route's room work (all
six exports before any import) already exceeds the flow-balance
minimum, so the guidance has nothing to charge it. The steering is
real — the paired 1M-state run expanded 5% fewer states for the same
route — but Grand Hall's slack lives in its hall fetch pattern, not
its doorway order.

The knob is `SOKOMIND_STRATEGIC_GUIDANCE`, unset by default, native
only (wasm has no environment to read).
