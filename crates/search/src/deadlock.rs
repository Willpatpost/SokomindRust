use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, WALL};

/// Sound post-push deadlock detection, ported from the reference engine's
/// `creates2x2Deadlock` and `createsFrozenComponentDeadlock`. It only ever
/// answers "dead" for states from which no solution exists, so every mode
/// may prune with it freely.
pub(crate) struct Deadlock {
    /// Cell -> box index, or u32::MAX when empty. Refreshed once per expansion.
    occupancy: Vec<u32>,
}

impl Deadlock {
    pub(crate) fn new(board: &Board) -> Self {
        Self {
            occupancy: vec![u32::MAX; board.tiles.len()],
        }
    }
    /// Rebuild occupancy for a state; call once per expansion, before its pushes.
    pub(crate) fn refresh(&mut self, boxes: &[Cell]) {
        self.occupancy.fill(u32::MAX);
        for (i, &cell) in boxes.iter().enumerate() {
            self.occupancy[cell as usize] = i as u32;
        }
    }
    fn at(&self, cell: Cell) -> Option<usize> {
        if cell == NONE {
            return None;
        }
        let id = self.occupancy[cell as usize];
        (id != u32::MAX).then_some(id as usize)
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
    /// involves the moved box, so only the four squares at `to` and the moved
    /// box's component are analyzed.
    pub(crate) fn is_dead_after_push(&self, board: &Board, from: Cell, to: Cell) -> bool {
        let index = self.at(from).expect("refresh saw a box at from");
        self.two_by_two(board, index, from, to) || self.frozen_component(board, index, from, to)
    }
    /// A completely blocked 2x2 square releases no participating box; it is
    /// dead only when at least one contained box is off its matching goal.
    fn square_dead(
        &self,
        board: &Board,
        box_at: impl Fn(Cell) -> Option<usize>,
        origin_row: isize,
        origin_col: isize,
    ) -> bool {
        let mut blocked = true;
        let mut unsolved = false;
        for (dy, dx) in [(0isize, 0isize), (1, 0), (0, 1), (1, 1)] {
            let cell =
                ((origin_row + dy) as usize * board.width + (origin_col + dx) as usize) as Cell;
            match box_at(cell) {
                Some(i) => {
                    if !board.on_goal(i, cell) {
                        unsolved = true;
                    }
                }
                None => {
                    if board.tiles[cell as usize] != WALL {
                        blocked = false;
                        break;
                    }
                }
            }
        }
        blocked && unsolved
    }
    fn two_by_two(&self, board: &Board, index: usize, from: Cell, to: Cell) -> bool {
        let (mx, my) = ((to as usize) % board.width, (to as usize) / board.width);
        for row in [my as isize - 1, my as isize] {
            for column in [mx as isize - 1, mx as isize] {
                if row < 0
                    || column < 0
                    || row + 1 >= board.height as isize
                    || column + 1 >= board.width as isize
                {
                    continue;
                }
                if self.square_dead(
                    board,
                    |cell| self.box_at(from, to, index, cell),
                    row,
                    column,
                ) {
                    return true;
                }
            }
        }
        false
    }
    /// Freeze fixpoint over the moved box's box-adjacency component. A box is
    /// frozen when each axis has a wall or an already-frozen box on it; a wall
    /// on either side blocks its axis, because pushing toward the wall is
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
                let adjacent = board.neighbors[cell as usize][d];
                if let Some(j) = self.box_at(from, to, index, adjacent) {
                    if !in_component[j] {
                        in_component[j] = true;
                        component[size] = (j, adjacent);
                        size += 1;
                    }
                }
            }
        }
        let component = &component[..size];
        let mut frozen = [false; MAX_BOXES];
        let mut changed = true;
        while changed {
            changed = false;
            for &(i, cell) in component {
                if frozen[i] {
                    continue;
                }
                let neighbors = board.neighbors[cell as usize];
                let blocker = |cell: Cell| {
                    cell == NONE
                        || self
                            .box_at(from, to, index, cell)
                            .is_some_and(|j| frozen[j])
                };
                if (blocker(neighbors[0]) || blocker(neighbors[1]))
                    && (blocker(neighbors[2]) || blocker(neighbors[3]))
                {
                    frozen[i] = true;
                    changed = true;
                }
            }
        }
        // A frozen box off its matching goal can never move again.
        if component
            .iter()
            .any(|&(i, cell)| frozen[i] && !board.on_goal(i, cell))
        {
            return true;
        }
        // If no component box can be pushed at all, none can ever move: every
        // push cell of a component box is adjacent to it, so only component
        // boxes — all currently immovable — could unblock it.
        for &(_, cell) in component {
            for d in 0..4 {
                let destination = board.neighbors[cell as usize][d];
                let support = board.neighbors[cell as usize][OPPOSITE[d]];
                if destination != NONE
                    && support != NONE
                    && self.box_at(from, to, index, destination).is_none()
                    && self.box_at(from, to, index, support).is_none()
                {
                    return false;
                }
            }
        }
        component.iter().any(|&(i, cell)| !board.on_goal(i, cell))
    }
}

