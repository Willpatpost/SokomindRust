//! Which side of a cell the keeper is on. A box on `cell` splits the rest
//! of the floor into at most four parts, the cell's sides: two floor
//! neighbors of the cell share a side exactly when they stay connected with
//! the cell removed. Other boxes never change which side of a box the
//! keeper is on: it only steps onto cells no box is on, so until that box
//! moves the keeper stays in one part of the floor without the box's cell.
//! The heuristic counts pushes per (cell, side) for that reason.
//!
//! One iterative Tarjan pass over the floor answers both lookups: the side
//! a cell's neighbor is on ([`Sides::toward`]) and the side holding any
//! keeper cell ([`Sides::holding`]).
use sokomind_core::{Board, Cell, NONE, WALL};
use std::mem::size_of;

/// A cell has at most one side per neighbor.
pub(crate) const SIDES: usize = 4;

/// The sides of every cell, and what places a keeper cell on one.
pub(crate) struct Sides {
    /// `side[cell * SIDES + d]`: the side of `cell` its neighbor in
    /// direction `d` is on, numbered from 0 in direction order of first
    /// appearance; 0 where there is no neighbor.
    side: Vec<u8>,
    /// Each floor cell's DFS discovery time, counted from 1.
    order: Vec<u16>,
    /// `parts[cell * SIDES + d]`: when the neighbor in direction `d` is a
    /// DFS child of `cell` whose subtree reaches the rest of the floor only
    /// through `cell`, that subtree's discovery times as `(first, last)`;
    /// the empty `(1, 0)` otherwise.
    parts: Vec<(u16, u16)>,
    /// The side of each cell its DFS parent is on, which holds every cell
    /// outside the subtrees in `parts`; 0 at a DFS root.
    above: Vec<u8>,
}
// Discovery times count floor cells from 1, so they fit u16.
const _: () = assert!(sokomind_core::MAX_CELLS < u16::MAX as usize);

impl Sides {
    /// The bytes [`Sides::new`] keeps per board cell.
    pub(crate) const BYTES_PER_CELL: usize =
        SIDES * (size_of::<u8>() + size_of::<(u16, u16)>()) + size_of::<u16>() + size_of::<u8>();

    pub(crate) fn new(board: &Board) -> Self {
        let neighbors = board.neighbors();
        let cells = neighbors.len();
        let mut order = vec![0u16; cells];
        // The last discovery time inside each cell's subtree, the lowest
        // discovery time its subtree reaches by one non-tree edge, and its
        // DFS parent. Counting the tree edge to the parent in `low` too only
        // ever lowers a child's `low` to its parent's own time, which the
        // `low >= order` test below still passes, so no edge is skipped.
        let mut last = vec![0u16; cells];
        let mut low = vec![0u16; cells];
        let mut parent = vec![NONE; cells];
        // (cell, next direction to try)
        let mut stack: Vec<(Cell, usize)> = Vec::new();
        let mut time = 0;
        for root in 0..cells {
            if board.tiles()[root] == WALL || order[root] != 0 {
                continue;
            }
            time += 1;
            order[root] = time;
            low[root] = time;
            stack.push((root as Cell, 0));
            while let Some(top) = stack.last_mut() {
                let cell = top.0 as usize;
                if let Some(&next) = neighbors[cell].get(top.1) {
                    top.1 += 1;
                    if next == NONE {
                        continue;
                    }
                    let next = next as usize;
                    if order[next] == 0 {
                        time += 1;
                        order[next] = time;
                        low[next] = time;
                        parent[next] = cell as Cell;
                        stack.push((next as Cell, 0));
                    } else {
                        low[cell] = low[cell].min(order[next]);
                    }
                } else {
                    stack.pop();
                    last[cell] = time;
                    let up = parent[cell] as usize;
                    if up != NONE as usize {
                        low[up] = low[up].min(low[cell]);
                    }
                }
            }
        }
        let mut side = vec![0; cells * SIDES];
        let mut parts = vec![(1, 0); cells * SIDES];
        let mut above = vec![0; cells];
        for (cell, around) in neighbors.iter().enumerate() {
            // Per side, the child heading its cut-off subtree, or NONE for
            // the part above the cell.
            let mut heads = [NONE; SIDES];
            let mut count = 0;
            for (d, &next) in around.iter().enumerate() {
                if next == NONE {
                    continue;
                }
                let at = order[next as usize];
                // A neighbor inside the subtree of a child that reaches no
                // higher than the cell is on that child's side; every other
                // neighbor is on the side above.
                let head = around
                    .iter()
                    .copied()
                    .find(|&child| {
                        child != NONE
                            && parent[child as usize] == cell as Cell
                            && (order[child as usize]..=last[child as usize]).contains(&at)
                    })
                    .filter(|&child| low[child as usize] >= order[cell])
                    .unwrap_or(NONE);
                let s = match heads[..count].iter().position(|&seen| seen == head) {
                    Some(s) => s,
                    None => {
                        heads[count] = head;
                        count += 1;
                        count - 1
                    }
                };
                side[cell * SIDES + d] = s as u8;
                if head == next {
                    parts[cell * SIDES + d] = (order[next as usize], last[next as usize]);
                } else if head == NONE {
                    above[cell] = s as u8;
                }
            }
        }
        Self {
            side,
            order,
            parts,
            above,
        }
    }

