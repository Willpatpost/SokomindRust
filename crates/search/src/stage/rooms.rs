//! Rooms, misplaced boxes and levels. A room is a region of floor behind
//! one cell, its gate. A box is misplaced when it sits in a room that, in
//! the start position, holds more boxes of the box's group than goals for
//! that group: some box of the group must leave through the gate. A state's
//! level is its boxes on goals of their own label minus its misplaced boxes
//! off goals. A stage looks for a rise, a state of a higher level than its
//! frame root. Ports the probe's `analyze_rooms`, `level`, `misplaced` and
//! `sig` (p4b.rs 563-639 and 668-699).
//!
//! A candidate room is a side of a floor cell, its gate: a part the cell's
//! floor component falls into once the gate is blocked. The side must hold
//! a box of the start position or a goal of any label, have at least 4
//! cells, and have at most 72% of the floor; the gate is not counted in
//! either size. The candidates are taken largest first, then by gate cell,
//! then by side label. Each one whose cells, gate included, overlap no room
//! taken earlier becomes the next room, up to 254 rooms. Side sizes come
//! from Blocks' DFS spans, so no cell is flooded.
use crate::{heuristic::Heuristic, reach::Blocks};
use sokomind_core::{Board, Cell, MAX_CELLS, State, WALL};
use std::{collections::TryReserveError, mem::size_of, ops::Range};

/// The fewest cells a candidate side has, its gate not counted.
const MIN_SIZE: usize = 4;
/// The largest share of the floor, in percent, a candidate side has, its
/// gate not counted.
const MAX_SHARE: usize = 72;
/// The most rooms a board has, so a room number fits a `u8` with 0 left
/// for no room.
const MAX_ROOMS: u8 = 254;

/// The rooms of a board's start position; see the module docs.
pub(super) struct Rooms {
    /// Each cell's room, numbered 1 to 254, or 0 outside every room.
    room_of: Vec<u8>,
    /// For room `r`, entry `r - 1` has bit `heuristic.group(i).start` set
    /// when the room starts with more boxes of box `i`'s group than goals
    /// of that group.
    surplus: Vec<u32>,
}

