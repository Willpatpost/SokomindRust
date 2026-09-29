//! Keeper regions for 5.5 O6 (feature `o6`): whether a stored keeper lies in
//! the region of a child the expansion is about to generate. A Fast arena
//! stores one record per box set and keeper region, so a duplicate probe
//! asks this once per stored record with the child's boxes, and usually
//! gets an O(1) answer from the parent's flood.
use crate::reach::Reach;
use sokomind_core::{Board, Cell, MAX_CELLS, NONE};

/// A set of cells in a fixed 512-byte bitmap, part of the fixed allowance
/// in [`crate::arena::Arena::new`]; clearing touches every word.
pub(crate) struct CellSet([u64; MAX_CELLS / 64]);
impl CellSet {
    pub(crate) const EMPTY: Self = Self([0; MAX_CELLS / 64]);
    pub(crate) fn clear(&mut self) {
        self.0.fill(0);
    }
    pub(crate) fn insert(&mut self, cell: Cell) {
        self.0[cell as usize / 64] |= 1 << (cell % 64);
    }
    pub(crate) fn contains(&self, cell: Cell) -> bool {
        self.0[cell as usize / 64] & (1 << (cell % 64)) != 0
    }
}

/// Up, right, down, left: the four sides of a cell in ring order.
const RING: [usize; 4] = [0, 3, 1, 2];

/// The keeper's region after the expanded state's box on `from` is pushed
/// to `to`: the component of `from` in the floor without the child's boxes.
/// Everything it reads is the reach fill of the expanded state, which the
/// expansion keeps for all its children.
///
/// With R the filled region, the child region is `R - {to} + {from}` when
/// both side cells of `from` are walls, boxes or in R (`enclosed`: `from`
/// opens onto nothing new) and removing `to` cannot split R (`connected`).
/// It is a subset when only `enclosed` holds and a superset when only
/// `connected` does, which answers every keeper on the decided side. The
/// rest grows one flood per child, shared by every keeper it tests, and
/// stops as soon as it reaches the keeper.
pub(crate) struct ChildRegion {
    from: Cell,
    to: Cell,
    direction: usize,
    shape: Option<Shape>,
    /// Queue position of the child flood once it has started.
    flood: Option<usize>,
}

#[derive(Clone, Copy)]
struct Shape {
    enclosed: bool,
    connected: bool,
}

impl ChildRegion {
    pub(crate) fn new(from: Cell, to: Cell, direction: usize) -> Self {
        Self {
            from,
            to,
            direction,
            shape: None,
            flood: None,
        }
    }
    /// Whether `keeper`, the player of a stored state with the child's
    /// boxes, is in the child's region. `region` is scratch owned by this
    /// child until the probe ends.
    pub(crate) fn contains(
        &mut self,
        board: &Board,
        reach: &mut Reach,
        region: &mut CellSet,
        keeper: Cell,
    ) -> bool {
        debug_assert_ne!(keeper, self.to, "a keeper never stands on a box");
        if keeper == self.from {
            return true;
        }
        let (from, to, direction) = (self.from, self.to, self.direction);
        let shape = *self
            .shape
            .get_or_insert_with(|| Shape::of(board, reach, from, to, direction));
        // The keeper stands on free floor of the child, so never on `to`.
        let in_parent = reach.distance(keeper) != NONE;
        match (shape.enclosed, shape.connected, in_parent) {
            (true, true, _) => in_parent,
            (true, false, false) => false,
            (false, true, true) => true,
            _ => reach.grow_child_region(board, (from, to), region, &mut self.flood, keeper),
        }
    }
}

impl Shape {
    fn of(board: &Board, reach: &Reach, from: Cell, to: Cell, direction: usize) -> Self {
        let neighbors = board.neighbors()[from as usize];
        // The two sides across the push: left and right of a vertical one.
        let sides = (direction & 2) ^ 2;
        let enclosed = [neighbors[sides], neighbors[sides | 1]]
            .iter()
            .all(|&cell| cell == NONE || reach.settled(cell));
        let connected = reach.distance(to) == NONE || detours_around(board, reach, from, to);
        Self {
            enclosed,
            connected,
        }
    }
}

