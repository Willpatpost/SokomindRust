use crate::{
    MAX_STATES, MAX_STATES_RANGE, MEMORY_MIB_RANGE, SearchError, SearchStats, Status,
    deadlock::Deadlock, heuristic::Heuristic, reach::Reach,
};
use sokomind_core::{Cell, MAX_BOXES, MAX_CELLS, MAX_ROUTE, NONE, State};
use std::{cmp::Reverse, collections::BinaryHeap, mem::size_of};

/// No node: an empty table slot, or the root's parent.
pub(crate) const NIL: u32 = u32::MAX;

/// Bits of a node id in a queue key and in a record's parent field.
const ID_BITS: u32 = 20;
/// Low `ID_BITS` set: extracts an id, and stands for NIL in a record.
const ID_MASK: u32 = (1 << ID_BITS) - 1;
// Every id, the spare node's included, is at most MAX_STATES, so it fits
// both fields with the all-ones value left over for a record's NIL parent.
const _: () = assert!(MAX_STATES < ID_MASK as usize);
// A node's depth is at most its id, and one push adds at most MAX_CELLS
// moves (a walk shorter than the board, then the push), so a full u32 g
// never overflows and is never narrowed.
const _: () = assert!(MAX_STATES as u64 * MAX_CELLS as u64 <= u32::MAX as u64);

/// Largest h any estimate may queue: MAX_BOXES push distances over (cell,
/// keeper side) pairs, each below `4 * MAX_CELLS`, plus one keeper walk
/// below `MAX_CELLS`. Today's estimates stay below
/// `(MAX_BOXES + 1) * MAX_CELLS`; the rest is headroom for side-aware
/// distances and a walk term at every node.
pub(crate) const MAX_QUEUED_H: u32 = ((4 * MAX_BOXES + 1) * MAX_CELLS) as u32;

#[derive(Clone, Copy)]
pub(crate) struct Node {
    pub state: State,
    pub g: u32,
    pub parent: u32,
    /// Direction of the push that made this node (unused at the root); the
    /// pushed box started at `state.player`.
    pub direction: u8,
    /// The state's estimate, or `u16::MAX` when it is unknown or too large
    /// and must be recomputed. Estimates depend only on the canonical
    /// state, so a cheaper duplicate reuses it. This stack value is decoded
    /// from compact storage only when an expansion or replay needs it.
    pub h: u16,
}

/// A stored node without its state: all a duplicate probe or an incumbent
/// check reads, at the cost of one record load.
#[derive(Clone, Copy)]
pub(crate) struct Meta {
    pub g: u32,
    /// As in [`Node::h`].
    pub h: u16,
    /// Expanded at least once ([`Arena::close`]).
    pub closed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Region keys (5.5 O6): a probe matches a node with the same boxes
    /// only through the region test, exact lookups still find live nodes
    /// and only those, and re-keying by state keeps the live nodes, their
    /// flags and the stats.
    #[cfg(feature = "o6")]
    #[test]
    fn region_keys_find_by_membership_and_rekey_by_state() {
        let mut arena = Arena::new(100, 2, 10, 4).unwrap();
        arena.key_by_region(true);
        let at = |player: Cell| State {
            player,
            boxes: {
                let mut boxes = [NONE; MAX_BOXES];
                boxes[..2].copy_from_slice(&[20, 30]);
                boxes
            },
        };
        let node = |player, g| Node {
            state: at(player),
            g,
            parent: NIL,
            direction: 0,
            h: 5,
        };
        // Keepers 1 and 2 share a region, keeper 9 is apart.
        let region = |a: Cell, b: Cell| (a < 9) == (b < 9);
        let (slot, found) = arena.find_region(&at(1), |keeper| region(keeper, 1));
        assert_eq!(found, None);
        let first = arena.insert(node(1, 10), slot);
        let (slot, found) = arena.find_region(&at(9), |keeper| region(keeper, 9));
        assert_eq!(found, None);
        let apart = arena.insert(node(9, 10), slot);
        let (slot, found) = arena.find_region(&at(2), |keeper| region(keeper, 2));
        assert_eq!(found, Some(first));
        assert_eq!(arena.find(&at(1)).1, Some(first));
        assert_eq!(arena.find(&at(2)).1, None);
        // A cheaper keeper of the region replaces the first node.
        let better = arena.insert(node(2, 8), slot);
        assert!(arena.is_superseded(first) && !arena.is_superseded(better));
        assert_eq!(arena.find(&at(1)).1, None);
        assert_eq!(arena.find(&at(2)).1, Some(better));
        assert_eq!(
            arena.find_region(&at(1), |keeper| region(keeper, 1)).1,
            Some(better)
        );
        assert_eq!(
            arena.find_region(&at(9), |keeper| region(keeper, 9)).1,
            Some(apart)
        );
        arena.key_by_state();
        assert!(!arena.keeper_regions());
        assert_eq!(arena.find(&at(2)).1, Some(better));
        assert_eq!(arena.find(&at(9)).1, Some(apart));
        assert_eq!(arena.find(&at(1)).1, None);
        assert!(arena.is_superseded(first));
        assert_eq!(arena.node(first).state, at(1));
        assert_eq!(
            (
                arena.stats.unique_states,
                arena.stats.duplicate_improvements
            ),
            (2, 1)
        );
    }