impl Rooms {
    /// Heap bytes `build` reserves on a board of `cells` cells: a room per
    /// cell and a surplus mask per possible room.
    pub(super) const fn bytes_for(cells: usize) -> usize {
        // A u8 widens to usize; `usize::from` is not const.
        cells + MAX_ROOMS as usize * size_of::<u32>()
    }
    /// Heap bytes these vectors hold, which tests compare with
    /// [`Self::bytes_for`].
    #[cfg(test)]
    pub(super) fn heap_bytes(&self) -> usize {
        self.room_of.capacity() + self.surplus.capacity() * size_of::<u32>()
    }
    /// Finds the rooms of `board.initial()`. Each candidate side gets the
    /// sort key `((MAX_CELLS - size) << 14) | (gate << 2) | label`. A cell
    /// has as many sides as blocks it lies in, so the keys number below
    /// `2 * cells`. The keys are unique, so an unstable sort gives the
    /// probe's stable order. `taken`, a prefix sum over DFS order, is
    /// rebuilt after each room is taken and tests a candidate for overlap
    /// in `O(1)` per span.
    ///
    /// Scratch, left holding garbage:
    /// - `keys`: at least `2 * cells` entries.
    /// - `order`: at least `cells` entries, each floor cell at its entry
    ///   time.
    /// - `prefix`: at least `cells + 1` entries, the count of cells holding
    ///   a start box or a goal before each entry time, then each entry
    ///   time's component root.
    /// - `taken`: at least `cells + 1` entries.
    ///
    /// The two vectors are the only allocations, [`Self::bytes_for`] bytes
    /// in all. Each is reserved once, at a size that never grows.
    pub(super) fn build(
        board: &Board,
        heuristic: &Heuristic,
        blocks: &Blocks,
        keys: &mut [u32],
        order: &mut [u16],
        prefix: &mut [u16],
        taken: &mut [u16],
    ) -> Result<Self, TryReserveError> {
        let tiles = board.tiles();
        let start = board.initial();
        let boxes = &start.boxes[..board.labels().len()];
        let mut room_of = super::filled(tiles.len(), 0u8)?;
        let mut floor = 0;
        for (c, &tile) in tiles.iter().enumerate() {
            if tile != WALL {
                order[usize::from(blocks.span(c as Cell).start)] = c as Cell;
                floor += 1;
            }
        }
        prefix[0] = 0;
        for (t, &c) in order[..floor].iter().enumerate() {
            let marked = tiles[usize::from(c)] != 0 || boxes.contains(&c);
            prefix[t + 1] = prefix[t] + u16::from(marked);
        }
        // Entry times run through each component from its root, whose span
        // is the whole component, so the component of `t` starts at the
        // last root at or before it.
        let mut count = 0;
        let (mut first, mut end) = (0, 0);
        for t in 0..floor {
            let gate = order[t];
            if t == end {
                first = t;
                end = usize::from(blocks.span(gate).end);
            }
            let labels = blocks.labels(gate);
            if labels.count_ones() < 2 {
                continue;
            }
            let own = prefix[t + 1] - prefix[t];
            for label in (0..4).filter(|&label| (labels >> label) & 1 != 0) {
                let ranges = side(blocks, gate, label, first..end);
                // The gate lies in one range: take it out of both counts.
                let (mut size, mut marked) = (0, 0);
                for &(a, b) in &ranges {
                    size += b - a;
                    marked += prefix[b] - prefix[a];
                }
                let (size, marked) = (size - 1, marked - own);
                if size >= MIN_SIZE && size * 100 <= floor * MAX_SHARE && marked > 0 {
                    keys[count] = key(size, gate, label);
                    count += 1;
                }
            }
        }
        keys[..count].sort_unstable();
        // From here on `prefix` holds each entry time's component root.
        let mut root = 0;
        while root < floor {
            let next = usize::from(blocks.span(order[root]).end);
            prefix[root..next].fill(root as u16);
            root = next;
        }
        taken[..=floor].fill(0);
        let mut rooms = 0;
        for &packed in &keys[..count] {
            if rooms == MAX_ROOMS {
                break;
            }
            let gate = ((packed >> 2) & 0xfff) as Cell;
            let label = (packed & 3) as u8;
            let first = usize::from(prefix[usize::from(blocks.span(gate).start)]);
            let end = usize::from(blocks.span(order[first]).end);
            let ranges = side(blocks, gate, label, first..end);
            if ranges.iter().any(|&(a, b)| taken[a] != taken[b]) {
                continue;
            }
            rooms += 1;
            for &(a, b) in &ranges {
                for &c in &order[a..b] {
                    room_of[usize::from(c)] = rooms;
                }
            }
            for t in 0..floor {
                taken[t + 1] = taken[t] + u16::from(room_of[usize::from(order[t])] != 0);
            }
        }
        // Reserved at the most rooms a board has, so the bytes do not depend
        // on the board (see `bytes_for`).
        let mut surplus = Vec::new();
        surplus.try_reserve_exact(usize::from(MAX_ROOMS))?;
        surplus.resize(usize::from(rooms), 0u32);
        let goal_cells = heuristic.goal_cells();
        let mut i = 0;
        while i < boxes.len() {
            let group = heuristic.group(i);
            let bit = 1u32 << group.start;
            i = group.end;
            // Each room's boxes minus goals of this group, room 0 included.
            let mut net = [0i8; 256];
            for (&b, &q) in boxes[group.clone()].iter().zip(&goal_cells[group]) {
                net[usize::from(room_of[usize::from(b)])] += 1;
                net[usize::from(room_of[usize::from(q)])] -= 1;
            }
            for (mask, &n) in surplus.iter_mut().zip(&net[1..]) {
                if n > 0 {
                    *mask |= bit;
                }
            }
        }
        Ok(Self { room_of, surplus })
    }
    /// Cell `c`'s room, or 0 outside every room.
    pub(super) fn room(&self, c: Cell) -> u8 {
        self.room_of[usize::from(c)]
    }
    /// Whether box `i` on cell `c` is in a room whose surplus holds its
    /// group. A box on a goal of its own label counts here too; `level` and
    /// `sig` leave it out.
    pub(super) fn misplaced(&self, heuristic: &Heuristic, i: usize, c: Cell) -> bool {
        let room = usize::from(self.room(c));
        room > 0 && (self.surplus[room - 1] >> heuristic.group(i).start) & 1 != 0
    }
    /// `state`'s level: the boxes on goals of their own label minus the
    /// misplaced boxes off goals. It lies in `-MAX_BOXES..=MAX_BOXES`.
    /// `O(boxes)`.
    pub(super) fn level(&self, board: &Board, heuristic: &Heuristic, state: &State) -> i8 {
        let mut level = 0;
        for (i, &c) in state.boxes[..board.labels().len()].iter().enumerate() {
            if board.on_goal(i, c) {
                level += 1;
            } else if self.misplaced(heuristic, i, c) {
                level -= 1;
            }
        }
        level
    }
    /// What a mask ban matches: the filled goals, with bit `t` set when goal
    /// column `t` of `heuristic.goal_cells()` holds a box of its group, and
    /// the count of misplaced boxes off goals. The probe numbers the goal
    /// bits in board goal order instead, so its test remaps them.
    pub(super) fn sig(&self, board: &Board, heuristic: &Heuristic, state: &State) -> (u32, u8) {
        let goal_cells = heuristic.goal_cells();
        let (mut filled, mut misplaced) = (0, 0);
        for (i, &c) in state.boxes[..board.labels().len()].iter().enumerate() {
            if board.on_goal(i, c) {
                // A goal of box `i`'s label is one of its group's columns.
                let columns = heuristic.group(i);
                let start = columns.start;
                if let Some(k) = goal_cells[columns].iter().position(|&q| q == c) {
                    filled |= 1 << (start + k);
                }
            } else if self.misplaced(heuristic, i, c) {
                misplaced += 1;
            }
        }
        (filled, misplaced)
    }
}

