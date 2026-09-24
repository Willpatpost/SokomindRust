use sokomind_core::{ACTIONS, Board, Cell, NONE, OPPOSITE, State};

pub struct Reach {
    pub occupied: Vec<bool>,
    pub distances: Vec<u16>,
    parent_direction: Vec<u8>,
    queue: Vec<Cell>,
}
impl Reach {
    pub fn new(cells: usize) -> Self {
        Self {
            occupied: vec![false; cells],
            distances: vec![NONE; cells],
            parent_direction: vec![0; cells],
            queue: Vec::with_capacity(cells),
        }
    }
    pub fn fill(&mut self, board: &Board, state: &State) {
        self.occupied.fill(false);
        self.distances.fill(NONE);
        self.queue.clear();
        for &cell in &state.boxes[..board.labels.len()] {
            self.occupied[cell as usize] = true;
        }
        self.distances[state.player as usize] = 0;
        self.queue.push(state.player);
        let mut head = 0;
        while head < self.queue.len() {
            let cell = self.queue[head];
            head += 1;
            for d in 0..4 {
                let next = board.neighbors[cell as usize][d];
                if next != NONE
                    && !self.occupied[next as usize]
                    && self.distances[next as usize] == NONE
                {
                    self.distances[next as usize] = self.distances[cell as usize] + 1;
                    self.parent_direction[next as usize] = d as u8;
                    self.queue.push(next);
                }
            }
        }
    }
    pub fn append_path(&self, board: &Board, mut cell: Cell, route: &mut Vec<u8>) {
        let start = route.len();
        while self.distances[cell as usize] != 0 {
            let d = self.parent_direction[cell as usize] as usize;
            route.push(ACTIONS[d]);
            cell = board.neighbors[cell as usize][OPPOSITE[d]];
        }
        route[start..].reverse();
    }
}