    #[test]
    fn compact_versions_preserve_parents_and_exact_keeper_identity() {
        for count in [1, 8, MAX_BOXES] {
            let mut arena = Arena::new(100, count, 10, 4).unwrap();
            let mut state = State {
                player: 1,
                boxes: [NONE; MAX_BOXES],
            };
            for (i, cell) in state.boxes[..count].iter_mut().enumerate() {
                *cell = i as Cell + 10;
            }
            let (slot, _) = arena.find(&state);
            let old = arena.insert(
                Node {
                    state,
                    g: 10,
                    parent: NIL,
                    direction: 0,
                    h: 3,
                },
                slot,
            );
            arena.close(old);
            let mut child = state;
            child.player = 2;
            let (slot, found) = arena.find(&child);
            assert_eq!(found, None);
            let descendant = arena.insert(
                Node {
                    state: child,
                    g: 11,
                    parent: old,
                    direction: 1,
                    h: 2,
                },
                slot,
            );
            let (slot, _) = arena.find(&state);
            let improved = arena.insert(
                Node {
                    state,
                    g: 8,
                    parent: NIL,
                    direction: 0,
                    h: 3,
                },
                slot,
            );
            assert_eq!(arena.find(&state).1, Some(improved));
            assert_eq!(arena.find(&child).1, Some(descendant));
            assert_eq!(arena.node(descendant).parent, old);
            assert_eq!(arena.node(descendant).direction, 1);
            assert_eq!(arena.node(improved).parent, NIL);
            assert_eq!(arena.node(old).g, 10);
            assert_eq!(arena.node(old).state, state);
            assert_eq!(arena.node(improved).g, 8);
            // Only the replaced version is stale, and closing is per version.
            let flags = |id| (arena.is_superseded(id), arena.meta(id).closed);
            assert_eq!(flags(old), (true, true));
            assert_eq!(flags(improved), (false, false));
            assert_eq!(flags(descendant), (false, false));
            assert_eq!(arena.stats.unique_states, 2);
            assert_eq!(arena.stats.duplicate_improvements, 1);
            assert_eq!(arena.stats.reopened_states, 1);
        }
    }

    #[test]
    fn a_budget_under_the_fixed_buffers_is_its_own_error() {
        // The per-cell buffers of a million cells alone overflow 4 MiB.
        assert_eq!(
            Arena::new(1 << 20, 1, 10, 4).err(),
            Some(SearchError::BudgetTooSmall)
        );
        assert_eq!(Arena::new(100, 1, 0, 4).err(), Some(SearchError::Limits));
    }