/// The side `label` of floor cell `gate`, with `gate` itself, as at most
/// four ranges of entry times; unused entries are empty. `component` is
/// the entry times of `gate`'s floor component. A separated side is its
/// span, plus the gate's own entry time. The rest side is the component
/// with every separated span cut out, which leaves the gate in; a cell off
/// a root has at most three separated sides, so four gaps suffice.
fn side(blocks: &Blocks, gate: Cell, label: u8, component: Range<usize>) -> [(usize, usize); 4] {
    let mut ranges = [(0, 0); 4];
    if let Some((_, span)) = blocks.separated(gate).find(|&(l, _)| l == label) {
        let tin = usize::from(blocks.span(gate).start);
        ranges[0] = (usize::from(span.start), usize::from(span.end));
        ranges[1] = (tin, tin + 1);
        return ranges;
    }
    debug_assert_eq!(blocks.rest(gate), Some(label));
    let mut separated = blocks.separated(gate);
    let mut from = component.start;
    for range in &mut ranges {
        match separated.next() {
            Some((_, span)) => {
                *range = (from, usize::from(span.start));
                from = usize::from(span.end);
            }
            None => {
                *range = (from, component.end);
                break;
            }
        }
    }
    ranges
}

/// A candidate side's sort key: larger sides first, then lower gate cells,
/// then lower labels. The gate takes 12 bits, as stage.rs asserts.
fn key(size: usize, gate: Cell, label: u8) -> u32 {
    (((MAX_CELLS - size) as u32) << 14) | (u32::from(gate) << 2) | u32::from(label)
}

#[cfg(test)]
mod tests {
    use crate::{
        heuristic::Heuristic,
        stage::reference::{FAR, Geo, Owned, at, random_state},
        testkit::{Lcg, catalog},
    };
    use sokomind_core::{Board, Cell, NONE, State, WALL};

    /// A built board: blocking (2,4) cuts the room (2,5) to the right off
    /// from the 11 cells left of it, which hold live floor for the `A` box
    /// but no box and no goal, so they get no room.
    const SIDE: &str = "OOOOOOOOOO\nO    O   O\nO     Aa O\nO    O R O\nOOOOOOOOOO";

    /// The probe's rooms of a board.
    struct ProbeRooms {
        /// Each cell's room, or 0.
        room_of: Vec<u8>,
        /// Each room's surplus, bit `g` for the probe's group `g`.
        surplus: Vec<u32>,
        /// Each room's gate and size, the gate included.
        gates: Vec<(Cell, usize)>,
    }

