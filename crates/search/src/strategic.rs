//! The doorway guidance: a soft, order-aware penalty for the weighted
//! searches, derived from the original Sokomind's strategic analyzer.
//! A room with a single door serializes its traffic — every box that
//! must leave crosses ahead of every box that must enter — and
//! flow-balance arithmetic bounds how many exports should complete
//! before the first import: with `e` exports and `i` imports, at least
//! `min(e, e - i + 1)` boxes have to be out before the first one is
//! in, or the door's traffic deadlocks against itself. The penalty
//! charges states that jump the recommendation, and boxes parked on a
//! gate that still owes crossings. It is guidance, never a prune: the
//! engine adds it only to the weighted policies' keys, behind
//! `SOKOMIND_STRATEGIC_GUIDANCE`, and the exact search's admissible
//! key never sees it.
use sokomind_core::{Board, Cell, NONE, State};

/// Whether `SOKOMIND_STRATEGIC_GUIDANCE` asks for the doorway guidance.
/// Native only: wasm has no environment to read.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn enabled() -> bool {
    std::env::var_os("SOKOMIND_STRATEGIC_GUIDANCE").is_some()
}

/// The wasm twin: no environment, so no guidance.
#[cfg(target_arch = "wasm32")]
pub(crate) fn enabled() -> bool {
    false
}

/// One single-door room's crossing bookkeeping. Rooms without
/// crossings in both directions impose no order and are dropped at
/// build, so the per-state walk stays proportional to the rooms that
/// can actually be mis-sequenced.
struct GuidanceRoom {
    /// The room's cells, as a board-length membership mask.
    cells: Vec<bool>,
    /// The gate: the room cell the room's single door feeds.
    gate: Cell,
    /// The room's goal counts per label.
    goal_counts: Vec<(u8, usize)>,
    /// The surplus at the start: boxes inside whose label has fewer
    /// goals inside, every one of which must leave.
    exports: usize,
    /// The deficit at the start: goals inside whose label has fewer
    /// boxes inside, every one of which must be brought in.
    imports: usize,
    /// The flow-balance minimum: exports that should complete before
    /// the first import.
    minimum_first: usize,
}

/// The doorway guidance over a board's single-door rooms, built once
/// per search and evaluated per state.
pub(crate) struct DoorwayGuidance {
    rooms: Vec<GuidanceRoom>,
    /// Each box slot's label, from the start's row-major order.
    box_labels: Vec<u8>,
}

impl DoorwayGuidance {
    /// The guidance over a board's gated rooms, from the same room
    /// detection the table build uses. A detection that cannot pay
    /// its scratch ledger returns no rooms, and the guidance runs
    /// empty rather than failing the search.
    pub(crate) fn analyze(board: &Board, start: &State) -> Self {
        Self::build(board, start, &crate::room_bound::gated_rooms(board, start))
    }

    /// Builds the guidance over the single-door `rooms` (each the
    /// doors and the cells a room detection found): the per-label
    /// counting, the crossing totals, and the flow-balance minimum.
    /// Rooms that owe no crossings in both directions are dropped.
    pub(crate) fn build(board: &Board, start: &State, rooms: &[(Vec<Cell>, Vec<Cell>)]) -> Self {
        let cells = board.tiles().len();
        let box_labels = board.labels().to_vec();
        let mut guidance = Vec::new();
        for (doors, room_cells) in rooms {
            let [door] = doors[..] else {
                continue;
            };
            let mut mask = vec![false; cells];
            for &cell in room_cells {
                mask[usize::from(cell)] = true;
            }
            let neighbors = board.neighbors();
            let Some(gate) = room_cells
                .iter()
                .copied()
                .find(|&cell| neighbors[usize::from(cell)].contains(&door))
            else {
                continue;
            };
            let mut goal_counts: Vec<(u8, usize)> = Vec::new();
            for &(cell, label) in board.goals() {
                if mask[usize::from(cell)] {
                    bump(&mut goal_counts, label);
                }
            }
            let mut inside: Vec<(u8, usize)> = Vec::new();
            for (slot, &cell) in start.boxes.iter().enumerate().take(box_labels.len()) {
                if cell != NONE && mask[usize::from(cell)] {
                    bump(&mut inside, box_labels[slot]);
                }
            }
            let (mut exports, mut imports) = (0, 0);
            for &(label, goals) in &goal_counts {
                let boxes = count(&inside, label);
                exports += boxes.saturating_sub(goals);
                imports += goals.saturating_sub(boxes);
            }
            for &(label, boxes) in &inside {
                if count(&goal_counts, label) == 0 {
                    exports += boxes;
                }
            }
            if exports == 0 || imports == 0 {
                continue;
            }
            let minimum_first = exports.min(exports.saturating_sub(imports) + 1).max(1);
            guidance.push(GuidanceRoom {
                cells: mask,
                gate,
                goal_counts,
                exports,
                imports,
                minimum_first,
            });
        }
        Self {
            rooms: guidance,
            box_labels,
        }
    }