    #[test]
    fn reweight_orders_the_queue_as_if_queued_at_the_new_weight() {
        let mut arena = Arena::new(100, 1, 10, 4).unwrap();
        // (g, h) per id: the order at weight 5 differs from the order at 3.
        // Re-keying reads each g from its record, so the nodes are real.
        let queued: [(u32, u32); 6] = [(0, 4), (9, 1), (4, 3), (12, 0), (7, 2), (1, 4)];
        for (i, &(g, h)) in queued.iter().enumerate() {
            let mut state = State {
                player: 1,
                boxes: [NONE; MAX_BOXES],
            };
            state.boxes[0] = 10 + i as Cell;
            let (slot, _) = arena.find(&state);
            let node = Node {
                state,
                g,
                parent: NIL,
                direction: 0,
                h: Node::store_h(h),
            };
            let id = arena.insert(node, slot);
            arena.enqueue(u64::from(g + 5 * h), h, id);
        }
        let capacity = arena.heap.capacity();
        arena.reweight(5, 3);
        assert_eq!(arena.heap.capacity(), capacity);
        let mut expected: Vec<_> = queued
            .iter()
            .enumerate()
            .map(|(id, &(g, h))| (u64::from(g + 3 * h), h, id as u32))
            .collect();
        expected.sort();
        assert_eq!(arena.min_f(), Some(expected[0].0));
        let order: Vec<_> = std::iter::from_fn(|| arena.dequeue())
            .map(|key| (key.id(), key.h()))
            .collect();
        let wanted: Vec<_> = expected.iter().map(|&(_, h, id)| (id, h)).collect();
        assert_eq!(order, wanted);
        assert_eq!(arena.stats.peak_queue, queued.len() as u32);
    }

    #[test]
    fn clear_forgets_every_state_and_keeps_the_reservation() {
        let mut arena = Arena::new(100, 1, 10, 4).unwrap();
        let mut state = State {
            player: 1,
            boxes: [NONE; MAX_BOXES],
        };
        state.boxes[0] = 10;
        let node = Node {
            state,
            g: 0,
            parent: NIL,
            direction: 0,
            h: 1,
        };
        let (slot, _) = arena.find(&state);
        let id = arena.insert(node, slot);
        arena.enqueue(1, 1, id);
        let reserved = |arena: &Arena| {
            (
                arena.nodes.capacity(),
                arena.box_cells.capacity(),
                arena.heap.capacity(),
                arena.table.len(),
                arena.reserved_bytes(),
            )
        };
        let before = reserved(&arena);
        arena.clear();
        assert_eq!(reserved(&arena), before);
        assert_eq!((arena.len(), arena.dequeue()), (0, None));
        let (slot, found) = arena.find(&state);
        assert_eq!(found, None);
        // Ids restart at 0; the stats keep counting.
        assert_eq!(arena.insert(node, slot), 0);
        assert_eq!(arena.stats.unique_states, 2);
    }