    /// A port of the probe's `analyze_rooms` (p4b.rs 563-635), with its
    /// floods from `Geo::flood`.
    fn probe_rooms(geo: &Geo<'_>) -> ProbeRooms {
        let n = geo.nb.len();
        let tiles = geo.board.tiles();
        let floor = tiles.iter().filter(|&&tile| tile != WALL).count();
        let start = geo.board.initial();
        let mut marked: Vec<bool> = geo.goal_group.iter().map(|&g| g != usize::MAX).collect();
        for &c in &start.boxes[..geo.slots] {
            marked[usize::from(c)] = true;
        }
        let mut cands: Vec<(usize, Cell, Vec<bool>)> = Vec::new();
        let mut blocked = vec![false; n];
        for c in 0..n {
            let around: Vec<Cell> = geo.nb[c].iter().copied().filter(|&y| y != NONE).collect();
            if tiles[c] == WALL || around.len() < 2 {
                continue;
            }
            blocked[c] = true;
            let mut covered = vec![false; n];
            let mut sides = Vec::new();
            for &m in &around {
                if covered[usize::from(m)] {
                    continue;
                }
                let cells: Vec<bool> = geo.flood(m, &blocked).iter().map(|&d| d != FAR).collect();
                for (seen, &cell) in covered.iter_mut().zip(&cells) {
                    *seen |= cell;
                }
                sides.push(cells);
            }
            blocked[c] = false;
            if sides.len() < 2 {
                continue;
            }
            for cells in sides {
                let size = cells.iter().filter(|&&cell| cell).count();
                let useful = (0..n).any(|i| cells[i] && marked[i]);
                if size >= 4 && size * 100 <= floor * 72 && useful {
                    cands.push((size, c as Cell, cells));
                }
            }
        }
        cands.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut room_of = vec![0u8; n];
        let mut gates = Vec::new();
        for (size, gate, mut cells) in cands {
            cells[usize::from(gate)] = true;
            if (0..n).any(|i| cells[i] && room_of[i] != 0) || gates.len() == 254 {
                continue;
            }
            gates.push((gate, size + 1));
            for (room, &cell) in room_of.iter_mut().zip(&cells) {
                if cell {
                    *room = gates.len() as u8;
                }
            }
        }
        // Each room's boxes minus goals per group, room 0 included.
        let mut net = vec![vec![0i32; geo.groups.len()]; gates.len() + 1];
        for (i, &c) in start.boxes[..geo.slots].iter().enumerate() {
            net[usize::from(room_of[usize::from(c)])][geo.group[i]] += 1;
        }
        for &(q, g) in &geo.goals {
            net[usize::from(room_of[usize::from(q)])][g] -= 1;
        }
        let mut surplus = vec![0; gates.len()];
        for (mask, row) in surplus.iter_mut().zip(&net[1..]) {
            for (g, &count) in row.iter().enumerate() {
                if count > 0 {
                    *mask |= 1 << g;
                }
            }
        }
        ProbeRooms {
            room_of,
            surplus,
            gates,
        }
    }

    /// The probe's `misplaced` (p4b.rs 682-685).
    fn probe_misplaced(geo: &Geo<'_>, probe: &ProbeRooms, i: usize, c: Cell) -> bool {
        let r = usize::from(probe.room_of[usize::from(c)]);
        r > 0 && (probe.surplus[r - 1] >> geo.group[i]) & 1 == 1
    }

    /// The probe's `level` (p4b.rs 668-679).
    fn probe_level(geo: &Geo<'_>, probe: &ProbeRooms, s: &State) -> i32 {
        let mut level = 0;
        for (i, &c) in s.boxes[..geo.slots].iter().enumerate() {
            if geo.board.on_goal(i, c) {
                level += 1;
            } else if probe_misplaced(geo, probe, i, c) {
                level -= 1;
            }
        }
        level
    }

    /// The probe's `sig` (p4b.rs 688-697): goal bit `k` for goal `k` of
    /// `board.goals()`, and the misplaced boxes off goals.
    fn probe_sig(geo: &Geo<'_>, probe: &ProbeRooms, s: &State) -> (u64, u32) {
        let occ = geo.occ(s);
        let mut mask = 0;
        for (k, &(q, g)) in geo.goals.iter().enumerate() {
            if geo.filled(q, g, &occ) {
                mask |= 1 << k;
            }
        }
        let mut count = 0;
        for (i, &c) in s.boxes[..geo.slots].iter().enumerate() {
            if !geo.board.on_goal(i, c) && probe_misplaced(geo, probe, i, c) {
                count += 1;
            }
        }
        (mask, count)
    }

