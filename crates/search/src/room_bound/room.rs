//! A room's geometry, and the keeper's moves its tables are built over.
//!
//! A room Q is a region of the keeper's floor, boxes ignored, whose
//! boundary — every floor cell outside Q next to Q — is its doors, at most
//! a few (see `candidates` in the parent module). With one door Q is a
//! side of it: a component of the floor once the door is taken out, and
//! every other side of the door touches it, so the floor outside Q is
//! connected. With more doors neither holds: no door need be an
//! articulation point, and the outside may fall into several components.
//! In the relaxed problem of a table no box lies outside Q, so the keeper
//! walks anywhere there for free, and one node stands for all of it;
//! merging the outside's parts is another freedom the table's costs do
//! not count, so the tables stay lower bounds (see docs/room-bound.md,
//! Rooms with several doors). The keeper's nodes are the Q indices
//! `0..n`, then `n` for the floor outside Q.
//!
//! Index `n` also names the doors as a box's cell: a push out of Q can
//! only land on a door, and a push into Q, an entry, can only start from
//! one. An entry leaves the keeper on its door, node `n`.
use super::bit;
use sokomind_core::{Board, Cell, NONE, OPPOSITE, State};
use std::mem::size_of;

/// Not in the room: an entry of `Room::index`.
const NOT: u8 = u8::MAX;
/// The most cells a room may have: parts are u64 bit sets over its cells,
/// and a node fits a u8.
pub(super) const MAX_ROOM: usize = 62;

/// One room Q: its doors and its cells, which every table over Q shares.
/// A built table keeps its room, so the room's bytes count while it lives.
pub(super) struct Room {
    /// The doors, sorted: the floor cells outside Q next to Q, which the
    /// candidate that made the room listed.
    doors: Vec<Cell>,
    /// Q's cells in increasing order; a cell's position is its Q index.
    cells: Vec<Cell>,
    /// Per board cell, its Q index, or `NOT`.
    index: Vec<u8>,
}

impl Room {
    /// The room on `cells` behind `doors` (see the module docs). `doors`
    /// must hold every floor cell outside the room next to it, which is
    /// what the tables' entries and exits cross; both lists are sorted
    /// here.
    pub(super) fn new(board: &Board, mut doors: Vec<Cell>, mut cells: Vec<Cell>) -> Self {
        debug_assert!(!cells.is_empty() && cells.len() <= MAX_ROOM);
        debug_assert!(!doors.is_empty());
        doors.sort_unstable();
        cells.sort_unstable();
        // `candidates` checks that the doors name the room's whole
        // boundary; hold it here too, so a generator that stops checking
        // cannot quietly make tables whose boundary it lied about.
        debug_assert!(cells.iter().all(|&c| {
            board.neighbors()[usize::from(c)].iter().all(|&next| {
                next == NONE
                    || doors.binary_search(&next).is_ok()
                    || cells.binary_search(&next).is_ok()
            })
        }));
        let mut index = vec![NOT; board.neighbors().len()];
        for (q, &cell) in cells.iter().enumerate() {
            index[usize::from(cell)] = q as u8;
        }
        Self {
            doors,
            cells,
            index,
        }
    }

    pub(super) fn doors(&self) -> &[Cell] {
        &self.doors
    }

    pub(super) fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// The Q index of `cell`, or `None` outside Q.
    pub(super) fn index(&self, cell: Cell) -> Option<usize> {
        match self.index[usize::from(cell)] {
            NOT => None,
            q => Some(usize::from(q)),
        }
    }

    /// The keeper's node on `cell`: its Q index, or `n` outside Q.
    pub(super) fn node(&self, cell: Cell) -> usize {
        self.index(cell).unwrap_or(self.cells.len())
    }

    pub(super) fn nodes(&self) -> usize {
        self.cells.len() + 1
    }

    /// The part holding every cell of Q.
    pub(super) fn all(&self) -> u64 {
        (1u64 << self.cells.len()) - 1
    }

    /// The heap bytes the room holds.
    pub(super) fn bytes(&self) -> usize {
        self.doors.capacity() * size_of::<Cell>()
            + self.cells.capacity() * size_of::<Cell>()
            + self.index.capacity()
    }