    /// Every record field at its extremes, with every flag combination.
    #[test]
    fn records_round_trip_every_field_at_its_extremes() {
        let flags = [
            0,
            Record::CLOSED,
            Record::SUPERSEDED,
            Record::CLOSED | Record::SUPERSEDED,
        ];
        let last_cell = (MAX_CELLS - 1) as Cell;
        // Straight through the packing, for ids no small arena reaches.
        for parent in [NIL, 0, 1, MAX_STATES as u32, ID_MASK - 1] {
            for direction in 0..4 {
                for (g, h) in [(0, 0), (1, u16::MAX - 1), (u32::MAX, u16::MAX)] {
                    for player in [0, last_cell] {
                        let node = Node {
                            state: State {
                                player,
                                boxes: [NONE; MAX_BOXES],
                            },
                            g,
                            parent,
                            direction,
                            h,
                        };
                        for &set in &flags {
                            let mut record = Record::new(&node);
                            assert_eq!(record.link & (Record::CLOSED | Record::SUPERSEDED), 0);
                            record.link |= set;
                            assert_eq!(
                                (record.g, record.parent(), record.direction()),
                                (g, parent, direction)
                            );
                            assert_eq!((record.player, record.h), (player, h));
                            assert_eq!(record.has(Record::CLOSED), set & Record::CLOSED != 0);
                            assert_eq!(
                                record.has(Record::SUPERSEDED),
                                set & Record::SUPERSEDED != 0
                            );
                        }
                    }
                }
            }
        }
        // Through the arena API, with every box cell at an extreme.
        let mut arena = Arena::new(MAX_CELLS, MAX_BOXES, 4, 4).unwrap();
        let mut state = State {
            player: last_cell,
            boxes: [NONE; MAX_BOXES],
        };
        for (i, cell) in state.boxes.iter_mut().enumerate() {
            *cell = if i.is_multiple_of(2) {
                i as Cell
            } else {
                last_cell - i as Cell
            };
        }
        let mut child = state;
        child.player = 0;
        let root = Node {
            state,
            g: u32::MAX,
            parent: NIL,
            direction: 3,
            h: u16::MAX,
        };
        let (slot, _) = arena.find(&state);
        let first = arena.insert(root, slot);
        arena.close(first);
        let (slot, _) = arena.find(&child);
        let leaf = Node {
            state: child,
            g: 0,
            parent: first,
            direction: 0,
            h: 0,
        };
        let second = arena.insert(leaf, slot);
        let (slot, _) = arena.find(&state);
        let improved = Node {
            g: u32::MAX - 1,
            h: u16::MAX - 1,
            ..root
        };
        let third = arena.insert(improved, slot);
        for (id, node, closed, superseded) in [
            (first, root, true, true),
            (second, leaf, false, false),
            (third, improved, false, false),
        ] {
            let stored = arena.node(id);
            assert_eq!(stored.state, node.state);
            assert_eq!(
                (stored.g, stored.parent, stored.direction, stored.h),
                (node.g, node.parent, node.direction, node.h)
            );
            let meta = arena.meta(id);
            assert_eq!((meta.g, meta.h, meta.closed), (node.g, node.h, closed));
            assert_eq!(meta.known_h(), node.known_h());
            assert_eq!(arena.is_superseded(id), superseded);
        }
        assert_eq!(arena.meta(first).known_h(), None);
        assert_eq!(arena.meta(third).known_h(), Some(u32::from(u16::MAX - 1)));
    }

    /// Key order is `(f, h, id)` order over every value that can be queued,
    /// and saturation keeps each field in range and after every exact f.
    #[test]
    fn keys_order_like_tuples_and_saturate_safely() {
        let fs = [0, 1, MAX_ROUTE as u64, Key::F_SAT - 1, Key::F_SAT];
        let hs = [0, 1, MAX_QUEUED_H - 1, MAX_QUEUED_H, Key::H_SAT];
        let ids = [0, 1, MAX_STATES as u32 - 1, MAX_STATES as u32, ID_MASK - 1];
        let mut keys = Vec::new();
        for f in fs {
            for h in hs {
                for id in ids {
                    let key = Key::new(f, h, id);
                    assert_eq!((key.f(), key.h(), key.id()), (f, h, id));
                    keys.push(((f, h, id), key));
                }
            }
        }
        for (a, key_a) in &keys {
            for (b, key_b) in &keys {
                assert_eq!(key_a.cmp(key_b), a.cmp(b), "{a:?} {b:?}");
            }
        }
        let last_exact = Key::new(Key::F_SAT - 1, Key::H_SAT, ID_MASK - 1);
        for (f, h, id) in [
            (Key::F_SAT + 1, 0, 0),
            (u64::MAX, MAX_QUEUED_H, 1),
            (u64::MAX, u32::MAX, ID_MASK - 1),
        ] {
            let key = Key::new(f, h, id);
            assert_eq!(
                (key.f(), key.h(), key.id()),
                (Key::F_SAT, h.min(Key::H_SAT), id)
            );
            assert!(key > last_exact);
        }
    }
}
impl Node {
    /// The estimate as stored in [`Node::h`].
    pub(crate) fn store_h(h: u32) -> u16 {
        u16::try_from(h).unwrap_or(u16::MAX)
    }
    /// The stored estimate, if it fit.
    pub(crate) fn known_h(&self) -> Option<u32> {
        known(self.h)
    }
}
impl Meta {
    /// The stored estimate, if it fit.
    pub(crate) fn known_h(self) -> Option<u32> {
        known(self.h)
    }
}
fn known(h: u16) -> Option<u32> {
    (h < u16::MAX).then_some(u32::from(h))
}

