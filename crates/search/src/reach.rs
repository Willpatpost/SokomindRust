//! Keeper reachability by breadth-first flood. Each fill bumps an epoch
//! instead of clearing its buffers, so it costs only the cells it visits;
//! the engine reads distances, box cells and walks from the last fill.
use sokomind_core::{Board, Cell, NONE, OPPOSITE, State};
use std::mem::size_of;

/// Keeper-reachability flood with epoch stamps: fills cost only the cells
/// they actually visit, and never reset their buffers between calls.
pub(crate) struct Reach {
    distances: Vec<u16>,
    stamps: Vec<u32>,
    epoch: u32,
    queue: Vec<Cell>,
}
impl Reach {
    /// Distances, stamps and the queue.
    pub(crate) const BYTES_PER_CELL: usize =
        size_of::<u16>() + size_of::<u32>() + size_of::<Cell>();
    /// An empty flood over a board of `cells` cells, allocated once.
    pub(crate) fn new(cells: usize) -> Self {
        Self {
            distances: vec![NONE; cells],
            stamps: vec![0; cells],
            epoch: 0,
            queue: Vec::with_capacity(cells),
        }
    }
    /// Starts a flood: bumps the epoch so every stamp is stale, zeroing the
    /// stamps when it wraps, then stamps the boxes and queues the keeper.
    fn begin(&mut self, board: &Board, state: &State) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamps.fill(0);
            self.epoch = 1;
        }
        self.queue.clear();
        // Boxes are stamped without a distance, which blocks the flood and
        // marks the cell occupied for push generation.
        for &cell in &state.boxes[..board.labels().len()] {
            self.stamps[cell as usize] = self.epoch;
            self.distances[cell as usize] = NONE;
        }
        self.stamps[state.player as usize] = self.epoch;
        self.distances[state.player as usize] = 0;
        self.queue.push(state.player);
    }
    /// Floods every cell the player reaches in `state`, marking box cells.
    pub(crate) fn fill(&mut self, board: &Board, state: &State) {
        self.begin(board, state);
        let mut head = 0;
        while head < self.queue.len() {
            let cell = self.queue[head];
            head += 1;
            for d in 0..4 {
                let next = board.neighbors()[cell as usize][d];
                if next != NONE && self.stamps[next as usize] != self.epoch {
                    self.stamps[next as usize] = self.epoch;
                    self.distances[next as usize] = self.distances[cell as usize] + 1;
                    self.queue.push(next);
                }
            }
        }
    }
    /// Walking distance from the player to `cell` in the last fill, `NONE`
    /// for a box cell or one the player cannot reach.
    pub(crate) fn distance(&self, cell: Cell) -> u16 {
        if self.stamps[cell as usize] == self.epoch {
            self.distances[cell as usize]
        } else {
            NONE
        }
    }
    /// Whether `cell` held a box in the last fill.
    pub(crate) fn blocked(&self, cell: Cell) -> bool {
        self.stamps[cell as usize] == self.epoch && self.distances[cell as usize] == NONE
    }
    /// Appends a shortest walk from the player to `cell`, which the last
    /// `fill` reached, as direction indices from its last step back to its
    /// first. Each step takes the first direction, in `U D L R` order, whose
    /// predecessor is one step closer to the player; box cells carry no
    /// distance, so the walk never enters one.
    pub(crate) fn append_walk_reversed(&self, board: &Board, mut cell: Cell, route: &mut Vec<u8>) {
        let mut distance = self.distance(cell);
        while distance != 0 {
            let neighbors = board.neighbors()[cell as usize];
            // A flooded cell always has such a predecessor; were it missing,
            // the short route would fail the caller's length check.
            let Some(d) = (0..4).find(|&d| {
                let previous = neighbors[OPPOSITE[d]];
                previous != NONE && self.distance(previous) == distance - 1
            }) else {
                return;
            };
            route.push(d as u8);
            cell = neighbors[OPPOSITE[d]];
            distance -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Reach;
    use sokomind_core::{Board, Cell, NONE, Step};

    /// Walls and two boxes split the floor into branching corridors.
    const ROOM: &str = "OOOOOOOO\nOR  O  O\nO AOO  O\nO  B   O\nOa b   O\nOOOOOOOO";

    /// Every reached cell gets a walk as long as its distance that replays
    /// as plain moves, never touching a box, and ends on the cell.
    #[test]
    fn walk_replays_as_a_shortest_path() {
        let board = Board::parse(ROOM).unwrap();
        let start = board.initial();
        let mut reach = Reach::new(board.tiles().len());
        reach.fill(&board, &start);
        let mut walks = 0;
        for cell in 0..board.tiles().len() {
            let cell = cell as Cell;
            let distance = reach.distance(cell);
            if distance == NONE {
                continue;
            }
            let mut route = Vec::new();
            reach.append_walk_reversed(&board, cell, &mut route);
            route.reverse();
            assert_eq!(route.len(), distance as usize, "cell {cell}");
            let mut state = start;
            for &d in &route {
                assert_eq!(board.step(&mut state, d as usize), Some(Step::Walk));
            }
            assert_eq!(state.player, cell);
            assert_eq!(state.boxes, start.boxes);
            walks += 1;
        }
        assert!(walks > 10, "{walks}");
    }

    /// The fill after epoch `u32::MAX` wraps: it zeroes every stamp and
    /// restarts at epoch 1, so neither the stamps an old flood left at 1 nor
    /// the walls still at 0 from `new` pass for reached or box cells.
    #[test]
    fn epoch_wrap_clears_stale_stamps() {
        let board = Board::parse(ROOM).unwrap();
        let start = board.initial();
        // Walk around to box B and push it right, so the old flood measures
        // from another cell and stamps a box on a cell that is floor now.
        let mut other = start;
        for d in [1, 1, 1, 3, 0, 3] {
            assert!(board.step(&mut other, d).is_some(), "direction {d}");
        }
        assert_ne!(other.boxes, start.boxes);
        let cells = board.tiles().len();
        let mut reach = Reach::new(cells);
        reach.fill(&board, &other);
        assert_eq!(reach.epoch, 1);
        // Skip the 2^32 - 2 fills between, leaving the old stamps in place.
        reach.epoch = u32::MAX;
        reach.fill(&board, &start);
        assert_eq!(reach.epoch, 1, "the fill wrapped the epoch");
        let mut fresh = Reach::new(cells);
        fresh.fill(&board, &start);
        for cell in 0..cells {
            let cell = cell as Cell;
            assert_eq!(reach.distance(cell), fresh.distance(cell), "cell {cell}");
            assert_eq!(reach.blocked(cell), fresh.blocked(cell), "cell {cell}");
            if fresh.distance(cell) != NONE {
                let (mut walk, mut fresh_walk) = (Vec::new(), Vec::new());
                reach.append_walk_reversed(&board, cell, &mut walk);
                fresh.append_walk_reversed(&board, cell, &mut fresh_walk);
                assert_eq!(walk, fresh_walk, "cell {cell}");
            }
        }
    }
}
