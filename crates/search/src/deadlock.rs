//! Post-push freeze deadlocks: the greatest fixpoint of a freeze rule over
//! the pushed box's box-adjacency component. A box stays frozen while each
//! of its axes has a wall or another frozen box on one side, or a cell dead
//! for its label on both sides, and a push that leaves one frozen off its
//! goal leaves no solution. The corral detector asks the same question of
//! a subset of the boxes, so a members mask leaves every other box out, and
//! `frozen_off_goal` asks it of a plain list of boxes, with no occupancy map.
//! The `goals` submodule holds the stage ladder's goal-placement prunes.
mod goals;

use crate::heuristic::Heuristic;
#[cfg_attr(not(test), expect(unused_imports))]
pub(crate) use goals::{GoalReach, SinkLines};
use sokomind_core::{Board, Cell, MAX_BOXES, NONE};
use std::mem::size_of;

/// Occupancy marker for a cell without a box.
const EMPTY: u8 = u8::MAX;
/// A members mask with every box in it, bit `i` standing for box `i`; the
/// admit chain's freeze check sees the whole board.
pub(crate) const ALL_BOXES: u32 = u32::MAX;

/// Whether a box with the four `neighbors` of [`Board::neighbors`] is held
/// on both axes, up-down and left-right: each has a wall or a `blocked`
/// cell on one side or, with `dead_pair`, a cell dead for box `i`'s label
/// on both. Pushing the box along a held axis is illegal while its
/// blockers stay, or strands it on a dead cell. `blocked` and the dead-cell
/// test only ever see board cells, never `NONE`.
fn held(
    neighbors: [Cell; 4],
    i: usize,
    dead_pair: Option<&Heuristic>,
    blocked: impl Fn(Cell) -> bool,
) -> bool {
    let axis = |a: Cell, b: Cell| {
        a == NONE
            || b == NONE
            || blocked(a)
            || blocked(b)
            || dead_pair.is_some_and(|heuristic| heuristic.dead(i, a) && heuristic.dead(i, b))
    };
    axis(neighbors[0], neighbors[1]) && axis(neighbors[2], neighbors[3])
}

/// Whether the greatest freeze fixpoint over `members`, (box, cell) pairs,
/// leaves one off its goal: start with every member frozen and release any
/// box with an axis whose two sides are both free of walls and frozen boxes
/// and, with `dead_pair`, are not both dead cells for its label. A wall on
/// either side blocks its axis, because pushing toward the wall is illegal
/// and pushing away needs the player standing on the wall cell. Two dead
/// sides hold it too: a push either way leaves the box where it never
/// reaches a goal of its label. `occupant` names the box on a cell, and
/// must name each member on its own cell; a box it names outside `members`
/// is never frozen, so it counts as floor.
fn frozen_off_goal_among(
    board: &Board,
    dead_pair: Option<&Heuristic>,
    members: &[(usize, Cell)],
    occupant: impl Fn(Cell) -> Option<usize>,
) -> bool {
    let mut frozen = [false; MAX_BOXES];
    for &(i, _) in members {
        frozen[i] = true;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for &(i, cell) in members {
            if !frozen[i] {
                continue;
            }
            let neighbors = board.neighbors()[cell as usize];
            let frozen_box = |cell: Cell| occupant(cell).is_some_and(|j| frozen[j]);
            if !held(neighbors, i, dead_pair, frozen_box) {
                frozen[i] = false;
                changed = true;
            }
        }
    }
    // No solution moves a box still frozen: the first such push would
    // need both sides of one axis free, but each axis keeps a wall or a
    // frozen box that has not moved either, or has two dead sides, so
    // the push would strand the box on a dead cell. Off its goal, a
    // frozen box leaves no solution.
    members
        .iter()
        .any(|&(i, cell)| frozen[i] && !board.on_goal(i, cell))
}