#[cfg(test)]
mod tests {
    use super::Deadlock;
    use sokomind_core::{Board, Cell, MAX_BOXES, NONE, State};

    /// D is frozen on its goal once pushed down; pushing A left then freezes A
    /// against it, while pushing A right puts it on its goal.
    const CHAIN: &str = "O    O\nODO  O\nOd AaO\nOOO RO\nOOOOOO";
    /// Two boxes pushed against the wall form a wall/box 2x2 square.
    const WALL_PAIR: &str = "OOOOOOO\nOR    O\nO AB  O\nO     O\nOa b  O\nOOOOOOO";
    /// Pushing the upper X down lands it after the other X in row-major order,
    /// so the canonical child swaps their slots.
    const CROSSING_PAIR: &str = "OOOOOO\nOOOROO\nOO XOO\nOOX SO\nOOSOOO\nOOOOOO";

    fn at(board: &Board, row: usize, column: usize) -> Cell {
        (row * board.width + column) as Cell
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
    fn freeze_component_and_2x2_branches() {
        let board = Board::parse(CHAIN).unwrap();
        let d = at(&board, 2, 1);
        let a = at(&board, 2, 3);
        assert_eq!(board.initial.boxes[..2], [a, at(&board, 1, 1)]);
        let mut deadlock = Deadlock::new(&board);
        // Before D reaches its goal, pushing A left is merely a legal retreat.
        deadlock.refresh(&board.initial.boxes[..board.labels.len()]);
        assert!(!deadlock.is_dead_after_push(&board, a, at(&board, 2, 2)));
        // With D staged on its goal, the same push completes a frozen component.
        deadlock.refresh(&[a, d]);
        assert!(deadlock.is_dead_after_push(&board, a, at(&board, 2, 2)));
        // Pushing A down forms a wall/box 2x2 square.
        assert!(deadlock.is_dead_after_push(&board, a, at(&board, 3, 3)));
        // Pushing A up, or right onto its goal, stays legal; the latter solves.
        assert!(!deadlock.is_dead_after_push(&board, a, at(&board, 1, 3)));
        assert!(!deadlock.is_dead_after_push(&board, a, at(&board, 2, 4)));
        assert!(board.solved(&state(a, &[at(&board, 2, 4), d])));
    }

    #[test]
    fn wall_pair_2x2_deadlock() {
        let board = Board::parse(WALL_PAIR).unwrap();
        let a = at(&board, 2, 2);
        let b = at(&board, 2, 3);
        assert_eq!(board.initial.boxes[..2], [a, b]);
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[a, b]);
        // One box against the wall can still be pushed away from it.
        assert!(!deadlock.is_dead_after_push(&board, a, at(&board, 1, 2)));
        // The second box completes a fully blocked wall/box square.
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
        assert_eq!(board.initial.boxes[..2], [upper, lower]);
        let mut deadlock = Deadlock::new(&board);
        deadlock.refresh(&[upper, lower]);
        // The first push of the 4-move solution.
        assert!(!deadlock.is_dead_after_push(&board, upper, below));
        let mut child = state(upper, &[below, lower]);
        board.canonicalize(&mut child);
        assert_eq!(child.boxes[..2], [lower, below]);
    }
}
