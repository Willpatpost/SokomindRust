use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE};

/// Sound post-push deadlock detection, ported from the reference engine's
/// `creates2x2Deadlock` and `createsFrozenComponentDeadlock`. It only ever
/// answers "dead" for states from which no solution exists, so both engines
/// may prune with it freely.
pub struct Deadlock {
    /// Cell -> box index, or u32::MAX when empty. Refreshed once per expansion.
    occupancy: Vec<u32>,
}

impl Deadlock {
    pub fn new(board: &Board) -> Self {
        Self {
            occupancy: vec![u32::MAX; board.tiles.len()],
        }
    }
    /// Rebuild occupancy for a state; call once per expansion, before its pushes.
    pub fn refresh(&mut self, boxes: &[Cell]) {
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
    /// Whether pushing box `index` from `from` to `to` creates a deadlock.
    /// A newly created deadlock always involves the moved box, so only the
    /// four squares at `to` and the moved box's component are analyzed.
    pub fn is_dead_after_push(
        &self,
        board: &Board,
        boxes: &[Cell],
        index: usize,
        from: Cell,
        to: Cell,
    ) -> bool {
        self.two_by_two(board, index, from, to)
            || self.frozen_component(board, boxes, index, from, to)
    }
    /// Whether the refreshed state already contains a fully blocked 2x2
    /// square with an off-goal box, or a frozen box off its matching goal.
    pub fn is_dead_state(&self, board: &Board, boxes: &[Cell]) -> bool {
        (0..board.height - 1).any(|row| {
            (0..board.width - 1).any(|column| {
                self.square_dead(board, |cell| self.at(cell), row as isize, column as isize)
            })
        }) || self.any_frozen_off_goal(board, boxes)
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
                    if board.tiles[cell as usize] != board.labels[i] {
                        unsolved = true;
                    }
                }
                None => {
                    if board.tiles[cell as usize] != 255 {
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
                if self.square_dead(board, |cell| self.box_at(from, to, index, cell), row, column)
                {
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
    fn frozen_component(
        &self,
        board: &Board,
        boxes: &[Cell],
        index: usize,
        from: Cell,
        to: Cell,
    ) -> bool {
        let cell_of = |i: usize| if i == index { to } else { boxes[i] };
        let mut component = [0; MAX_BOXES];
        let mut in_component = [false; MAX_BOXES];
        let mut size = 0;
        let mut head = 0;
        component[size] = index;
        in_component[index] = true;
        size += 1;
        while head < size {
            let i = component[head];
            head += 1;
            for d in 0..4 {
                let adjacent = board.neighbors[cell_of(i) as usize][d];
                if let Some(j) = self.box_at(from, to, index, adjacent) {
                    if !in_component[j] {
                        in_component[j] = true;
                        component[size] = j;
                        size += 1;
                    }
                }
            }
        }
        let mut frozen = [false; MAX_BOXES];
        let mut changed = true;
        while changed {
            changed = false;
            for slot in 0..size {
                let i = component[slot];
                if frozen[i] {
                    continue;
                }
                let neighbors = board.neighbors[cell_of(i) as usize];
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
        for slot in 0..size {
            let i = component[slot];
            if frozen[i] && board.tiles[cell_of(i) as usize] != board.labels[i] {
                return true;
            }
        }
        // If no component box can be pushed at all, none can ever move: every
        // push cell of a component box is adjacent to it, so only component
        // boxes — all currently immovable — could unblock it.
        for slot in 0..size {
            let cell = cell_of(component[slot]);
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
        (0..size).any(|slot| {
            let i = component[slot];
            board.tiles[cell_of(i) as usize] != board.labels[i]
        })
    }
    fn any_frozen_off_goal(&self, board: &Board, boxes: &[Cell]) -> bool {
        let n = boxes.len();
        let mut frozen = [false; MAX_BOXES];
        let mut changed = true;
        while changed {
            changed = false;
            for i in 0..n {
                if frozen[i] {
                    continue;
                }
                let neighbors = board.neighbors[boxes[i] as usize];
                let blocker = |cell: Cell| cell == NONE || self.at(cell).is_some_and(|j| frozen[j]);
                if (blocker(neighbors[0]) || blocker(neighbors[1]))
                    && (blocker(neighbors[2]) || blocker(neighbors[3]))
                {
                    frozen[i] = true;
                    changed = true;
                }
            }
        }
        (0..n).any(|i| frozen[i] && board.tiles[boxes[i] as usize] != board.labels[i])
    }
}