/// Metadata is fixed-size; box cells live in an active-prefix column. Parent
/// indices always refer to immutable appended versions, never overwritten
/// states. Only the two flags ever change after the append.
#[derive(Clone, Copy)]
struct Record {
    g: u32,
    /// Parent id in bits 0..20 ([`ID_MASK`] at the root), bits 20..28
    /// spare and zero, the push direction in bits 28..30, then
    /// [`Record::CLOSED`] and [`Record::SUPERSEDED`].
    link: u32,
    player: Cell,
    h: u16,
}
// No padding and alignment 4 on wasm32 and 64-bit alike.
const _: () = assert!(size_of::<Record>() == 12 && ID_BITS <= Record::DIRECTION_SHIFT);
impl Record {
    const DIRECTION_SHIFT: u32 = 28;
    /// Expanded at least once.
    const CLOSED: u32 = 1 << 30;
    /// A later version of the state has replaced this one in the table, so
    /// its queued entry is stale.
    const SUPERSEDED: u32 = 1 << 31;
    /// A fresh, open version of `node`.
    fn new(node: &Node) -> Self {
        debug_assert!(node.direction < 4);
        debug_assert!(node.parent == NIL || node.parent < ID_MASK);
        let parent = if node.parent == NIL {
            ID_MASK
        } else {
            node.parent
        };
        Self {
            g: node.g,
            link: parent | (u32::from(node.direction) << Self::DIRECTION_SHIFT),
            player: node.state.player,
            h: node.h,
        }
    }
    fn parent(self) -> u32 {
        let parent = self.link & ID_MASK;
        if parent == ID_MASK { NIL } else { parent }
    }
    fn direction(self) -> u8 {
        ((self.link >> Self::DIRECTION_SHIFT) & 3) as u8
    }
    fn has(self, flag: u32) -> bool {
        self.link & flag != 0
    }
}

/// Queue entry `(f, h, id)` packed into one integer, f in the top 24 bits
/// and h and id in 20 bits each, so integer order is exactly tuple order:
/// lowest f first, then lowest h, then oldest. Values past a field
/// saturate; see [`Key::F_SAT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key(u64);
impl Key {
    const H_BITS: u32 = 20;
    const F_SHIFT: u32 = Self::H_BITS + ID_BITS;
    /// Largest stored f. A key reading `F_SAT` stands for any f at or above
    /// it: never more than the true f, so a frontier read from keys stays a
    /// lower bound, and only nodes past MAX_ROUTE can saturate (see the
    /// policy asserts in engine.rs).
    pub(crate) const F_SAT: u64 = u64::MAX >> Self::F_SHIFT;
    /// Largest stored h.
    const H_SAT: u32 = (1 << Self::H_BITS) - 1;
    pub(crate) fn new(f: u64, h: u32, id: u32) -> Self {
        debug_assert!(id < ID_MASK);
        let h = u64::from(h.min(Self::H_SAT));
        Self((f.min(Self::F_SAT) << Self::F_SHIFT) | (h << ID_BITS) | u64::from(id))
    }
    /// The queued f, exact below [`Key::F_SAT`].
    pub(crate) fn f(self) -> u64 {
        self.0 >> Self::F_SHIFT
    }
    /// The queued h, exact up to [`MAX_QUEUED_H`].
    pub(crate) fn h(self) -> u32 {
        ((self.0 >> ID_BITS) as u32) & Self::H_SAT
    }
    pub(crate) fn id(self) -> u32 {
        (self.0 as u32) & ID_MASK
    }
}
const _: () = assert!(MAX_QUEUED_H <= Key::H_SAT);

/// 8 bytes on wasm32 and 64-bit alike.
type Entry = Reverse<Key>;
const _: () = assert!(size_of::<Entry>() == 8);