/// Whether the freeze fixpoint over the boxes in `members` alone, with every
/// other box taken off the board and two dead sides holding an axis, leaves
/// one off its goal. Taking boxes away only removes blockers, so a flag
/// holds wherever the other boxes stand, as for the corral detector's
/// members mask. `members` lists distinct box slots on distinct cells; a
/// slot stands only for its label, its dead cells and goals. With no
/// occupancy map, each lookup scans the list, which a pocket's few boxes
/// keep short.
pub(crate) fn frozen_off_goal(
    board: &Board,
    heuristic: &Heuristic,
    members: &[(usize, Cell)],
) -> bool {
    for (k, &(i, cell)) in members.iter().enumerate() {
        debug_assert!(members[..k].iter().all(|&(j, c)| j != i && c != cell));
    }
    let occupant = |cell: Cell| members.iter().find_map(|&(j, c)| (c == cell).then_some(j));
    frozen_off_goal_among(board, Some(heuristic), members, occupant)
}

/// Sound post-push deadlock detection: one greatest freeze fixpoint over the
/// pushed box's component. Besides frozen groups of any shape, on every state
/// the engine expands it flags each push that completes a 2x2 square of boxes
/// and walls holding a box off its goal. The only such square it skips has a
/// box diagonal to the pushed one, cornered off its goal by the square's two
/// walls: a dead cell no expanded state has. It only ever answers "dead" for
/// states from which no solution exists, so every mode may prune with it
/// freely.
pub(crate) struct Deadlock {
    /// Cell -> box index, or `EMPTY`. Box indices are below `MAX_BOXES`, so
    /// a byte holds one. Refreshed once per expansion.
    occupancy: Vec<u8>,
    /// Cells written by the last refresh, the only ones not `EMPTY`.
    placed: [Cell; MAX_BOXES],
    placed_len: usize,
}

impl Deadlock {
    /// The occupancy map.
    pub(crate) const BYTES_PER_CELL: usize = size_of::<u8>();
    /// An empty occupancy map for `board`, allocated once.
    pub(crate) fn new(board: &Board) -> Self {
        Self {
            occupancy: vec![EMPTY; board.tiles().len()],
            placed: [NONE; MAX_BOXES],
            placed_len: 0,
        }
    }
    /// Rebuild occupancy for a state; call once per expansion, before its
    /// pushes. Resets only the cells the previous refresh wrote.
    pub(crate) fn refresh(&mut self, boxes: &[Cell]) {
        for &cell in &self.placed[..self.placed_len] {
            self.occupancy[cell as usize] = EMPTY;
        }
        for (i, &cell) in boxes.iter().enumerate() {
            self.occupancy[cell as usize] = i as u8;
        }
        self.placed[..boxes.len()].copy_from_slice(boxes);
        self.placed_len = boxes.len();
    }
    /// Index of the box on `cell` as of the last refresh; `None` for no box
    /// or for `NONE`, the missing neighbor past an edge.
    pub(crate) fn at(&self, cell: Cell) -> Option<usize> {
        if cell == NONE {
            return None;
        }
        let id = self.occupancy[cell as usize];
        (id != EMPTY).then_some(id as usize)
    }
    /// Box at `cell` after the hypothetical push of `index` from `from` to
    /// `to`, counting only boxes in `members` besides the pushed one.
    fn box_at(
        &self,
        members: u32,
        from: Cell,
        to: Cell,
        index: usize,
        cell: Cell,
    ) -> Option<usize> {
        if cell == to {
            Some(index)
        } else if cell == from {
            None
        } else {
            self.at(cell).filter(|&j| (members >> j) & 1 == 1)
        }
    }
    /// Whether pushing the box at `from` to `to` creates a deadlock in the
    /// state last given to `refresh`. A newly created deadlock always
    /// involves the moved box, so only the moved box's component is analyzed.
    /// With `dead_pair`, an axis whose two neighbors are both dead cells for
    /// the box's label also holds it. Many pushes, most of them on the
    /// catalog boards, land the box with no box beside it; `lone_answer`
    /// answers those without the fixpoint.
    ///
    /// Only the boxes in `members` take part, the pushed one always; the
    /// others count as floor. Admit passes [`ALL_BOXES`]. Taking boxes away
    /// only removes blockers, so the fixpoint over fewer boxes freezes a
    /// subset of what it freezes over all of them, and a push flagged with
    /// some boxes left out is flagged with them too. The corral detector
    /// relies on that: its answer must hold wherever the boxes outside the
    /// corral go.
    pub(crate) fn is_dead_after_push(
        &self,
        board: &Board,
        dead_pair: Option<&Heuristic>,
        members: u32,
        from: Cell,
        to: Cell,
    ) -> bool {
        let index = self.at(from).expect("refresh saw a box at from");
        self.lone_answer(board, dead_pair, members, index, from, to)
            .unwrap_or_else(|| self.frozen_component(board, dead_pair, members, index, from, to))
    }
    /// The fixpoint's answer for the push of `index` from `from` to `to`
    /// when no box borders `to` after it, else `None`. The component is
    /// then the box alone, which no partner can hold, so the fixpoint
    /// reduces to one `held` test. With `dead_pair` that test is exactly
    /// the dead-cell table's: a held box off its goal can only be pushed
    /// onto a dead cell, so its own cell is dead; and a dead cell is no
    /// goal, and each push from it, with floor on both sides of its axis,
    /// lands on a dead cell (`push_distances`).
    fn lone_answer(
        &self,
        board: &Board,
        dead_pair: Option<&Heuristic>,
        members: u32,
        index: usize,
        from: Cell,
        to: Cell,
    ) -> Option<bool> {
        let neighbors = board.neighbors()[to as usize];
        if neighbors
            .iter()
            .any(|&cell| self.box_at(members, from, to, index, cell).is_some())
        {
            return None;
        }
        Some(match dead_pair {
            Some(heuristic) => heuristic.dead(index, to),
            None => held(neighbors, index, None, |_| false) && !board.on_goal(index, to),
        })
    }
    /// Greatest freeze fixpoint over the moved box's box-adjacency component,
    /// the moved box and every box of `members` joined to it through
    /// side-by-side boxes, run by `frozen_off_goal_among`. A box outside the
    /// component borders none in it, so it could hold none of their axes.
    fn frozen_component(
        &self,
        board: &Board,
        dead_pair: Option<&Heuristic>,
        members: u32,
        index: usize,
        from: Cell,
        to: Cell,
    ) -> bool {
        // (box, cell) pairs after the push, seeded with the moved box.
        let mut component = [(0, NONE); MAX_BOXES];
        let mut in_component = [false; MAX_BOXES];
        let mut size = 0;
        let mut head = 0;
        component[size] = (index, to);
        in_component[index] = true;
        size += 1;
        while head < size {
            let (_, cell) = component[head];
            head += 1;
            for d in 0..4 {
                let adjacent = board.neighbors()[cell as usize][d];
                if let Some(j) = self.box_at(members, from, to, index, adjacent)
                    && !in_component[j]
                {
                    in_component[j] = true;
                    component[size] = (j, adjacent);
                    size += 1;
                }
            }
        }
        let occupant = |cell: Cell| self.box_at(members, from, to, index, cell);
        frozen_off_goal_among(board, dead_pair, &component[..size], occupant)
    }
}

