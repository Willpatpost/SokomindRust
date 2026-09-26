use sokomind_core::{Board, Cell, NONE, OPPOSITE, State};
use std::mem::size_of;

/// Keeper-reachability flood with epoch stamps: fills cost only the cells
/// they actually visit, and never reset their buffers between calls.
pub struct Reach {
    distances: Vec<u16>,
    stamps: Vec<u32>,
    parent_direction: Vec<u8>,
    epoch: u32,
    queue: Vec<Cell>,
}
impl Reach {
    /// Distances, stamps, parent directions and the queue.
    pub(crate) const BYTES_PER_CELL: usize =
        size_of::<u16>() + size_of::<u32>() + size_of::<u8>() + size_of::<Cell>();
    pub fn new(cells: usize) -> Self {
        Self {
            distances: vec![NONE; cells],
            stamps: vec![0; cells],
            parent_direction: vec![0; cells],
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
                    self.parent_direction[next as usize] = d as u8;
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
    /// Appends the walk from the player to `cell`, which the last `fill`
    /// reached, as direction indices from its last step back to its first.
    pub fn append_walk_reversed(&self, board: &Board, mut cell: Cell, route: &mut Vec<u8>) {
        while self.distance(cell) != 0 {
            let d = self.parent_direction[cell as usize];
            route.push(d);
            cell = board.neighbors()[cell as usize][OPPOSITE[d as usize]];
        }
    }
}
