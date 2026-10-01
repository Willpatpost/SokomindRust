//! Post-push freeze deadlocks: the greatest fixpoint of a freeze rule over
//! the pushed box's box-adjacency component. A box stays frozen while each
//! of its axes has a wall or another frozen box on one side, and a push that
//! leaves one frozen off its goal leaves no solution.
use sokomind_core::{Board, Cell, MAX_BOXES, NONE};
use std::mem::size_of;

/// Occupancy marker for a cell without a box.
const EMPTY: u8 = u8::MAX;

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
    fn at(&self, cell: Cell) -> Option<usize> {
        if cell == NONE {
            return None;
        }
        let id = self.occupancy[cell as usize];
        (id != EMPTY).then_some(id as usize)
    }
    /// Box at `cell` after the hypothetical push of `index` from `from` to `to`.
    fn box_at(&self, from: Cell, to: Cell, index: usize, cell: Cell) -> Option<usize> {
        if cell == to {
            Some(index)
        } else if cell == from {
            None
        } else {
            self.at(cell)
        }
    }
    /// Whether pushing the box at `from` to `to` creates a deadlock in the
    /// state last given to `refresh`. A newly created deadlock always
    /// involves the moved box, so only the moved box's component is analyzed.
    pub(crate) fn is_dead_after_push(&self, board: &Board, from: Cell, to: Cell) -> bool {
        let index = self.at(from).expect("refresh saw a box at from");
        self.frozen_component(board, index, from, to)
    }
    /// Greatest freeze fixpoint over the moved box's box-adjacency component:
    /// start with every component box frozen and release any box with an axis
    /// whose two sides are both free of walls and frozen boxes. A wall on
    /// either side blocks its axis, because pushing toward the wall is
    /// illegal and pushing away needs the player standing on the wall cell.
    fn frozen_component(&self, board: &Board, index: usize, from: Cell, to: Cell) -> bool {
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
                if let Some(j) = self.box_at(from, to, index, adjacent)
                    && !in_component[j]
                {
                    in_component[j] = true;
                    component[size] = (j, adjacent);
                    size += 1;
                }
            }
        }
        let component = &component[..size];
        let mut frozen = [false; MAX_BOXES];
        for &(i, _) in component {
            frozen[i] = true;
        }
        let mut changed = true;
        while changed {
            changed = false;
            for &(i, cell) in component {
                if !frozen[i] {
                    continue;
                }
                let neighbors = board.neighbors()[cell as usize];
                let blocker = |cell: Cell| {
                    cell == NONE
                        || self
                            .box_at(from, to, index, cell)
                            .is_some_and(|j| frozen[j])
                };
                let held = (blocker(neighbors[0]) || blocker(neighbors[1]))
                    && (blocker(neighbors[2]) || blocker(neighbors[3]));
                if !held {
                    frozen[i] = false;
                    changed = true;
                }
            }
        }
        // A box still frozen never moves: its first push would need both
        // sides of one axis free, but each axis keeps a wall or a frozen box
        // that has not moved either. Off its goal, it leaves no solution.
        component
            .iter()
            .any(|&(i, cell)| frozen[i] && !board.on_goal(i, cell))
    }
}

