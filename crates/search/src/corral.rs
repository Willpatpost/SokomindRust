//! Sealed-corral dead states, checked once per expansion right after the
//! keeper flood. A corral is a 4-connected component of the cells the
//! keeper cannot reach, box cells included, flooded from one of its boxes.
//! Its empty cells border only walls and its own cells: an empty cell next
//! to a reached one would be reached, and any other neighbor would join the
//! component. So until one of its boxes moves, neither the keeper nor a box
//! from outside enters it, and the first push of one of its boxes in any
//! solution starts from a stand the keeper reaches now and lands on a cell
//! with no box now. Those potential pushes are judged as if every box
//! outside the corral were absent: a push is fatal when it lands on a dead
//! cell for its label, or when the freeze fixpoint over the corral's boxes
//! alone, after the push, holds one off its goal. Boxes outside can only
//! hold more axes, so the push is fatal wherever they go by then. A corral
//! whose potential pushes are all fatal never moves, and the state has no
//! solution when it holds a box off its goal or an empty goal cell, which
//! needs a box from outside that can never get in.
//!
//! Boxes outside the corral never block anything here: treating a box that
//! may still move as a permanent blocker is the false proof Sokomind2's
//! corral check made (audit F-001). Only the dead-state check is ported;
//! restricting a state's successors to corral pushes and ordering corrals
//! are push-objective techniques that lose routes or proofs under the move
//! objective, so neither is.
//!
//! A corral with no empty cell is skipped, a choice of scope and cost: its
//! boxes all border the keeper's region, and judging such corrals too flags
//! far more states but no more that the admit chain keeps (the census in
//! pruning/t2-census.json, not tracked, finds the same 1,276 and 620 kept
//! flags on the rooms and the catalog tested below either way). When the
//! reached cells and the boxes cover every floor cell, every corral is
//! such a one, so the scan is skipped outright. A corral with more than
//! [`CAP`] potential pushes is skipped too, which bounds the freeze work
//! per expansion; one with none is always judged.
use crate::{deadlock::Deadlock, heuristic::Heuristic, reach::Reach};
use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, WALL};
use std::mem::size_of;

/// Potential pushes judged per corral at most; a corral with more is
/// skipped. The reference's PI-corral check caps a corral's boundary boxes
/// at six instead (`MAX_BOUNDARY_BOXES` in pi-corral.ts); capping the
/// pushes is equally sound and bounds the work directly.
const CAP: usize = 6;

/// A potential push of box `i` from `from` to `to`.
#[derive(Clone, Copy)]
struct Push {
    i: usize,
    from: Cell,
    to: Cell,
}

/// Sealed-corral detection with epoch stamps: each corral bumps the epoch
/// instead of clearing the stamps, so a check costs only the cells its
/// corrals cover. It only ever answers "dead" for states from which no
/// solution exists, so every mode may prune with it freely.
pub(crate) struct Corral {
    /// Cell -> epoch of the last corral that covered it. A stamp at or
    /// above the first epoch of the current check marks a cell already
    /// covered by it.
    stamps: Vec<u32>,
    epoch: u32,
    /// The current corral's cells, in flood order.
    queue: Vec<Cell>,
    /// Non-wall cells on the board.
    floor: usize,
}

/// The set bits of `mask`, lowest first: the box slots a members mask holds.
fn bits(mut mask: u32) -> impl Iterator<Item = usize> {
    std::iter::from_fn(move || {
        (mask != 0).then(|| {
            let j = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            j
        })
    })
}

/// The potential pushes of the boxes in `members`, box-major and in
/// direction order, as the expansion's static check would find them: a
/// stand the last fill reached and a destination with no box. `None` when
/// there are more than [`CAP`].
fn pushes(
    board: &Board,
    boxes: &[Cell],
    reach: &Reach,
    members: u32,
) -> Option<([Push; CAP], usize)> {
    let mut pushes = [Push {
        i: 0,
        from: NONE,
        to: NONE,
    }; CAP];
    let mut count = 0;
    for i in bits(members) {
        let from = boxes[i];
        let neighbors = board.neighbors()[from as usize];
        for (d, &opposite) in OPPOSITE.iter().enumerate() {
            let (to, stand) = (neighbors[d], neighbors[opposite]);
            if to == NONE || stand == NONE || reach.blocked(to) || reach.distance(stand) == NONE {
                continue;
            }
            if count == CAP {
                return None;
            }
            pushes[count] = Push { i, from, to };
            count += 1;
        }
    }
    Some((pushes, count))
}

