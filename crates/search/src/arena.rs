use crate::{SearchStats, Status, deadlock::Deadlock, heuristic::Heuristic, reach::Reach};
use sokomind_core::{Cell, MAX_BOXES, MAX_ROUTE, NONE, State};
use std::{cmp::Reverse, collections::BinaryHeap, mem::size_of};

/// No node: an empty table slot, or the root's parent.
pub(crate) const NIL: u32 = u32::MAX;

#[derive(Clone, Copy)]
pub(crate) struct Node {
    pub state: State,
    pub g: u32,
    pub parent: u32,
    /// Direction of the push that made this node (unused at the root); the
    /// pushed box started at `state.player`.
    pub direction: u8,
    /// [`Node::CLOSED`] once the node has been expanded.
    pub flags: u8,
    /// The state's estimate, or `u16::MAX` when it is unknown or too large
    /// and must be recomputed. Estimates depend only on the canonical
    /// state, so a cheaper duplicate reuses it. This stack value is decoded
    /// from compact storage only when an expansion or replay needs it.
    pub h: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    flags: 0,
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
                    flags: 0,
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
                    flags: 0,
                    h: 3,
                },
                slot,
            );
            assert_eq!(arena.find(&state).1, Some(improved));
            assert_eq!(arena.find(&child).1, Some(descendant));
            assert_eq!(arena.node(descendant).parent, old);
            assert_eq!(arena.node(old).g, 10);
            assert_eq!(arena.node(old).state, state);
            assert_eq!(arena.node(improved).g, 8);
            assert_eq!(arena.stats.unique_states, 2);
            assert_eq!(arena.stats.duplicate_improvements, 1);
            assert_eq!(arena.stats.reopened_states, 1);
        }
    }
}
impl Node {
    pub(crate) const CLOSED: u8 = 1;
    /// The estimate as stored in [`Node::h`].
    pub(crate) fn store_h(h: u32) -> u16 {
        u16::try_from(h).unwrap_or(u16::MAX)
    }
    /// The stored estimate, if it fit.
    pub(crate) fn known_h(&self) -> Option<u32> {
        (self.h < u16::MAX).then_some(u32::from(self.h))
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.flags & Self::CLOSED != 0
    }
}
/// Metadata is fixed-size; box cells live in an active-prefix column. Parent
/// indices always refer to immutable appended versions, never overwritten states.
#[derive(Clone, Copy)]
struct Record {
    g: u32,
    parent: u32,
    player: Cell,
    h: u16,
    direction: u8,
    flags: u8,
}

/// Queue entry `(f, h, id)`: lowest f first, then lowest h, then oldest.
type Entry = Reverse<(u64, u32, u32)>;

fn hash(state: &State, boxes: usize) -> usize {
    let mut h = (state.player as u64).wrapping_add(0x9e3779b97f4a7c15);
    for &cell in &state.boxes[..boxes] {
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
}
impl Arena {
    pub(crate) fn new(
        cells: usize,
        boxes: usize,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, String> {
        if !(1..=1_000_000).contains(&max_states) || !(4..=256).contains(&memory_mib) {
            return Err("Use 1..1000000 states and 4..256 MiB".into());
        }
        // Flood and deadlock buffers, the dead-cell mask, u16 reverse
        // distances per goal (one goal per box), and the route plus its string.
        let per_cell = Reach::BYTES_PER_CELL
            + Deadlock::BYTES_PER_CELL
            + Heuristic::BYTES_PER_CELL
            + boxes * 2;
        let fixed_bytes = cells * per_cell + 2 * MAX_ROUTE + 64 * 1024;
        let budget = memory_mib * 1024 * 1024;
        // One spare node keeps a solution found at the exact limit reachable.
        let bytes_for = |count: usize| {
            fixed_bytes
                + (count + 1)
                    * (size_of::<Record>() + boxes * size_of::<Cell>() + size_of::<Entry>())
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
            return Err("Memory budget is too small".into());
        }
        let limit = low;
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(limit + 1)
            .map_err(|_| "Cannot reserve node arena")?;
        let mut box_cells = Vec::new();
        box_cells
            .try_reserve_exact((limit + 1) * boxes)
            .map_err(|_| "Cannot reserve box arena")?;
        let mut heap = BinaryHeap::new();
        heap.try_reserve_exact(limit + 1)
            .map_err(|_| "Cannot reserve search queue")?;
        let table_size = ((limit + 1) * 2).next_power_of_two();
        let mut table = Vec::new();
        table
            .try_reserve_exact(table_size)
            .map_err(|_| "Cannot reserve state table")?;
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
        let mut slot = hash(state, self.boxes) & mask;
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
    /// Appends `node` and binds `slot`, which [`Arena::find`] returned for its
    /// state with no insert since, replacing any older node there. Only one
    /// node may go past the limit, into the spare slot: a solution discovered
    /// at the exact moment the arena filled.
    pub(crate) fn insert(&mut self, node: Node, slot: usize) -> u32 {
        debug_assert!(self.nodes.len() <= self.node_limit);
        let id = self.nodes.len() as u32;
        let previous = self.table[slot];
        if previous == NIL {
            self.stats.unique_states += 1;
        } else {
            self.stats.duplicate_improvements += 1;
            if self.nodes[previous as usize].flags & Node::CLOSED != 0 {
                self.stats.reopened_states += 1;
            }
        }
        self.box_cells
            .extend_from_slice(&node.state.boxes[..self.boxes]);
        self.nodes.push(Record {
            g: node.g,
            parent: node.parent,
            player: node.state.player,
            h: node.h,
            direction: node.direction,
            flags: node.flags,
        });
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
            parent: record.parent,
            h: record.h,
            direction: record.direction,
            flags: record.flags,
        }
    }
    pub(crate) fn stats(&self) -> SearchStats {
        self.stats
    }
    /// Marks the node expanded.
    pub(crate) fn close(&mut self, id: u32) {
        self.nodes[id as usize].flags |= Node::CLOSED;
    }
    pub(crate) fn enqueue(&mut self, f: u64, h: u32, id: u32) {
        self.heap.push(Reverse((f, h, id)));
        self.stats.peak_queue = self.stats.peak_queue.max(self.heap.len() as u32);
    }
    /// The node of the lowest queued entry, which may be stale, with the h
    /// it was queued with.
    pub(crate) fn dequeue(&mut self) -> Option<(u32, u32)> {
        self.heap.pop().map(|Reverse((_, h, id))| (id, h))
    }
    /// Lowest queued f, stale entries included.
    pub(crate) fn min_f(&self) -> Option<u64> {
        self.heap.peek().map(|Reverse((f, _, _))| *f)
    }
}