#[cfg(test)]
mod tests {
    use super::{ALL_BOXES, Deadlock, EMPTY, frozen_off_goal, frozen_off_goal_among};
    use crate::{
        heuristic::Heuristic,
        push::canonicalize,
        testkit::{Lcg, catalog, explored_catalog, random_room, remaining, solvable_states},
    };
    use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, State, Step, WALL};

    /// D is frozen on its goal once pushed down; pushing A left then freezes A
    /// against it, while pushing A right puts it on its goal.
    const CHAIN: &str = "O    O\nODO  O\nOd AaO\nOOO RO\nOOOOOO";
    /// Two boxes pushed side by side against the wall hold each other there.
    const WALL_PAIR: &str = "OOOOOOO\nOR    O\nO AB  O\nO     O\nOa b  O\nOOOOOOO";
    /// Pushing the upper X down lands it after the other X in row-major order,
    /// so the canonical child swaps their slots.
    const CROSSING_PAIR: &str = "OOOOOO\nOOOROO\nOO XOO\nOOX SO\nOOSOOO\nOOOOOO";
    /// Pushing B left locks A and B against each other: A has a wall above,
    /// B one below, and each blocks the other's row. C stays pushable and
    /// neither starts with a frozen partner, so the old no-push rule and
    /// least fixpoint both missed it.
    const LOCKED_PAIR: &str =
        "OOOOOOOO\nO      O\nO  O   O\nO CA BRO\nO   O  O\nO abc  O\nOOOOOOOO";
    /// Pushing D left fills a 2x2 square with A, B, C and D, all off their
    /// goals.
    const SQUARE: &str = "OOOOOOOO\nO      O\nO  AB  O\nO  C DRO\nO      O\nOabcd  O\nOOOOOOOO";
    /// Pushing B down fills a 2x2 square with B, A and two inner walls.
    const WALL_SQUARE: &str = "OOOOOOO\nO  R  O\nO  B  O\nO O   O\nO OA  O\nO   abO\nOOOOOOO";
    /// The goals form a 2x2 square that pushing D left from beside it fills.
    const GOAL_SQUARE: &str =
        "OOOOOOOO\nO      O\nO  ab  O\nO  cd RO\nO      O\nO ABCD O\nOOOOOOOO";
    /// A staged state: A sits on its goal under a wall, and pushing B up
    /// under it leaves B's row with two cells dead for B. B is frozen off its
    /// goal only through the dead-pair case. (The board's own start is
    /// already dead; only this push matters.)
    const DEAD_PAIR: &str = "OOOOOOO\nOOOaOOO\nOO   OO\nOOO OOO\nO  b  O\nOR A BO\nOOOOOOO";
    /// From a solvable start, the first push of B tucks it under A. A's row
    /// holds through two cells dead for A, A's column through B, and B's
    /// column through A: A never reaches its goal.
    const DEAD_PAIR_PARTNER: &str =
        "OOOOOOO\nOOOOaOO\nOOO A O\nOOO  OO\nOOO BOO\nOOO bOO\nOOO ROO\nOOOOOOO";

    fn at(board: &Board, row: usize, column: usize) -> Cell {
        (row * board.width() + column) as Cell
    }
    fn state(player: Cell, boxes: &[Cell]) -> State {
        let mut next = State {
            player,
            boxes: [NONE; MAX_BOXES],
        };
        next.boxes[..boxes.len()].copy_from_slice(boxes);
        next
    }

    #[test]
    fn freeze_chain_branches() {
        let board = Board::parse(CHAIN).unwrap();
        let d = at(&board, 2, 1);
        let a = at(&board, 2, 3);
        assert_eq!(board.initial().boxes[..2], [a, at(&board, 1, 1)]);
        let mut deadlock = Deadlock::new(&board);
        // Before D reaches its goal, pushing A left is merely a legal retreat.
        deadlock.refresh(&board.initial().boxes[..board.labels().len()]);
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, a, at(&board, 2, 2)));
        // With D staged on its goal, the same push completes a frozen component.
        deadlock.refresh(&[a, d]);
        assert!(deadlock.is_dead_after_push(&board, None, ALL_BOXES, a, at(&board, 2, 2)));
        // Pushing A down wedges it into a wall corner.
        assert!(deadlock.is_dead_after_push(&board, None, ALL_BOXES, a, at(&board, 3, 3)));
        // Pushing A up, or right onto its goal, stays legal; the latter solves.
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, a, at(&board, 1, 3)));
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, a, at(&board, 2, 4)));
        assert!(board.solved(&state(a, &[at(&board, 2, 4), d])));
    }

    #[test]
    fn refresh_resets_only_previous_boxes() {
        let board = Board::parse(CHAIN).unwrap();
        let d = at(&board, 2, 1);
        let a = at(&board, 2, 3);
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&board.initial().boxes[..board.labels().len()]);
        deadlock.refresh(&[a, d]);
        let mut fresh = Deadlock::new(&board);
        fresh.refresh(&[a, d]);
        assert_eq!(deadlock.occupancy, fresh.occupancy);
        deadlock.refresh(&[]);
        assert!(deadlock.occupancy.iter().all(|&id| id == EMPTY));
    }

    #[test]
    fn wall_pair_freezes() {
        let board = Board::parse(WALL_PAIR).unwrap();
        let a = at(&board, 2, 2);
        let b = at(&board, 2, 3);
        assert_eq!(board.initial().boxes[..2], [a, b]);
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[a, b]);
        // One box against the wall can still be pushed along it under the
        // wall/frozen rule. The dead-pair case flags it, because the whole
        // row is dead for A, a push admit's dead-cell check already prunes.
        let heuristic = Heuristic::new(&board);
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, a, at(&board, 1, 2)));
        assert!(heuristic.dead(0, at(&board, 1, 2)));
        assert!(deadlock.is_dead_after_push(
            &board,
            Some(&heuristic),
            ALL_BOXES,
            a,
            at(&board, 1, 2)
        ));
        // The second box freezes both against the wall.
        deadlock.refresh(&[at(&board, 1, 2), b]);
        assert!(deadlock.is_dead_after_push(&board, None, ALL_BOXES, b, at(&board, 1, 3)));
    }

    /// The moved box is found by its parent cell, so the answer cannot depend
    /// on the order canonicalize gives the child.
    #[test]
    fn push_past_a_same_label_box() {
        let board = Board::parse(CROSSING_PAIR).unwrap();
        let upper = at(&board, 2, 3);
        let lower = at(&board, 3, 2);
        let below = at(&board, 3, 3);
        assert_eq!(board.initial().boxes[..2], [upper, lower]);
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[upper, lower]);
        // The first push of the 4-move solution.
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, upper, below));
        let mut child = state(upper, &[below, lower]);
        canonicalize(&board, &mut child);
        assert_eq!(child.boxes[..2], [lower, below]);
    }

    #[test]
    fn mutually_locked_pair_freezes() {
        let board = Board::parse(LOCKED_PAIR).unwrap();
        let (a, b, c) = (at(&board, 3, 3), at(&board, 3, 5), at(&board, 3, 2));
        assert_eq!(board.initial().boxes[..3], [a, b, c]);
        let left = at(&board, 3, 4);
        // B's destination is not a dead cell, so only the freeze prunes it.
        assert!(!Heuristic::new(&board).dead(1, left));
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[a, b, c]);
        assert!(deadlock.is_dead_after_push(&board, None, ALL_BOXES, b, left));
        // With A one row lower instead, the same push leaves B's row free.
        deadlock.refresh(&[at(&board, 4, 3), b, c]);
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, b, left));
    }

    #[test]
    fn square_of_four_boxes_freezes() {
        let board = Board::parse(SQUARE).unwrap();
        let d = at(&board, 3, 5);
        let boxes = [at(&board, 2, 3), at(&board, 2, 4), at(&board, 3, 3), d];
        assert_eq!(board.initial().boxes[..4], boxes);
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&boxes);
        // Each box has a frozen neighbor across both of its axes.
        assert!(deadlock.is_dead_after_push(&board, None, ALL_BOXES, d, at(&board, 3, 4)));
        // Pushed up instead, D joins the others without closing a square.
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, d, at(&board, 2, 5)));
    }

    #[test]
    fn square_of_two_boxes_and_two_walls_freezes() {
        let board = Board::parse(WALL_SQUARE).unwrap();
        let (a, b) = (at(&board, 4, 3), at(&board, 2, 3));
        assert_eq!(board.initial().boxes[..2], [a, b]);
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[a, b]);
        // A wall blocks each box's row, and each blocks the other's column.
        assert!(deadlock.is_dead_after_push(&board, None, ALL_BOXES, b, at(&board, 3, 3)));
        // With A one column right, B lands against the wall alone.
        deadlock.refresh(&[at(&board, 4, 4), b]);
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, b, at(&board, 3, 3)));
    }

    /// A full square is no deadlock while every box in it is on its goal,
    /// and is one as soon as two of them trade places.
    #[test]
    fn square_on_its_goals_is_not_dead() {
        let board = Board::parse(GOAL_SQUARE).unwrap();
        let (a, b, c) = (at(&board, 2, 3), at(&board, 2, 4), at(&board, 3, 3));
        let (from, to) = (at(&board, 3, 5), at(&board, 3, 4));
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[a, b, c, from]);
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, from, to));
        assert!(board.solved(&state(from, &[a, b, c, to])));
        deadlock.refresh(&[b, a, c, from]);
        assert!(deadlock.is_dead_after_push(&board, None, ALL_BOXES, from, to));
    }

    #[test]
    fn dead_pair_axis_freezes() {
        let board = Board::parse(DEAD_PAIR).unwrap();
        let heuristic = Heuristic::new(&board);
        let (a, from, to) = (at(&board, 1, 3), at(&board, 3, 3), at(&board, 2, 3));
        // Admit's dead-cell check passes the push; both row neighbors are dead.
        assert!(!heuristic.dead(1, to));
        assert!(heuristic.dead(1, at(&board, 2, 2)) && heuristic.dead(1, at(&board, 2, 4)));
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[a, from]);
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, from, to));
        assert!(deadlock.is_dead_after_push(&board, Some(&heuristic), ALL_BOXES, from, to));
    }

    /// The dead pair holds the moved box's partner, not the moved box, so a
    /// rule that applies the case only to the moved box misses it.
    #[test]
    fn dead_pair_holds_the_partner() {
        let board = Board::parse(DEAD_PAIR_PARTNER).unwrap();
        let heuristic = Heuristic::new(&board);
        let (a, b, up) = (at(&board, 2, 4), at(&board, 4, 4), at(&board, 3, 4));
        assert_eq!(board.initial().boxes[..2], [a, b]);
        assert!(!heuristic.dead(1, up));
        assert!(heuristic.dead(0, at(&board, 2, 3)) && heuristic.dead(0, at(&board, 2, 5)));
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[a, b]);
        assert!(!deadlock.is_dead_after_push(&board, None, ALL_BOXES, b, up));
        assert!(deadlock.is_dead_after_push(&board, Some(&heuristic), ALL_BOXES, b, up));
    }

    /// Every push the rule flags, from any reachable state, leads to a state
    /// with no solution. Flags from solvable parents are the ones that could
    /// fail, so the test also requires enough of them.
    #[test]
    fn flagged_pushes_leave_no_solution() {
        let mut rng = Lcg(0x5eed);
        let (mut flagged, mut from_solvable, mut live_destination) = (0, 0, 0);
        for _ in 0..150 {
            let rows = random_room(&mut rng);
            let board = Board::parse(&rows).unwrap();
            let heuristic = Heuristic::new(&board);
            let solvable = solvable_states(&board);
            let boxes = board.labels().len();
            let mut deadlock = Deadlock::new(&board);
            for (&(player, cells), &parent_solvable) in &solvable {
                deadlock.refresh(&cells[..boxes]);
                for d in 0..4 {
                    let mut child = State {
                        player,
                        boxes: cells,
                    };
                    let Some(Step::Push(i)) = board.step(&mut child, d) else {
                        continue;
                    };
                    // Taken before canonicalize can reorder the slots.
                    let (from, to) = (cells[i], child.boxes[i]);
                    if !deadlock.is_dead_after_push(&board, Some(&heuristic), ALL_BOXES, from, to) {
                        continue;
                    }
                    flagged += 1;
                    if parent_solvable {
                        from_solvable += 1;
                    }
                    // The flags admit acts on: its dead-cell check runs first.
                    if !heuristic.dead(i, to) {
                        live_destination += 1;
                    }
                    canonicalize(&board, &mut child);
                    assert!(!solvable[&(child.player, child.boxes)], "{rows:?}");
                }
            }
        }
        // A Python replica of this test (pruning/replicas/freeze_replica.py,
        // not tracked) gives 15_686 flags for seed 0x5eed, 693 from solvable
        // parents and 2_577 with a live destination (10_658, 503 and 1_993
        // without the dead-pair case). The bounds sit a little below, so
        // dropping either half of the rule fails them. Applying the dead-pair
        // case to the moved box alone (15_102, 693 and 1_993) fails only the
        // live-destination bound, and dead_pair_holds_the_partner. No room
        // exercises an occupied dead-pair neighbor: a rule that applies the
        // case only between two empty cells gives the same counts.
        assert!(
            flagged >= 15_000 && from_solvable >= 650 && live_destination >= 2_450,
            "{flagged} {from_solvable} {live_destination}"
        );
    }

    /// Every push the rule flags on an explored catalog board, from any
    /// reachable state, leads to a state with no solution. The 4x4 rooms
    /// above miss some geometry: an unsound variant that also holds a box
    /// when its row is held and the cells above it and to its left are both
    /// dead for it passes there, bounds included, and fails here.
    #[test]
    fn flagged_catalog_pushes_leave_no_solution() {
        let (mut flagged, mut from_solvable) = (0, 0);
        for (id, board, states, edges) in explored_catalog() {
            let heuristic = Heuristic::new(board);
            let left = remaining(board, states, edges);
            let boxes = board.labels().len();
            let mut deadlock = Deadlock::new(board);
            for (s, out) in edges.iter().enumerate() {
                deadlock.refresh(&states[s].boxes[..boxes]);
                for &(to, push) in out {
                    // Explored states keep the board's box order, so slot i
                    // is the same box in both states.
                    let Some((i, _)) = push else {
                        continue;
                    };
                    let (from, cell) = (states[s].boxes[i], states[to].boxes[i]);
                    if !deadlock.is_dead_after_push(board, Some(&heuristic), ALL_BOXES, from, cell)
                    {
                        continue;
                    }
                    flagged += 1;
                    if left[s] != u32::MAX {
                        from_solvable += 1;
                    }
                    assert_eq!(left[to], u32::MAX, "{id}: state {s} to {to}");
                }
            }
        }
        // pruning/replicas/catalog_flags.py (not tracked) gives 2_229 flags,
        // 209 from solvable parents (1_597 and 114 without the dead-pair
        // case), and finds 2 of the unsound variant's flags on tiny.
        assert!(
            flagged >= 2_100 && from_solvable >= 190,
            "{flagged} {from_solvable}"
        );
    }

    /// The shortcut for a box that lands with no box beside it answers as
    /// the full fixpoint does, with and without the dead-pair case, on
    /// every push of the explored catalog boards.
    #[test]
    fn lone_box_shortcut_matches_the_fixpoint() {
        let (mut lone, mut lone_flags, mut grouped) = (0, [0; 2], 0);
        for (id, board, states, edges) in explored_catalog() {
            let heuristic = Heuristic::new(board);
            let boxes = board.labels().len();
            let mut deadlock = Deadlock::new(board);
            for (s, out) in edges.iter().enumerate() {
                deadlock.refresh(&states[s].boxes[..boxes]);
                for &(to, push) in out {
                    // Explored states keep the board's box order, so slot i
                    // is the same box in both states.
                    let Some((i, _)) = push else {
                        continue;
                    };
                    let (from, cell) = (states[s].boxes[i], states[to].boxes[i]);
                    let mut alone = false;
                    for (k, dead_pair) in [None, Some(&heuristic)].into_iter().enumerate() {
                        let full =
                            deadlock.frozen_component(board, dead_pair, ALL_BOXES, i, from, cell);
                        let flagged =
                            deadlock.is_dead_after_push(board, dead_pair, ALL_BOXES, from, cell);
                        assert_eq!(flagged, full, "{id}: state {s} to {to}");
                        let shortcut =
                            deadlock.lone_answer(board, dead_pair, ALL_BOXES, i, from, cell);
                        if let Some(answer) = shortcut {
                            assert_eq!(answer, full, "{id}: state {s} to {to}");
                            alone = true;
                            lone_flags[k] += usize::from(answer);
                        }
                    }
                    if alone {
                        lone += 1;
                    } else {
                        grouped += 1;
                    }
                }
            }
        }
        // A replica of this test counts 4_058 pushes that land the box
        // alone, 960 and 1_545 of them flagged without and with the
        // dead-pair case, and 854 beside another box; the bounds sit a
        // little below, so the shortcut must stay the common case.
        assert!(
            lone >= 3_900 && lone_flags[0] >= 900 && lone_flags[1] >= 1_450 && grouped >= 800,
            "{lone} {lone_flags:?} {grouped}"
        );
    }

    /// The probe's geometry for its `frozen_off_goal` (slurm/probes/p4b.rs,
    /// not tracked): floor, neighbors, each slot's group, each goal cell's
    /// group, and per group the floor cells from which no push sequence
    /// takes a box to one of its goals. The probe pulls those from the
    /// goals; `Heuristic::dead` marks the same cells, as `dead_matches_pull`
    /// checks on every catalog board.
    #[derive(Clone)]
    struct Geo {
        floor: Vec<bool>,
        nb: Vec<[Cell; 4]>,
        group: Vec<usize>,
        goal_group: Vec<usize>,
        dead: Vec<Vec<bool>>,
    }

    impl Geo {
        fn new(board: &Board, heuristic: &Heuristic) -> Self {
            let (group, goal_group) = crate::testkit::probe_groups(board);
            let floor: Vec<bool> = board.tiles().iter().map(|&tile| tile != WALL).collect();
            let mut dead: Vec<Vec<bool>> = Vec::new();
            for i in (0..group.len()).filter(|&i| heuristic.group(i).start == i) {
                let cells = (0..floor.len()).map(|c| floor[c] && heuristic.dead(i, c as Cell));
                dead.push(cells.collect());
            }
            Self {
                floor,
                nb: board.neighbors().to_vec(),
                group,
                goal_group,
                dead,
            }
        }

        /// The probe's `frozen_off_goal`: box `j` of group `group[j]` on
        /// `cells[j]`, no other box on the board.
        fn frozen_off_goal(&self, cells: &[Cell], group: &[usize]) -> bool {
            let wall = |c: Cell| c == NONE || !self.floor[c as usize];
            let mut frozen = vec![true; cells.len()];
            loop {
                let mut changed = false;
                for (j, (&x, &g)) in cells.iter().zip(group).enumerate() {
                    if !frozen[j] {
                        continue;
                    }
                    let (nb, dead) = (self.nb[x as usize], &self.dead[g]);
                    let held = (0..4).all(|d| {
                        let (a, b) = (nb[d], nb[OPPOSITE[d]]);
                        let blocker = |c: Cell| {
                            wall(c) || cells.iter().zip(&frozen).any(|(&y, &f)| f && y == c)
                        };
                        blocker(a) || blocker(b) || (dead[a as usize] && dead[b as usize])
                    });
                    if !held {
                        frozen[j] = false;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            cells
                .iter()
                .zip(group)
                .zip(&frozen)
                .any(|((&c, &g), &f)| f && self.goal_group[c as usize] != g)
        }
    }

    /// `frozen_off_goal` answers as the probe's does on random sets of up to
    /// 8 boxes on every catalog board, and the shared fixpoint without the
    /// dead-pair case as the probe's with no dead cells. Most boxes land
    /// beside an earlier one, so they group and hold one another.
    #[test]
    fn frozen_off_goal_matches_probe() {
        let mut rng = Lcg(0xf20e);
        // Flagged, passed, flagged with no box flagged alone, and flagged
        // only through the dead-pair case.
        let mut seen = [0; 4];
        for (id, board) in catalog() {
            let heuristic = Heuristic::new(&board);
            let geo = Geo::new(&board, &heuristic);
            let blind_geo = Geo {
                dead: vec![vec![false; geo.floor.len()]; geo.dead.len()],
                ..geo.clone()
            };
            let floor: Vec<Cell> = (0..board.tiles().len() as Cell)
                .filter(|&cell| geo.floor[cell as usize])
                .collect();
            let boxes = board.labels().len();
            for _ in 0..200 {
                let mut slots: Vec<usize> = (0..boxes).collect();
                let mut members: Vec<(usize, Cell)> = Vec::new();
                let count = 1 + rng.below(boxes.min(8));
                for _ in 0..count {
                    let i = slots.swap_remove(rng.below(slots.len()));
                    let mut cell = NONE;
                    if !members.is_empty() && rng.below(4) != 0 {
                        let (_, beside) = members[rng.below(members.len())];
                        cell = board.neighbors()[beside as usize][rng.below(4)];
                    }
                    while cell == NONE || members.iter().any(|&(_, c)| c == cell) {
                        cell = floor[rng.below(floor.len())];
                    }
                    members.push((i, cell));
                }
                let (cells, group): (Vec<Cell>, Vec<usize>) =
                    members.iter().map(|&(i, c)| (c, geo.group[i])).unzip();
                let flagged = frozen_off_goal(&board, &heuristic, &members);
                let probe = geo.frozen_off_goal(&cells, &group);
                assert_eq!(flagged, probe, "{id}: {members:?}");
                let blind = frozen_off_goal_among(&board, None, &members, |cell| {
                    members.iter().find_map(|&(j, c)| (c == cell).then_some(j))
                });
                let blind_probe = blind_geo.frozen_off_goal(&cells, &group);
                assert_eq!(blind, blind_probe, "{id}: {members:?}");
                let alone = members
                    .iter()
                    .any(|&member| frozen_off_goal(&board, &heuristic, &[member]));
                seen[0] += usize::from(flagged);
                seen[1] += usize::from(!flagged);
                seen[2] += usize::from(flagged && !alone);
                seen[3] += usize::from(flagged && !blind);
            }
        }
        assert!(seen.iter().all(|&count| count > 0), "{seen:?}");
    }
}