impl Corral {
    /// The stamps and the queue.
    pub(crate) const BYTES_PER_CELL: usize = size_of::<u32>() + size_of::<Cell>();
    /// Empty buffers for `board`, allocated once.
    pub(crate) fn new(board: &Board) -> Self {
        let cells = board.tiles().len();
        Self {
            stamps: vec![0; cells],
            epoch: 0,
            queue: Vec::with_capacity(cells),
            floor: board.tiles().iter().filter(|&&tile| tile != WALL).count(),
        }
    }
    /// Whether some corral of the state with `boxes` leaves no solution.
    /// `reach` must hold the state's fill and `deadlock` its refresh, as
    /// they do in the engine's expansion before the first push. `dead_pair`
    /// is the freeze rule's dead-pair case, as admit passes it; the
    /// dead-cell test always uses `heuristic`.
    pub(crate) fn is_dead(
        &mut self,
        board: &Board,
        boxes: &[Cell],
        reach: &Reach,
        deadlock: &Deadlock,
        heuristic: &Heuristic,
        dead_pair: Option<&Heuristic>,
    ) -> bool {
        if reach.reached() + boxes.len() == self.floor {
            return false;
        }
        // A check bumps the epoch once per corral, at most once per box, so
        // zeroing the stamps whenever fewer than MAX_BOXES epochs are left
        // keeps it from wrapping inside a check.
        if self.epoch > u32::MAX - MAX_BOXES as u32 {
            self.stamps.fill(0);
            self.epoch = 0;
        }
        let first = self.epoch + 1;
        for (i, &seed) in boxes.iter().enumerate() {
            if self.stamps[seed as usize] >= first {
                // An earlier box's corral already covered this one.
                continue;
            }
            let members = self.flood(board, boxes, reach, i);
            if self.queue.len() == members.count_ones() as usize
                || !self.goal_open(board, boxes, reach, members)
            {
                continue;
            }
            let Some((pushes, count)) = pushes(board, boxes, reach, members) else {
                continue;
            };
            if pushes[..count].iter().all(|push| {
                heuristic.dead(push.i, push.to)
                    || deadlock.is_dead_after_push(board, dead_pair, members, push.from, push.to)
            }) {
                return true;
            }
        }
        false
    }
    /// Floods the corral of box `i` into the queue under a fresh epoch and
    /// returns its boxes as a members mask. A box before `i` in it would
    /// have covered `i` already, so only `i` and later boxes are tested.
    fn flood(&mut self, board: &Board, boxes: &[Cell], reach: &Reach, i: usize) -> u32 {
        self.epoch += 1;
        let epoch = self.epoch;
        self.queue.clear();
        self.queue.push(boxes[i]);
        self.stamps[boxes[i] as usize] = epoch;
        let mut head = 0;
        while head < self.queue.len() {
            let cell = self.queue[head];
            head += 1;
            for &next in &board.neighbors()[cell as usize] {
                if next != NONE
                    && reach.distance(next) == NONE
                    && self.stamps[next as usize] != epoch
                {
                    self.stamps[next as usize] = epoch;
                    self.queue.push(next);
                }
            }
        }
        if self.queue.len() == 1 {
            // A box with only reached cells and walls around it, a common
            // corral, needs no scan.
            return 1 << i;
        }
        boxes
            .iter()
            .enumerate()
            .skip(i)
            .filter(|&(_, &cell)| self.stamps[cell as usize] == epoch)
            .fold(0, |members, (j, _)| members | (1 << j))
    }
    /// Whether the corral in the queue still needs a box to move: one of
    /// `members` is off its goal, or a goal cell in it has no box. Without
    /// either it is settled, and leaving it alone is no deadlock.
    fn goal_open(&self, board: &Board, boxes: &[Cell], reach: &Reach, members: u32) -> bool {
        bits(members).any(|j| !board.on_goal(j, boxes[j]))
            || self.queue.iter().any(|&cell| {
                let tile = board.tiles()[cell as usize];
                tile != 0 && tile != WALL && !reach.blocked(cell)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Corral, bits, pushes};
    use crate::{
        Status,
        deadlock::{ALL_BOXES, Deadlock},
        engine::{Engine, Policy},
        heuristic::Heuristic,
        reach::Reach,
        testkit::{Lcg, catalog, explored_catalog, random_room, remaining, solvable_states},
    };
    use sokomind_core::{Board, Cell, MAX_BOXES, NONE, State};
    use std::mem::size_of;

    /// Two X boxes stacked in a one-cell door and the room below it, both
    /// goals above: the keeper reaches only the goal row, no push is legal,
    /// and the door and the room are one corral with empty cells and no
    /// potential push.
    const SEALED: &str = "OOOOOOO\nOSS R O\nOOOXOOO\nO  X  O\nO     O\nO     O\nOOOOOOO";
    /// Audit F-001's board, which Sokomind2's corral check reported
    /// unsolvable; pushing the upper X down twice and the other one left
    /// solves it in 3.
    const CROSSING: &str = "OOOOOO\nOOOROO\nOOOXOO\nOSX OO\nOOOSOO\nOOOOOO";
    /// A closes the pocket above it. Its only potential pushes go left onto
    /// a dead corner and right against B, which can still slide right to
    /// its goal first: the start is solvable, and only a check that counts
    /// B as a blocker freezes A and B against the wall above them.
    const OUTSIDE_BOX: &str = "OOOOOOOO\nOO OOOOO\nO A BabO\nORO    O\nO      O\nOOOOOOOO";
    /// Three boxes, each with floor on every side.
    const OPEN: &str = "OOOOOOO\nO     O\nO X X O\nO  X  O\nOSSS RO\nOOOOOOO";
    /// A must settle on its goal in the corridor only after B has passed
    /// it to the goal below; pushed in first, A seals B's goal off.
    const SETTLED: &str = "OOOOOOO\nO     O\nO BRA O\nOOOaOOO\nO  b  O\nOOOOOOO";

    fn at(board: &Board, row: usize, column: usize) -> Cell {
        (row * board.width() + column) as Cell
    }

    /// The buffers the engine hands the detector, filled for one state at a
    /// time as an expansion fills them.
    struct Checker {
        board: Board,
        heuristic: Heuristic,
        reach: Reach,
        deadlock: Deadlock,
        corral: Corral,
    }
    impl Checker {
        fn new(board: &Board) -> Self {
            Self {
                board: board.clone(),
                heuristic: Heuristic::new(board),
                reach: Reach::new(board.tiles().len()),
                deadlock: Deadlock::new(board),
                corral: Corral::new(board),
            }
        }
        /// Fills and refreshes for the state, then runs the detector with
        /// the dead-pair case, as the engine does.
        fn dead(&mut self, player: Cell, boxes: &[Cell]) -> bool {
            let mut state = State {
                player,
                boxes: [NONE; MAX_BOXES],
            };
            state.boxes[..boxes.len()].copy_from_slice(boxes);
            self.reach.fill(&self.board, &state);
            self.deadlock.refresh(boxes);
            self.corral.is_dead(
                &self.board,
                boxes,
                &self.reach,
                &self.deadlock,
                &self.heuristic,
                Some(&self.heuristic),
            )
        }
        /// Whether the admit chain keeps the state `dead` last saw: no box
        /// on a dead cell and none frozen off its goal over every box.
        fn admitted(&self, boxes: &[Cell]) -> bool {
            let h = &self.heuristic;
            boxes.iter().enumerate().all(|(i, &cell)| !h.dead(i, cell))
                && !boxes.iter().any(|&cell| {
                    self.deadlock
                        .is_dead_after_push(&self.board, Some(h), ALL_BOXES, cell, cell)
                })
        }
    }

    /// Runs the exact policy on `board` from its start to a terminal
    /// status, with every prune on.
    fn exact(board: &Board, max_states: usize) -> Engine {
        let mut engine = Engine::new(
            board.clone(),
            board.initial(),
            Policy::EXACT,
            max_states,
            64,
        )
        .unwrap();
        while engine.status() == Status::Running {
            engine.advance(1 << 10);
        }
        engine
    }

    #[test]
    fn bits_lists_each_slot_once() {
        assert_eq!(bits(0).count(), 0);
        assert_eq!(bits(0b1010_0001).collect::<Vec<_>>(), [0, 5, 7]);
        assert_eq!(bits(u32::MAX).count(), MAX_BOXES);
    }

    #[test]
    fn bytes_per_cell_matches_the_buffers() {
        let board = Board::parse(SETTLED).unwrap();
        let corral = Corral::new(&board);
        assert_eq!(
            corral.stamps.len() * size_of::<u32>() + corral.queue.capacity() * size_of::<Cell>(),
            board.tiles().len() * Corral::BYTES_PER_CELL
        );
    }

    /// The keeper reaches 5 of the 21 floor cells, so the scan runs. The
    /// door and the room below are one corral with both boxes, empty cells
    /// and no potential push, and its boxes are off their goals. Neither
    /// box is on a dead cell or frozen, so the admit chain alone keeps the
    /// state. The Python port of this module
    /// (`pruning/replicas/corral_port.py fixtures`, not tracked, line f1)
    /// finds 5 reachable states, one per keeper cell, all flagged and none
    /// solvable.
    #[test]
    fn sealed_corral_is_dead() {
        let board = Board::parse(SEALED).unwrap();
        let start = board.initial();
        let boxes = &start.boxes[..2];
        assert_eq!(boxes, [at(&board, 2, 3), at(&board, 3, 3)]);
        let mut checker = Checker::new(&board);
        assert!(checker.dead(start.player, boxes));
        assert_eq!((checker.reach.reached(), checker.corral.floor), (5, 21));
        assert!(checker.admitted(boxes));
        let solvable = solvable_states(&board);
        assert_eq!(solvable.len(), 5);
        for (&(player, cells), &has_solution) in &solvable {
            assert!(!has_solution);
            assert!(checker.dead(player, &cells[..2]), "{player}");
        }
    }

    /// A check that starts with exactly `MAX_BOXES` epochs left runs
    /// without zeroing the stamps and spends one; the next one, with fewer
    /// left, zeroes them and starts over at epoch 1. Both see the same
    /// corral (`corral_port.py wrap`).
    #[test]
    fn epochs_restart_before_they_wrap() {
        let board = Board::parse(SEALED).unwrap();
        let start = board.initial();
        let mut checker = Checker::new(&board);
        checker.corral.epoch = u32::MAX - MAX_BOXES as u32;
        assert!(checker.dead(start.player, &start.boxes[..2]));
        assert_eq!(checker.corral.epoch, u32::MAX - MAX_BOXES as u32 + 1);
        assert!(checker.dead(start.player, &start.boxes[..2]));
        assert_eq!(checker.corral.epoch, 1);
        assert!(checker.corral.stamps.iter().all(|&stamp| stamp <= 1));
    }

    /// Audit F-001: none of the board's 10 reachable states is flagged, so
    /// the exact policy still finds 3 (`corral_port.py fixtures`, line
    /// f2). The board guards the reported false proof but cannot catch its
    /// cause: freezing over every box flags none of its states either;
    /// `a_box_outside_the_corral_is_floor` does.
    #[test]
    fn a_corral_with_a_live_push_survives() {
        let board = Board::parse(CROSSING).unwrap();
        let mut checker = Checker::new(&board);
        let solvable = solvable_states(&board);
        assert_eq!(solvable.len(), 10);
        for &(player, cells) in solvable.keys() {
            assert!(!checker.dead(player, &cells[..2]), "{player}");
        }
        let engine = exact(&board, 1_000);
        assert_eq!(
            (engine.status(), engine.best_moves()),
            (Status::Solved, Some(3))
        );
        assert_eq!(engine.stats().pruned_corrals, 0);
    }

    /// A box outside the corral counts as floor. A's push right lands
    /// beside B, and the freeze over every box holds the pair against the
    /// wall above; over A alone it does not. B lies outside the corral and
    /// can be pushed right to its goal before A moves, so it must not count
    /// as a blocker. The start is solvable and not flagged, while a detector
    /// that froze over every box flags it. `corral_port.py fixtures`, line
    /// f4, finds 233 reachable states, 109 solvable, 44 flagged and none
    /// of those solvable; freezing over every box flags 59, 15 of them
    /// solvable, the start among them. The exact policy finds 18 with the
    /// corral on or off.
    #[test]
    fn a_box_outside_the_corral_is_floor() {
        let board = Board::parse(OUTSIDE_BOX).unwrap();
        let start = board.initial();
        let (a, b) = (at(&board, 2, 2), at(&board, 2, 4));
        assert_eq!(start.boxes[..2], [a, b]);
        let mut checker = Checker::new(&board);
        assert!(!checker.dead(start.player, &start.boxes[..2]));
        let (left, right) = (at(&board, 2, 1), at(&board, 2, 3));
        assert!(checker.heuristic.dead(0, left) && !checker.heuristic.dead(0, right));
        let pushed = |members| {
            checker
                .deadlock
                .is_dead_after_push(&board, Some(&checker.heuristic), members, a, right)
        };
        assert!(!pushed(1) && pushed(ALL_BOXES));
        let solvable = solvable_states(&board);
        let mut flagged = 0;
        for (&(player, cells), &has_solution) in &solvable {
            if checker.dead(player, &cells[..2]) {
                flagged += 1;
                assert!(!has_solution, "{player} {:?}", &cells[..2]);
            }
        }
        assert_eq!((solvable.len(), flagged), (233, 44));
        let engine = exact(&board, 1_000);
        assert_eq!(
            (engine.status(), engine.best_moves()),
            (Status::Solved, Some(18))
        );
    }

    /// The 45 reachable states with A on its goal in the corridor and B
    /// still in the upper room, nine keeper cells for each of B's five
    /// cells, have no solution. B's empty goal below A keeps the corral of
    /// A and the lower row open, and A's one potential push, down, lands on
    /// a dead cell. B on a corner, (2,1) or (2,5), is on a dead cell, so
    /// the admit chain keeps 27 of the states; the detector flags all but
    /// the 9 with B at (2,3), the 18 kept ones with B at (2,2) or (2,4)
    /// among them. B at (2,3) joins A's corral, its sideways pushes are
    /// live, and it blocks A's stand. The counts are `corral_port.py
    /// fixtures`, line f3.
    #[test]
    fn an_empty_goal_behind_a_settled_corral_is_dead() {
        let board = Board::parse(SETTLED).unwrap();
        assert_eq!(board.labels(), b"AB");
        let a = at(&board, 3, 3);
        let solvable = solvable_states(&board);
        let mut checker = Checker::new(&board);
        let mut counts = [(0, 0); 5];
        for (column, (flagged, admitted)) in (1..=5).zip(&mut counts) {
            let b = at(&board, 2, column);
            let mut cells = [NONE; MAX_BOXES];
            cells[..2].copy_from_slice(&[a, b]);
            for row in 1..=2 {
                for player in (1..=5).map(|c| at(&board, row, c)).filter(|&p| p != b) {
                    assert!(!solvable[&(player, cells)], "{player} {b}");
                    *flagged += usize::from(checker.dead(player, &cells[..2]));
                    *admitted += usize::from(checker.admitted(&cells[..2]));
                }
            }
        }
        assert_eq!(counts, [(9, 0), (9, 9), (0, 9), (9, 9), (9, 0)]);
    }

    /// Every state the detector flags on the 150 rooms that
    /// `flagged_pushes_leave_no_solution` checks, drawn the same way from
    /// `Lcg(0x5eed)`, has no solution. `corral_port.py rooms` flags 12_591
    /// of their 108_590 canonical states, 1_276 of them states the admit
    /// chain keeps. The bounds sit a little below, and
    /// `pruning/replicas/corral_variants.py` (not tracked) shows what fails
    /// them: skipping a corral whose boxes are all on goals despite an
    /// empty goal cell (12_573 flags), capping at three pushes (12_067) or
    /// dropping the dead-pair case (11_971). Lifting the cap, or freezing
    /// over every box, flags the same states here and on the catalog;
    /// `a_box_outside_the_corral_is_floor` catches the second.
    #[test]
    fn corral_dead_room_states_have_no_solution() {
        let mut rng = Lcg(0x5eed);
        let (mut flagged, mut admitted) = (0, 0);
        for _ in 0..150 {
            let rows = random_room(&mut rng);
            let board = Board::parse(&rows).unwrap();
            let boxes = board.labels().len();
            let mut checker = Checker::new(&board);
            for (&(player, cells), &has_solution) in &solvable_states(&board) {
                if !checker.dead(player, &cells[..boxes]) {
                    continue;
                }
                flagged += 1;
                admitted += usize::from(checker.admitted(&cells[..boxes]));
                assert!(!has_solution, "{rows:?}: {player} {:?}", &cells[..boxes]);
            }
        }
        assert!(
            flagged >= 12_580 && admitted >= 1_250,
            "{flagged} {admitted}"
        );
    }

    /// Every flagged state of an explored catalog board has no solution.
    /// `corral_port.py oracle` flags 8_675 of the 32_743 states, 620 of
    /// them states the admit chain keeps, all on box-5x5-a and the two
    /// gen-v2 boards. The variants above flag 8_664, 8_638 and 8_624, below
    /// the bound.
    #[test]
    fn corral_dead_states_have_no_solution() {
        let (mut flagged, mut admitted) = (0, 0);
        for (id, board, states, edges) in explored_catalog() {
            let left = remaining(board, states, edges);
            let boxes = board.labels().len();
            let mut checker = Checker::new(board);
            for (s, state) in states.iter().enumerate() {
                if !checker.dead(state.player, &state.boxes[..boxes]) {
                    continue;
                }
                flagged += 1;
                admitted += usize::from(checker.admitted(&state.boxes[..boxes]));
                assert_eq!(left[s], u32::MAX, "{id}: state {s}");
            }
        }
        assert!(flagged >= 8_670 && admitted >= 600, "{flagged} {admitted}");
    }

    /// No catalog start is flagged, huge's included; every one has a
    /// route.
    #[test]
    fn no_catalog_start_is_flagged() {
        for (id, board) in catalog() {
            let start = board.initial();
            let mut checker = Checker::new(&board);
            assert!(
                !checker.dead(start.player, &start.boxes[..board.labels().len()]),
                "{id}"
            );
        }
    }

    /// The large boards the oracles cannot explore: at 20,000 states the
    /// exact policy still finds medium's 34 and stops at the limit on large
    /// and huge, never ending Exhausted. `corral_port.py negative` prunes
    /// 680, 2_376 and 11 states there, and medium's search generates 16_110
    /// records, 18_504 without the check. The lower bound, the frontier
    /// capped by the route as `ExactSearch` reads it, rises from 73 to 84
    /// on large and stays at 215 on huge.
    #[test]
    fn large_boards_are_never_exhausted() {
        let boards = catalog();
        for (id, status, moves, lower, corrals, generated) in [
            ("medium", Status::Solved, Some(34), 34, 680, Some(16_110)),
            ("large", Status::StateLimit, None, 84, 2_376, None),
            ("huge", Status::StateLimit, None, 215, 11, None),
        ] {
            let (_, board) = boards.iter().find(|(name, _)| name == id).unwrap();
            let engine = exact(board, 20_000);
            assert_eq!(
                (engine.status(), engine.best_moves()),
                (status, moves),
                "{id}"
            );
            let route = engine.best_moves().map_or(u64::MAX, u64::from);
            assert_eq!(engine.frontier().min(route), lower, "{id}");
            assert_eq!(engine.stats().pruned_corrals, corrals, "{id}");
            if let Some(generated) = generated {
                assert_eq!(engine.generated(), generated, "{id}");
            }
        }
    }

    /// Two boxes in the open have eight potential pushes, past `CAP`, so
    /// their corral would be skipped; one alone has four.
    #[test]
    fn a_corral_past_the_cap_is_skipped() {
        let board = Board::parse(OPEN).unwrap();
        let start = board.initial();
        let boxes = &start.boxes[..3];
        let mut reach = Reach::new(board.tiles().len());
        reach.fill(&board, &start);
        assert!(pushes(&board, boxes, &reach, 0b001).is_some_and(|(_, count)| count == 4));
        assert!(pushes(&board, boxes, &reach, 0b011).is_none());
        assert!(pushes(&board, boxes, &reach, 0b111).is_none());
    }
}