#[cfg(test)]
mod tests {
    use super::{Deadlock, EMPTY};
    use crate::{
        engine::canonicalize,
        heuristic::Heuristic,
        testkit::{Lcg, random_room},
    };
    use sokomind_core::{Board, Cell, MAX_BOXES, NONE, State, Step};
    use std::collections::HashMap;

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
        assert!(!deadlock.is_dead_after_push(&board, a, at(&board, 2, 2)));
        // With D staged on its goal, the same push completes a frozen component.
        deadlock.refresh(&[a, d]);
        assert!(deadlock.is_dead_after_push(&board, a, at(&board, 2, 2)));
        // Pushing A down wedges it into a wall corner.
        assert!(deadlock.is_dead_after_push(&board, a, at(&board, 3, 3)));
        // Pushing A up, or right onto its goal, stays legal; the latter solves.
        assert!(!deadlock.is_dead_after_push(&board, a, at(&board, 1, 3)));
        assert!(!deadlock.is_dead_after_push(&board, a, at(&board, 2, 4)));
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
        // One box against the wall can still be pushed along it.
        assert!(!deadlock.is_dead_after_push(&board, a, at(&board, 1, 2)));
        // The second box freezes both against the wall.
        deadlock.refresh(&[at(&board, 1, 2), b]);
        assert!(deadlock.is_dead_after_push(&board, b, at(&board, 1, 3)));
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
        assert!(!deadlock.is_dead_after_push(&board, upper, below));
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
        assert!(deadlock.is_dead_after_push(&board, b, left));
        // With A one row lower instead, the same push leaves B's row free.
        deadlock.refresh(&[at(&board, 4, 3), b, c]);
        assert!(!deadlock.is_dead_after_push(&board, b, left));
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
        assert!(deadlock.is_dead_after_push(&board, d, at(&board, 3, 4)));
        // Pushed up instead, D joins the others without closing a square.
        assert!(!deadlock.is_dead_after_push(&board, d, at(&board, 2, 5)));
    }

    #[test]
    fn square_of_two_boxes_and_two_walls_freezes() {
        let board = Board::parse(WALL_SQUARE).unwrap();
        let (a, b) = (at(&board, 4, 3), at(&board, 2, 3));
        assert_eq!(board.initial().boxes[..2], [a, b]);
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[a, b]);
        // A wall blocks each box's row, and each blocks the other's column.
        assert!(deadlock.is_dead_after_push(&board, b, at(&board, 3, 3)));
        // With A one column right, B lands against the wall alone.
        deadlock.refresh(&[at(&board, 4, 4), b]);
        assert!(!deadlock.is_dead_after_push(&board, b, at(&board, 3, 3)));
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
        assert!(!deadlock.is_dead_after_push(&board, from, to));
        assert!(board.solved(&state(from, &[a, b, c, to])));
        deadlock.refresh(&[b, a, c, from]);
        assert!(deadlock.is_dead_after_push(&board, from, to));
    }

    /// Every state reachable by primitive moves, keyed canonically, mapped to
    /// whether a solved state is reachable from it. A 4x4 floor keeps this
    /// under 44k states.
    fn solvable_states(board: &Board) -> HashMap<(Cell, [Cell; MAX_BOXES]), bool> {
        let key = |mut state: State| {
            canonicalize(board, &mut state);
            (state.player, state.boxes)
        };
        let mut ids = HashMap::from([(key(board.initial()), 0)]);
        let mut states = vec![board.initial()];
        let mut parents: Vec<Vec<usize>> = vec![Vec::new()];
        let mut head = 0;
        while head < states.len() {
            for d in 0..4 {
                let mut next = states[head];
                if board.step(&mut next, d).is_none() {
                    continue;
                }
                let id = *ids.entry(key(next)).or_insert_with(|| {
                    states.push(next);
                    parents.push(Vec::new());
                    states.len() - 1
                });
                parents[id].push(head);
            }
            head += 1;
        }
        let mut solvable: Vec<bool> = states.iter().map(|s| board.solved(s)).collect();
        let mut stack: Vec<usize> = (0..states.len()).filter(|&s| solvable[s]).collect();
        while let Some(s) = stack.pop() {
            for &parent in &parents[s] {
                if !solvable[parent] {
                    solvable[parent] = true;
                    stack.push(parent);
                }
            }
        }
        ids.into_iter().map(|(k, id)| (k, solvable[id])).collect()
    }

    /// Every push the rule flags, from any reachable state, leads to a state
    /// with no solution. Flags from solvable parents are the ones that could
    /// fail, so the test also requires enough of them.
    #[test]
    fn flagged_pushes_leave_no_solution() {
        let mut rng = Lcg(0x5eed);
        let (mut flagged, mut from_solvable) = (0, 0);
        for _ in 0..150 {
            let rows = random_room(&mut rng);
            let board = Board::parse(&rows).unwrap();
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
                    if !deadlock.is_dead_after_push(&board, cells[i], child.boxes[i]) {
                        continue;
                    }
                    flagged += 1;
                    if parent_solvable {
                        from_solvable += 1;
                    }
                    canonicalize(&board, &mut child);
                    assert!(!solvable[&(child.player, child.boxes)], "{rows:?}");
                }
            }
        }
        // A Python replica of this test, not kept in the repo, gave 10_658
        // flags for seed 0x5eed, 503 from solvable parents; the bounds sit a
        // little below that, leaving room for rule changes that remove a few
        // flags.
        assert!(
            flagged >= 10_000 && from_solvable >= 450,
            "{flagged} {from_solvable}"
        );
    }
}
