use sokomind_core::{Board, Cell, NONE, OPPOSITE, State};
use std::mem::size_of;

/// Keeper-reachability flood with epoch stamps: fills cost only the cells
/// they actually visit, and never reset their buffers between calls.
pub struct Reach {
    distances: Vec<u16>,
    stamps: Vec<u32>,
    epoch: u32,
    queue: Vec<Cell>,
}
impl Reach {
    /// Distances, stamps and the queue.
    pub(crate) const BYTES_PER_CELL: usize =
        size_of::<u16>() + size_of::<u32>() + size_of::<Cell>();
    pub fn new(cells: usize) -> Self {
        Self {
            distances: vec![NONE; cells],
            stamps: vec![0; cells],
            epoch: 0,
            queue: Vec::with_capacity(cells),
        }
    }
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
    pub fn fill(&mut self, board: &Board, state: &State) {
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
    pub fn distance(&self, cell: Cell) -> u16 {
        if self.stamps[cell as usize] == self.epoch {
            self.distances[cell as usize]
        } else {
            NONE
        }
    }
    pub fn blocked(&self, cell: Cell) -> bool {
        self.stamps[cell as usize] == self.epoch && self.distances[cell as usize] == NONE
    }
    /// Appends a shortest walk from the player to `cell`, which the last
    /// `fill` reached, as direction indices from its last step back to its
    /// first. Each step takes the first direction, in `U D L R` order, whose
    /// predecessor is one step closer to the player; box cells carry no
    /// distance, so the walk never enters one.
    pub fn append_walk_reversed(&self, board: &Board, mut cell: Cell, route: &mut Vec<u8>) {
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
    use sokomind_core::{Board, Cell, MAX_BOXES, NONE};

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
                assert_eq!(board.step(&mut state, d as usize), Some(MAX_BOXES));
            }
            assert_eq!(state.player, cell);
            assert_eq!(state.boxes, start.boxes);
            walks += 1;
        }
        assert!(walks > 10, "{walks}");
    }
}