    /// Q split in two. With one door the line runs through that door's
    /// first neighbor in Q, by column when the neighbor is above or below
    /// the door, else by row, and the neighbor's side is the second half.
    /// With more doors no one door's line is right — the first door can
    /// sit at Q's edge and leave one half empty — so the line runs across
    /// Q's longer extent at the median cell along it: by row when Q is
    /// taller, by column when wider, ties by row. Either half may be
    /// empty; the split test drops such halves.
    pub(super) fn halves(&self, board: &Board) -> [u64; 2] {
        let width = board.width();
        let mut low = 0u64;
        if self.doors.len() == 1 {
            let door = self.doors[0];
            let neighbors = board.neighbors();
            let (d, inner) = (0..OPPOSITE.len())
                .find_map(|d| {
                    let r = neighbors[usize::from(door)][d];
                    (r != NONE && self.index[usize::from(r)] != NOT).then_some((d, r))
                })
                .expect("a room touches its doors");
            let line = |cell: Cell| {
                if d < 2 {
                    usize::from(cell) % width
                } else {
                    usize::from(cell) / width
                }
            };
            for (q, &cell) in self.cells.iter().enumerate() {
                if line(cell) < line(inner) {
                    low |= 1 << q;
                }
            }
            return [low, self.all() & !low];
        }
        let (mut rows, mut cols) = ((0usize, 0usize), (0usize, 0usize));
        for &cell in &self.cells {
            let c = usize::from(cell);
            rows.0 = rows.0.min(c / width);
            rows.1 = rows.1.max(c / width);
            cols.0 = cols.0.min(c % width);
            cols.1 = cols.1.max(c % width);
        }
        let by_row = rows.1 - rows.0 >= cols.1 - cols.0;
        let mut values: Vec<usize> = self
            .cells
            .iter()
            .map(|&cell| {
                let c = usize::from(cell);
                if by_row { c / width } else { c % width }
            })
            .collect();
        values.sort_unstable();
        let line = values[(values.len() - 1) / 2];
        for (q, &cell) in self.cells.iter().enumerate() {
            let c = usize::from(cell);
            let at = if by_row { c / width } else { c % width };
            if at < line {
                low |= 1 << q;
            }
        }
        [low, self.all() & !low]
    }

    /// The labels with a start box or a goal on `part`, sorted, each once.
    pub(super) fn labels(&self, board: &Board, start: &State, part: u64) -> Vec<u8> {
        let on = |cell: Cell| self.index(cell).is_some_and(|q| bit(part, q));
        let mut labels: Vec<u8> = board
            .labels()
            .iter()
            .zip(&start.boxes)
            .filter(|&(_, &cell)| on(cell))
            .map(|(&label, _)| label)
            .chain(
                board
                    .goals()
                    .iter()
                    .filter(|&&(cell, _)| on(cell))
                    .map(|&(_, label)| label),
            )
            .collect();
        labels.sort_unstable();
        labels.dedup();
        labels
    }
}

/// Per node, a list of moves, all in one vector: node `u`'s are
/// `items[starts[u]..starts[u + 1]]`. Only a keeper in Q, on the door or
/// next to it moves a box or crosses the door, at most four moves of a
/// kind from each such cell: a few hundred in all, so u16 offsets fit.
struct Lists<T> {
    starts: Vec<u16>,
    items: Vec<T>,
}

impl<T: Copy + Ord> Lists<T> {
    /// The lists of nodes `0..nodes` from `(node, move)` pairs in any order,
    /// each sorted with no repeats.
    fn new(nodes: usize, mut pairs: Vec<(u8, T)>) -> Self {
        pairs.sort_unstable();
        pairs.dedup();
        let starts = (0..=nodes)
            .map(|u| pairs.partition_point(|&(v, _)| usize::from(v) < u) as u16)
            .collect();
        let items = pairs.into_iter().map(|(_, item)| item).collect();
        Self { starts, items }
    }

