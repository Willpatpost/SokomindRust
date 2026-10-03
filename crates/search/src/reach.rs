//! Keeper reachability by breadth-first flood. Each fill bumps an epoch
//! instead of clearing its buffers, so it costs only the cells it visits;
//! the engine reads distances, blocked cells and walks from the last fill.
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
    /// stamps when it wraps, then stamps the blocked cells and queues the
    /// keeper.
    fn begin(&mut self, player: Cell, blocked: &[Cell]) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamps.fill(0);
            self.epoch = 1;
        }
        self.queue.clear();
        // Blocked cells are stamped without a distance, which blocks the
        // flood and, in a fill of a state, marks its box cells occupied for
        // push generation.
        for &cell in blocked {
            self.stamps[cell as usize] = self.epoch;
            self.distances[cell as usize] = NONE;
        }
        self.stamps[player as usize] = self.epoch;
        self.distances[player as usize] = 0;
        self.queue.push(player);
    }
    /// Floods every cell the player reaches in `state`, its boxes blocked.
    pub(crate) fn fill(&mut self, board: &Board, state: &State) {
        self.fill_from(board, state.player, &state.boxes[..board.labels().len()]);
    }
    /// Floods every cell `player` reaches with the `blocked` cells as walls
    /// for this flood alone, such as a state's boxes or only some of them.
    /// The player is stamped after the blocked cells, so it must not be one;
    /// afterwards [`Self::blocked`] reads exactly those cells and
    /// [`Self::distance`] gives `NONE` for each.
    pub(crate) fn fill_from(&mut self, board: &Board, player: Cell, blocked: &[Cell]) {
        debug_assert!(!blocked.contains(&player));
        self.begin(player, blocked);
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
    /// for a blocked cell or one the player cannot reach.
    pub(crate) fn distance(&self, cell: Cell) -> u16 {
        if self.stamps[cell as usize] == self.epoch {
            self.distances[cell as usize]
        } else {
            NONE
        }
    }
    /// Whether `cell` was blocked in the last fill, which in a fill of a
    /// state means it held a box.
    pub(crate) fn blocked(&self, cell: Cell) -> bool {
        self.stamps[cell as usize] == self.epoch && self.distances[cell as usize] == NONE
    }
    /// How many cells the last fill reached, the player's included. The
    /// queue holds each reached cell once, and blocked cells are stamped but
    /// never queued.
    pub(crate) fn reached(&self) -> usize {
        self.queue.len()
    }
    /// Appends a shortest walk from the player to `cell`, which the last
    /// fill reached, as direction indices from its last step back to its
    /// first. Each step takes the first direction, in `U D L R` order, whose
    /// predecessor is one step closer to the player; blocked cells carry no
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
    use crate::testkit::{Lcg, catalog, random_room};
    use sokomind_core::{Board, Cell, NONE, Step, WALL};
    use std::collections::VecDeque;

    /// Walls and two boxes split the floor into branching corridors.
    const ROOM: &str = "OOOOOOOO\nOR  O  O\nO AOO  O\nO  B   O\nOa b   O\nOOOOOOOO";

    /// Every reached cell gets a walk as long as its distance that replays
    /// as plain moves, never touching a box, and ends on the cell, and
    /// `reached` counts exactly those cells.
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
        assert_eq!(reach.reached(), walks);
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

    /// `fill_from` against a plain breadth-first search on every catalog
    /// board and 200 random rooms, with one flood reused across each board's
    /// trials so stale stamps would show: from the start with nothing
    /// blocked, from the start with its boxes blocked (through `fill`), then
    /// from random floor cells with random other floor cells blocked, from
    /// all of them down to about one in six. Every cell's distance and
    /// blocked flag, and the reached count, must match.
    #[test]
    fn fill_from_matches_a_reference_search() {
        let mut rng = Lcg(0xf111);
        let mut boards = catalog();
        for n in 0..200 {
            let board = Board::parse(&random_room(&mut rng)).unwrap();
            boards.push((format!("room {n}"), board));
        }
        for (id, board) in &boards {
            let (start, tiles) = (board.initial(), board.tiles());
            let floor: Vec<Cell> = (0..tiles.len() as Cell)
                .filter(|&cell| tiles[cell as usize] != WALL)
                .collect();
            let mut reach = Reach::new(tiles.len());
            for trial in 0..8 {
                let (player, blocked): (Cell, Vec<Cell>) = match trial {
                    0 => (start.player, Vec::new()),
                    1 => (start.player, start.boxes[..board.labels().len()].to_vec()),
                    _ => {
                        let player = floor[rng.below(floor.len())];
                        let blocked = floor
                            .iter()
                            .copied()
                            .filter(|&cell| cell != player && rng.below(trial - 1) == 0)
                            .collect();
                        (player, blocked)
                    }
                };
                if trial == 1 {
                    reach.fill(board, &start);
                } else {
                    reach.fill_from(board, player, &blocked);
                }
                let mut is_blocked = vec![false; tiles.len()];
                for &cell in &blocked {
                    is_blocked[cell as usize] = true;
                }
                let reference = reference_distances(board, player, &is_blocked);
                let mut reached = 0;
                for (cell, &distance) in reference.iter().enumerate() {
                    reached += usize::from(distance != NONE);
                    let expected = (distance, is_blocked[cell]);
                    let cell = cell as Cell;
                    assert_eq!(
                        (reach.distance(cell), reach.blocked(cell)),
                        expected,
                        "{id} trial {trial} cell {cell}"
                    );
                }
                assert_eq!(reach.reached(), reached, "{id} trial {trial}");
            }
        }
    }

    /// Walking distances from `player` by a plain breadth-first search over
    /// the tiles, apart from the board's neighbor table, with the `blocked`
    /// cells as walls: `NONE` for a wall, a blocked cell or one out of reach.
    fn reference_distances(board: &Board, player: Cell, blocked: &[bool]) -> Vec<u16> {
        let (tiles, width) = (board.tiles(), board.width());
        let mut distances = vec![NONE; tiles.len()];
        distances[player as usize] = 0;
        let mut queue = VecDeque::from([player as usize]);
        while let Some(cell) = queue.pop_front() {
            let x = cell % width;
            let steps = [
                cell.checked_sub(width),
                Some(cell + width),
                x.checked_sub(1).map(|_| cell - 1),
                (x + 1 < width).then_some(cell + 1),
            ];
            for next in steps.into_iter().flatten() {
                if tiles.get(next).is_some_and(|&tile| tile != WALL)
                    && !blocked[next]
                    && distances[next] == NONE
                {
                    distances[next] = distances[cell] + 1;
                    queue.push_back(next);
                }
            }
        }
        distances
    }
}