    /// The probe's group mask `mask` with group `g`'s bit moved to bit
    /// `heuristic.group(i).start` of each slot `i` of the group.
    fn remap(geo: &Geo<'_>, heuristic: &Heuristic, mask: u32) -> u32 {
        let mut out = 0;
        for (i, &g) in geo.group.iter().enumerate() {
            if (mask >> g) & 1 != 0 {
                out |= 1 << heuristic.group(i).start;
            }
        }
        out
    }

    /// The probe's goal mask `mask` with goal `k`'s bit moved to the
    /// goal's column in `heuristic.goal_cells()`.
    fn remap_goals(geo: &Geo<'_>, heuristic: &Heuristic, mask: u64) -> u32 {
        let goal_cells = heuristic.goal_cells();
        let mut out = 0;
        for (k, &(q, _)) in geo.goals.iter().enumerate() {
            if (mask >> k) & 1 != 0 {
                let t = goal_cells.iter().position(|&c| c == q).unwrap();
                out |= 1 << t;
            }
        }
        out
    }

    /// On every catalog board and [`SIDE`], `room_of` and `surplus` equal
    /// [`probe_rooms`], room numbers and cells included. Huge has one room,
    /// gate (10,7) and 28 cells with the gate, on a floor of 127. On SIDE
    /// the room of gate (2,4) takes the box side, and the 11 live cells on
    /// the other side, with no box and no goal, stay outside every room.
    #[test]
    fn rooms_match_probe() {
        let side = Board::parse(SIDE).unwrap();
        let boards = catalog().into_iter().chain([("side".to_owned(), side)]);
        for (id, board) in boards {
            let owned = Owned::new(&board);
            let (heuristic, rooms) = (&owned.heuristic, &owned.facts.rooms);
            let geo = Geo::new(&board, heuristic);
            let probe = probe_rooms(&geo);
            assert_eq!(rooms.room_of, probe.room_of, "{id}");
            let remapped = probe.surplus.iter().map(|&m| remap(&geo, heuristic, m));
            assert_eq!(rooms.surplus, remapped.collect::<Vec<_>>(), "{id}");
            let cells = |room: u8| rooms.room_of.iter().filter(|&&r| r == room).count();
            if id == "huge" {
                let floor = board.tiles().iter().filter(|&&tile| tile != WALL).count();
                assert_eq!(floor, 127);
                assert_eq!(probe.gates, [(at(&board, 10, 7), 28)]);
                assert_eq!((rooms.surplus.len(), cells(1)), (1, 28));
            }
            if id == "side" {
                assert_eq!(probe.gates, [(at(&board, 2, 4), 11)]);
                assert_eq!((rooms.surplus.len(), cells(1)), (1, 11));
                assert_eq!(rooms.surplus, [0]);
                assert!(!heuristic.dead(0, at(&board, 2, 2)));
                for (c, room) in [(4, 1), (5, 1), (3, 0), (1, 0)] {
                    assert_eq!(rooms.room(at(&board, 2, c)), room, "(2,{c})");
                }
            }
        }
    }

    /// On 40 random states of each catalog board, `level` and `sig` equal
    /// [`probe_level`] and [`probe_sig`], with the probe's goal bits
    /// remapped to goal columns. Some state must have a misplaced box off
    /// goals.
    #[test]
    fn level_and_sig_match_probe() {
        let mut rng = Lcg(0x5e7a);
        let mut misplaced = 0;
        for (id, board) in catalog() {
            let owned = Owned::new(&board);
            let (heuristic, rooms) = (&owned.heuristic, &owned.facts.rooms);
            let geo = Geo::new(&board, heuristic);
            let probe = probe_rooms(&geo);
            for _ in 0..40 {
                let state = random_state(&mut rng, &board, heuristic);
                let level = rooms.level(&board, heuristic, &state);
                assert_eq!(i32::from(level), probe_level(&geo, &probe, &state), "{id}");
                let (filled, count) = rooms.sig(&board, heuristic, &state);
                let (mask, expected) = probe_sig(&geo, &probe, &state);
                assert_eq!(filled, remap_goals(&geo, heuristic, mask), "{id}");
                assert_eq!(u32::from(count), expected, "{id}");
                misplaced += u32::from(count);
            }
        }
        assert!(misplaced > 0);
    }
}