    fn of(&self, u: usize) -> &[T] {
        &self.items[usize::from(self.starts[u])..usize::from(self.starts[u + 1])]
    }

    fn bytes(&self) -> usize {
        self.starts.capacity() * size_of::<u16>() + self.items.capacity() * size_of::<T>()
    }
}

/// The keeper's moves in a room's relaxed problem, forward and inverse.
/// The forward lists drive a table's forward search, and the inverse ones
/// its backward search from the goal states, which stores no edges. Built
/// for a room's table builds and dropped after its last one. Every list is
/// sorted with no repeats, so the builds run in one fixed order.
pub(super) struct Moves {
    /// Per node `u`, the other nodes one keeper step from `u` reaches.
    /// Steps are symmetric, so these are also the steps that end on `u`.
    steps: Lists<u8>,
    /// Per node `u`, the pushes a keeper on `u` makes: `(x, ti)` moves the
    /// box on Q index `x` onto Q index `ti`, or onto the door when `ti` is
    /// `n`. The keeper ends on `x`.
    pushes: Lists<(u8, u8)>,
    /// Per node `u`, the Q indices `ti` a keeper on `u` pushes a box onto
    /// from the door. The keeper ends on the door, node `n`.
    entries: Lists<u8>,
    /// Per Q index `w`, the pushes that end with the keeper on `w`:
    /// `(u, ti)` for each `(w, ti)` in the pushes of node `u`.
    pulls: Lists<(u8, u8)>,
    /// Every entry as `(u, ti)`, the keeper's node before it and the box's
    /// Q index after it: the entries that end on node `n`, which is all of
    /// them.
    unentries: Vec<(u8, u8)>,
}

impl Moves {
    /// The moves of `room`, whose doors are cells of `inside`, the
    /// keeper's floor with boxes ignored.
    pub(super) fn new(board: &Board, room: &Room, inside: &[bool]) -> Self {
        let neighbors = board.neighbors();
        let (mut steps, mut pushes, mut entries) = (Vec::new(), Vec::new(), Vec::new());
        for (s, near) in neighbors.iter().enumerate() {
            if !inside[s] {
                continue;
            }
            let u = room.node(s as Cell) as u8;
            for (d, &x) in near.iter().enumerate() {
                if x == NONE {
                    continue;
                }
                let v = room.node(x) as u8;
                if v != u {
                    steps.push((u, v));
                }
                let t = neighbors[usize::from(x)][d];
                if t == NONE {
                    continue;
                }
                let (xi, ti) = (room.index[usize::from(x)], room.index[usize::from(t)]);
                if xi != NOT {
                    pushes.push((u, (xi, room.node(t) as u8)));
                } else if ti != NOT {
                    // `x` is outside Q and next to it, so it is a door.
                    entries.push((u, ti));
                }
            }
        }
        let (n, nodes) = (room.cells.len(), room.nodes());
        let pulls = pushes.iter().map(|&(u, (x, ti))| (x, (u, ti))).collect();
        let mut unentries = entries.clone();
        unentries.sort_unstable();
        unentries.dedup();
        Self {
            steps: Lists::new(nodes, steps),
            pushes: Lists::new(nodes, pushes),
            entries: Lists::new(nodes, entries),
            pulls: Lists::new(n, pulls),
            unentries,
        }
    }

    pub(super) fn steps(&self, u: usize) -> &[u8] {
        self.steps.of(u)
    }

    pub(super) fn pushes(&self, u: usize) -> &[(u8, u8)] {
        self.pushes.of(u)
    }

    pub(super) fn entries(&self, u: usize) -> &[u8] {
        self.entries.of(u)
    }

    pub(super) fn pulls(&self, w: usize) -> &[(u8, u8)] {
        self.pulls.of(w)
    }

    pub(super) fn unentries(&self) -> &[(u8, u8)] {
        &self.unentries
    }

    /// The heap bytes the lists hold.
    pub(super) fn bytes(&self) -> usize {
        self.steps.bytes()
            + self.pushes.bytes()
            + self.entries.bytes()
            + self.pulls.bytes()
            + self.unentries.capacity() * size_of::<(u8, u8)>()
    }
}