fn hash(player: Cell, boxes: &[Cell]) -> usize {
    let mut h = (player as u64).wrapping_add(0x9e3779b97f4a7c15);
    for &cell in boxes {
        h ^= cell as u64;
        h = h.wrapping_mul(0x100000001b3);
        h ^= h >> 29;
    }
    (h ^ (h >> 32)) as usize
}

/// Node arena, priority queue, and open-addressed index table, reserved once.
/// The table stores arena indices, never duplicated box arrays.
pub(crate) struct Arena {
    nodes: Vec<Record>,
    box_cells: Vec<Cell>,
    stats: SearchStats,
    heap: BinaryHeap<Entry>,
    table: Vec<u32>,
    /// Boxes per state, the prefix of `State::boxes` that is hashed.
    boxes: usize,
    node_limit: usize,
    reserved_bytes: usize,
    scaled_down: bool,
    /// The table keys states by boxes and keeper region instead of exact
    /// state (5.5 O6): see [`Arena::find_region`].
    #[cfg(feature = "o6")]
    keeper_regions: bool,
}
impl Arena {
    pub(crate) fn new(
        cells: usize,
        boxes: usize,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, SearchError> {
        if !MAX_STATES_RANGE.contains(&max_states) || !MEMORY_MIB_RANGE.contains(&memory_mib) {
            return Err(SearchError::Limits);
        }
        // Flood and deadlock buffers and the heuristic tables per cell, plus
        // the route and its string.
        let per_cell =
            Reach::BYTES_PER_CELL + Deadlock::BYTES_PER_CELL + Heuristic::bytes_per_cell(boxes);
        let fixed_bytes = cells * per_cell + 2 * MAX_ROUTE + 64 * 1024;
        let budget = memory_mib * 1024 * 1024;
        // One spare node keeps a solution found at the exact limit reachable.
        let bytes_for = |count: usize| {
            fixed_bytes
                + (count + 1) * (size_of::<Record>() + boxes * size_of::<Cell>())
                // The queue holds at most one live entry per record, a
                // partial expansion's re-queue included (5.3 S2).
                + (count + 1) * size_of::<Entry>()
                + ((count + 1) * 2).next_power_of_two() * size_of::<u32>()
        };
        // Exact largest limit that fits the budget, instead of stepping down 10%.
        let mut low = 0;
        let mut high = max_states;
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            if bytes_for(mid) <= budget {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        if low == 0 {
            return Err(SearchError::BudgetTooSmall);
        }
        let limit = low;
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(limit + 1)
            .map_err(|_| SearchError::Allocation("node arena"))?;
        let mut box_cells = Vec::new();
        box_cells
            .try_reserve_exact((limit + 1) * boxes)
            .map_err(|_| SearchError::Allocation("box arena"))?;
        // Every enqueue follows an insert or replaces the entry just popped,
        // so heap.len() <= nodes.len() <= limit + 1 and this never grows.
        let mut heap = BinaryHeap::new();
        heap.try_reserve_exact(limit + 1)
            .map_err(|_| SearchError::Allocation("search queue"))?;
        let table_size = ((limit + 1) * 2).next_power_of_two();
        let mut table = Vec::new();
        table
            .try_reserve_exact(table_size)
            .map_err(|_| SearchError::Allocation("state table"))?;
        table.resize(table_size, NIL);
        Ok(Self {
            boxes,
            reserved_bytes: bytes_for(limit),
            scaled_down: limit < max_states,
            node_limit: limit,
            nodes,
            box_cells,
            stats: SearchStats::default(),
            heap,
            table,
            #[cfg(feature = "o6")]
            keeper_regions: false,
        })
    }
    pub(crate) fn limit_status(&self) -> Status {
        if self.scaled_down {
            Status::MemoryLimit
        } else {
            Status::StateLimit
        }
    }
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }
    pub(crate) fn is_full(&self) -> bool {
        self.nodes.len() >= self.node_limit
    }
    /// The state's node, if any, and the table slot that holds it or would
    /// hold a new node for it.
    pub(crate) fn find(&self, state: &State) -> (usize, Option<u32>) {
        let mask = self.table.len() - 1;
        // An arena keyed by keeper region hashes no keeper, since its probes
        // compare keepers by region (`find_region`), not by cell.
        #[cfg(feature = "o6")]
        let player = if self.keeper_regions {
            NONE
        } else {
            state.player
        };
        #[cfg(not(feature = "o6"))]
        let player = state.player;
        let mut slot = hash(player, &state.boxes[..self.boxes]) & mask;
        loop {
            let id = self.table[slot];
            if id == NIL {
                return (slot, None);
            }
            let record = self.nodes[id as usize];
            let begin = id as usize * self.boxes;
            if record.player == state.player
                && self.box_cells[begin..begin + self.boxes] == state.boxes[..self.boxes]
            {
                return (slot, Some(id));
            }
            slot = (slot + 1) & mask;
        }
    }
    /// Keys the table of an empty arena by boxes and keeper region
    /// ([`Arena::find_region`]) or, with `on` false, by exact state.
    #[cfg(feature = "o6")]
    pub(crate) fn key_by_region(&mut self, on: bool) {
        debug_assert!(self.nodes.is_empty());
        self.keeper_regions = on;
    }
    #[cfg(feature = "o6")]
    pub(crate) fn keeper_regions(&self) -> bool {
        self.keeper_regions
    }
    /// [`Arena::find`] in an arena keyed by keeper region: the node with the
    /// state's boxes whose keeper `same_region` places in the state's keeper
    /// region, and the slot that holds it or would hold a new node for it.
    /// No two live nodes with the same boxes share a region, so at most one
    /// passes. [`Arena::find`] still finds a live node by its exact state,
    /// and only a live one.
    #[cfg(feature = "o6")]
    pub(crate) fn find_region(
        &self,
        state: &State,
        mut same_region: impl FnMut(Cell) -> bool,
    ) -> (usize, Option<u32>) {
        debug_assert!(self.keeper_regions);
        let boxes = &state.boxes[..self.boxes];
        let mask = self.table.len() - 1;
        let mut slot = hash(NONE, boxes) & mask;
        loop {
            let id = self.table[slot];
            if id == NIL {
                return (slot, None);
            }
            let begin = id as usize * self.boxes;
            if self.box_cells[begin..begin + self.boxes] == *boxes
                && same_region(self.nodes[id as usize].player)
            {
                return (slot, Some(id));
            }
            slot = (slot + 1) & mask;
        }
    }
    /// Re-keys a region-keyed table by exact state, in place, for a phase
    /// that keeps every keeper apart. Only live nodes are bound, under their
    /// real keepers, which never collide since no two live nodes with the
    /// same boxes share a region. A superseded node stays unbound and
    /// flagged, so its queued entry still pops as stale and a state it
    /// stood for is new again. Records, queue and stats are untouched.
    #[cfg(feature = "o6")]
    pub(crate) fn key_by_state(&mut self) {
        debug_assert!(self.keeper_regions);
        self.keeper_regions = false;
        self.table.fill(NIL);
        let mask = self.table.len() - 1;
        for (id, record) in self.nodes.iter().enumerate() {
            if record.has(Record::SUPERSEDED) {
                continue;
            }
            let begin = id * self.boxes;
            let mut slot = hash(record.player, &self.box_cells[begin..begin + self.boxes]) & mask;
            while self.table[slot] != NIL {
                slot = (slot + 1) & mask;
            }
            self.table[slot] = id as u32;
        }
    }
    /// Appends `node` and binds `slot`, which [`Arena::find`] (or
    /// `find_region`) returned for its state with no insert since,
    /// replacing any older node there, which becomes superseded. Only one
    /// node may go past the limit, into the spare slot: a solution
    /// discovered at the exact moment the arena filled.
    pub(crate) fn insert(&mut self, node: Node, slot: usize) -> u32 {
        debug_assert!(self.nodes.len() <= self.node_limit);
        let id = self.nodes.len() as u32;
        let previous = self.table[slot];
        if previous == NIL {
            self.stats.unique_states += 1;
        } else {
            self.stats.duplicate_improvements += 1;
            let previous = &mut self.nodes[previous as usize];
            if previous.has(Record::CLOSED) {
                self.stats.reopened_states += 1;
            }
            // Its queued entry, if any, now pops as stale.
            previous.link |= Record::SUPERSEDED;
        }
        self.box_cells
            .extend_from_slice(&node.state.boxes[..self.boxes]);
        self.nodes.push(Record::new(&node));
        self.table[slot] = id;
        id
    }
    pub(crate) fn node(&self, id: u32) -> Node {
        let record = self.nodes[id as usize];
        let mut state = State {
            player: record.player,
            boxes: [NONE; MAX_BOXES],
        };
        let begin = id as usize * self.boxes;
        state.boxes[..self.boxes].copy_from_slice(&self.box_cells[begin..begin + self.boxes]);
        Node {
            state,
            g: record.g,
            parent: record.parent(),
            direction: record.direction(),
            h: record.h,
        }
    }
    /// The node's g, stored h and closed flag, without decoding its state.
    pub(crate) fn meta(&self, id: u32) -> Meta {
        let record = self.nodes[id as usize];
        Meta {
            g: record.g,
            h: record.h,
            closed: record.has(Record::CLOSED),
        }
    }
    /// Whether a later version of the node's state has replaced it: exactly
    /// when [`Arena::find`] on its state would not return this id.
    pub(crate) fn is_superseded(&self, id: u32) -> bool {
        self.nodes[id as usize].has(Record::SUPERSEDED)
    }
    pub(crate) fn stats(&self) -> SearchStats {
        self.stats
    }
    /// Marks the node expanded.
    pub(crate) fn close(&mut self, id: u32) {
        self.nodes[id as usize].link |= Record::CLOSED;
    }
    /// Queues `id` at `f`, with `h` as the tie-break; see [`Key`].
    ///
    /// Every queued key must read `min(g + weight * h, F_SAT)` for the
    /// node's stored g at the current weight, with `h <= MAX_QUEUED_H`;
    /// [`Arena::reweight`] asserts it. So a node queued again after its pop
    /// (a partial expansion, at weight 1) is queued as `enqueue(f', f' - g,
    /// id)`, with its next threshold `f'` computed in exact u64 from
    /// `g + key.h()` and its children's f, never from the popped
    /// [`Key::f`]: that saturates, so code reading it must take `F_SAT` as
    /// "at least `F_SAT`".
    pub(crate) fn enqueue(&mut self, f: u64, h: u32, id: u32) {
        debug_assert!(h <= MAX_QUEUED_H);
        self.heap.push(Reverse(Key::new(f, h, id)));
        self.stats.peak_queue = self.stats.peak_queue.max(self.heap.len() as u32);
    }
    /// The lowest queued entry, which may be stale: its node, the h it was
    /// queued with, and its f.
    pub(crate) fn dequeue(&mut self) -> Option<Key> {
        self.heap.pop().map(|Reverse(key)| key)
    }
    /// Lowest queued f, stale entries included.
    pub(crate) fn min_f(&self) -> Option<u64> {
        self.heap.peek().map(|Reverse(key)| key.f())
    }
    /// Forgets every node and queued entry in place. The reservation, limit
    /// and stats are kept.
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.box_cells.clear();
        self.heap.clear();
        self.table.fill(NIL);
    }
    /// Re-keys every entry queued at `g + from * h` as `g + to * h`, in the
    /// reserved allocation, so the order is as if each entry had been queued
    /// at `to`. g comes from the entry's record, which is exact even where
    /// the old key saturated.
    pub(crate) fn reweight(&mut self, from: u32, to: u32) {
        let mut entries = std::mem::take(&mut self.heap).into_vec();
        for Reverse(key) in &mut entries {
            let (g, h) = (u64::from(self.nodes[key.id() as usize].g), key.h());
            debug_assert_eq!(
                key.f(),
                (g + u64::from(from) * u64::from(h)).min(Key::F_SAT)
            );
            *key = Key::new(g + u64::from(to) * u64::from(h), h, key.id());
        }
        self.heap = BinaryHeap::from(entries);
    }
}