    /// The side of `cell` that its neighbor in direction `d` is on, which
    /// must exist.
    pub(crate) fn toward(&self, cell: Cell, d: usize) -> usize {
        usize::from(self.side[cell as usize * SIDES + d])
    }

    /// The side of `cell` holding `keeper`, another floor cell. A keeper
    /// the floor never connects to `cell` gets a fixed side, the one above
    /// it: no push ever moves a box there, so any fixed side bounds its
    /// pushes as well as another, and it never changes.
    pub(crate) fn holding(&self, cell: Cell, keeper: Cell) -> usize {
        let at = self.order[keeper as usize];
        let base = cell as usize * SIDES;
        self.parts[base..base + SIDES]
            .iter()
            .position(|&(first, last)| (first..=last).contains(&at))
            .map_or(usize::from(self.above[cell as usize]), |d| {
                usize::from(self.side[base + d])
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{SIDES, Sides};
    use crate::testkit::{Lcg, catalog, components, random_room};
    use sokomind_core::{Board, Cell, NONE, WALL};
    use std::mem::size_of;

    /// Both lookups against a flood of the floor without each cell, by index
    /// arithmetic apart from the neighbor table, on the catalog and on small
    /// walled rooms, some split by their walls: the neighbors of a cell
    /// share a side exactly when they stay connected without it, sides are
    /// numbered by first neighbor in direction order, and a keeper cell gets
    /// the side its part of the floor touches. A keeper the floor never
    /// connects to the cell gets some side. The tables hold exactly
    /// `BYTES_PER_CELL` bytes per cell.
    #[test]
    fn sides_match_a_flood_without_the_cell() {
        let mut rng = Lcg(5);
        let rooms = (0..300).map(|_| Board::parse(&random_room(&mut rng)).unwrap());
        for board in catalog().into_iter().map(|(_, board)| board).chain(rooms) {
            let sides = Sides::new(&board);
            assert_eq!(
                sides.side.len()
                    + sides.order.len() * size_of::<u16>()
                    + sides.parts.len() * size_of::<(u16, u16)>()
                    + sides.above.len(),
                board.tiles().len() * Sides::BYTES_PER_CELL
            );
            let floor = (0..board.tiles().len()).filter(|&cell| board.tiles()[cell] != WALL);
            for cell in floor.clone() {
                let part = components(&board, Some(cell));
                // Each part next to the cell, in side order.
                let mut touching = Vec::new();
                for (d, &next) in board.neighbors()[cell].iter().enumerate() {
                    if next == NONE {
                        continue;
                    }
                    let next_part = part[next as usize];
                    if !touching.contains(&next_part) {
                        touching.push(next_part);
                    }
                    let side = touching.iter().position(|&p| p == next_part).unwrap();
                    assert_eq!(
                        sides.toward(cell as Cell, d),
                        side,
                        "{} {cell} {d}",
                        board.fingerprint()
                    );
                }
                for keeper in floor.clone().filter(|&keeper| keeper != cell) {
                    let side = sides.holding(cell as Cell, keeper as Cell);
                    match touching.iter().position(|&p| p == part[keeper]) {
                        Some(expected) => {
                            assert_eq!(side, expected, "{} {cell} {keeper}", board.fingerprint())
                        }
                        None => assert!(side < SIDES),
                    }
                }
            }
        }
    }
}