/// Whether the free sides of `to` stay linked around it once it holds a
/// box: they form one run in ring order, consecutive sides joined through a
/// free corner. Then a walk through `to` can step around it, so removing it
/// cannot split the region it was in. Free means floor without a box after
/// the push: `from` is free, and `to` is never a side or corner of itself.
/// A false answer may still be connected by a longer detour; that only
/// costs a flood.
fn detours_around(board: &Board, reach: &Reach, from: Cell, to: Cell) -> bool {
    let free = |cell: Cell| cell != NONE && (cell == from || !reach.blocked(cell));
    let sides = RING.map(|d| board.neighbors()[to as usize][d]);
    let open = sides.iter().filter(|&&cell| free(cell)).count();
    let links = (0..4)
        .filter(|&k| {
            let (side, next) = (sides[k], sides[(k + 1) % 4]);
            free(side) && free(next) && free(board.neighbors()[side as usize][RING[(k + 1) % 4]])
        })
        .count();
    // Links never outnumber open sides; four of each is the full ring.
    open - links == 1 || links == 4
}

#[cfg(test)]
mod tests {
    use super::{CellSet, ChildRegion};
    use crate::reach::Reach;
    use sokomind_core::{Board, NONE, OPPOSITE, State};

    /// Every catalog board with its id.
    fn catalog() -> Vec<(String, Board)> {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../../data/puzzles.json")).unwrap();
        catalog
            .as_array()
            .unwrap()
            .iter()
            .map(|puzzle| {
                let rows: Vec<&str> = puzzle["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row.as_str().unwrap())
                    .collect();
                let board = Board::parse(&rows.join("\n")).unwrap();
                (puzzle["id"].as_str().unwrap().to_owned(), board)
            })
            .collect()
    }

    /// Every legal push of `state`, as the engine generates them before any
    /// pruning: box index, from, to, direction.
    fn pushes(board: &Board, reach: &Reach, state: &State) -> Vec<(usize, u16, u16, usize)> {
        let mut out = Vec::new();
        for i in 0..board.labels().len() {
            let from = state.boxes[i];
            for (d, &opposite) in OPPOSITE.iter().enumerate() {
                let to = board.neighbors()[from as usize][d];
                let stand = board.neighbors()[from as usize][opposite];
                if to != NONE
                    && stand != NONE
                    && !reach.blocked(to)
                    && reach.distance(stand) != NONE
                {
                    out.push((i, from, to, d));
                }
            }
        }
        out
    }

    /// For every state within two pushes of each catalog start, every push
    /// and every floor cell off the child's boxes, asked in scrambled order
    /// so the flood is grown and reused: the answer is membership in a full
    /// flood of the child, and the expanded state's fill survives intact.
    #[test]
    fn child_region_matches_a_full_flood() {
        let (mut quick, mut flooded, mut asked) = (0, 0, 0);
        for (id, board) in catalog() {
            let cells = board.tiles().len();
            let (mut reach, mut truth) = (Reach::new(cells), Reach::new(cells));
            let mut region = CellSet::EMPTY;
            let mut layer = vec![board.initial()];
            for _ in 0..2 {
                let mut next_layer = Vec::new();
                for state in &layer {
                    reach.fill(&board, state);
                    let before: Vec<u16> = (0..cells as u16).map(|c| reach.distance(c)).collect();
                    for (i, from, to, d) in pushes(&board, &reach, state) {
                        let mut child = *state;
                        child.player = from;
                        child.boxes[i] = to;
                        truth.fill(&board, &child);
                        let mut probe = ChildRegion::new(from, to, d);
                        // Coprime stride: every cell once, far from flood order.
                        for k in 0..cells {
                            let keeper = ((k * 7919) % cells) as u16;
                            if board.neighbors()[keeper as usize] == [NONE; 4]
                                || child.boxes[..board.labels().len()].contains(&keeper)
                            {
                                continue;
                            }
                            let expected = truth.distance(keeper) != NONE;
                            let got = probe.contains(&board, &mut reach, &mut region, keeper);
                            assert_eq!(got, expected, "{id}: {from}->{to} keeper {keeper}");
                            asked += 1;
                        }
                        if probe.flood.is_some() {
                            flooded += 1;
                        } else {
                            quick += 1;
                        }
                        next_layer.push(child);
                    }
                    let after: Vec<u16> = (0..cells as u16).map(|c| reach.distance(c)).collect();
                    assert_eq!(before, after, "{id}: the parent fill changed");
                }
                // Debug builds stay fast on the boards with many boxes.
                next_layer.truncate(64);
                layer = next_layer;
            }
        }
        assert!(
            quick > 0 && flooded > 0 && asked > 10_000,
            "{quick} {flooded} {asked}"
        );
    }

    #[test]
    fn cell_sets_hold_exactly_what_was_inserted() {
        let mut set = CellSet::EMPTY;
        for cell in [0, 63, 64, 4095] {
            set.insert(cell);
        }
        assert!([0, 63, 64, 4095].iter().all(|&cell| set.contains(cell)));
        assert!([1, 62, 65, 4094].iter().all(|&cell| !set.contains(cell)));
        set.clear();
        assert!(!set.contains(4095));
    }
}