    /// The soft penalty at `state`: one unit per import that jumped
    /// the flow-balance minimum (counted against the exports already
    /// completed), plus one unit for a box parked on a gate that still
    /// owes crossings. Zero over an empty guidance.
    pub(crate) fn penalty(&self, state: &State) -> u32 {
        let mut total = 0u32;
        for room in &self.rooms {
            let mut inside: Vec<(u8, usize)> = Vec::new();
            for (slot, &cell) in state.boxes.iter().enumerate().take(self.box_labels.len()) {
                if cell != NONE && room.cells[usize::from(cell)] {
                    bump(&mut inside, self.box_labels[slot]);
                }
            }
            let mut surplus = 0;
            let mut deficit = 0;
            for &(label, goals) in &room.goal_counts {
                let boxes = count(&inside, label);
                surplus += boxes.saturating_sub(goals);
                deficit += goals.saturating_sub(boxes);
            }
            for &(label, boxes) in &inside {
                if count(&room.goal_counts, label) == 0 {
                    surplus += boxes;
                }
            }
            let completed = room.exports.saturating_sub(surplus);
            let imported = room.imports.saturating_sub(deficit);
            let allowed = if completed < room.minimum_first {
                0
            } else if completed >= room.exports {
                room.imports
            } else {
                let permanent = room.exports.saturating_sub(room.imports);
                room.imports.min(completed.saturating_sub(permanent))
            };
            total += imported.saturating_sub(allowed) as u32;
            if surplus + deficit > 0
                && state
                    .boxes
                    .iter()
                    .take(self.box_labels.len())
                    .any(|&cell| cell == room.gate)
            {
                total += 1;
            }
        }
        total
    }
}

/// Adds one to `label`'s count in the small per-label tally.
fn bump(counts: &mut Vec<(u8, usize)>, label: u8) {
    if let Some(entry) = counts.iter_mut().find(|(other, _)| *other == label) {
        entry.1 += 1;
    } else {
        counts.push((label, 1));
    }
}

/// `label`'s count in the small per-label tally.
fn count(counts: &[(u8, usize)], label: u8) -> usize {
    counts
        .iter()
        .find(|(other, _)| *other == label)
        .map_or(0, |&(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One gated room: two X boxes and an S goal inside (one X must
    /// leave for the S below the door), an a goal inside and its A box
    /// outside (the A must enter). The flow-balance minimum is one
    /// export before the import.
    fn board() -> Board {
        Board::parse(
            "OOOOOOOOO\n\
             OXX S a O\n\
             OOO OOOOO\n\
             OA S  R O\n\
             OOOOOOOOO",
        )
        .unwrap()
    }

    /// The room above the door: row 1's interior, fed by the door at
    /// r2c3 through the gate r1c3.
    fn room() -> (Vec<Cell>, Vec<Cell>) {
        (vec![21], (10..=16).map(|cell| cell as Cell).collect())
    }

    #[test]
    fn imports_wait_for_the_flow_balance_minimum() {
        let board = board();
        let start = board.initial();
        let guidance = DoorwayGuidance::build(&board, &start, &[room()]);
        assert_eq!(guidance.rooms.len(), 1);
        assert_eq!(guidance.rooms[0].exports, 1);
        assert_eq!(guidance.rooms[0].imports, 1);
        assert_eq!(guidance.rooms[0].minimum_first, 1);
        assert_eq!(guidance.penalty(&start), 0);

        // The A jumps in before any X leaves: one violation. The A is
        // slot 0 — the parse groups boxes by label, A before X.
        let mut boxes = start.boxes;
        boxes[0] = 15;
        assert_eq!(
            guidance.penalty(&State {
                boxes,
                player: start.player
            }),
            1
        );

        // One X out first: the import is now within its allowance.
        let mut boxes = start.boxes;
        boxes[0] = 15;
        boxes[1] = 30;
        assert_eq!(
            guidance.penalty(&State {
                boxes,
                player: start.player
            }),
            0
        );
    }

    #[test]
    fn a_box_parked_on_a_working_gate_is_charged() {
        let board = board();
        let start = board.initial();
        let guidance = DoorwayGuidance::build(&board, &start, &[room()]);
        let mut boxes = start.boxes;
        boxes[1] = 12;
        assert_eq!(
            guidance.penalty(&State {
                boxes,
                player: start.player
            }),
            1
        );
    }

    #[test]
    fn rooms_without_crossings_in_both_directions_are_dropped() {
        let board = Board::parse(
            "OOOOOOOOO\n\
             OX  S   O\n\
             OOO OOOOO\n\
             O   R   O\n\
             OOOOOOOOO",
        )
        .unwrap();
        let start = board.initial();
        let guidance = DoorwayGuidance::build(
            &board,
            &start,
            &[(vec![21], (10..=16).map(|cell| cell as Cell).collect())],
        );
        assert!(guidance.rooms.is_empty());
        assert_eq!(guidance.penalty(&start), 0);
    }
}
