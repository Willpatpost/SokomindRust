use crate::{Status, deadlock::Deadlock, reach::Reach};
use sokomind_core::{MAX_ROUTE, State};
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
    nodes: Vec<Node>,
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
        // Flood and deadlock buffers, u16 reverse distances per goal (one
        // goal per box), and the route plus its string.
        let per_cell = Reach::BYTES_PER_CELL + Deadlock::BYTES_PER_CELL + boxes * 2;
        let fixed_bytes = cells * per_cell + 2 * MAX_ROUTE + 64 * 1024;
        let budget = memory_mib * 1024 * 1024;
        // One spare node keeps a solution found at the exact limit reachable.
        let bytes_for = |count: usize| {
            fixed_bytes
                + (count + 1) * (size_of::<Node>() + size_of::<Entry>())
                + ((count + 1) * 2).next_power_of_two() * size_of::<u32>()
        };
        // Exact largest limit that fits the budget, instead of stepping down 10%.
        let mut low = 0;
        let mut high = max_states;
        while low < high {
            let mid = low + (high - low + 1) / 2;
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
            if self.nodes[id as usize].state == *state {
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
        self.nodes.push(node);
        self.table[slot] = id;
        id
    }
    pub(crate) fn node(&self, id: u32) -> Node {
        self.nodes[id as usize]
    }
    pub(crate) fn enqueue(&mut self, f: u64, h: u32, id: u32) {
        self.heap.push(Reverse((f, h, id)));
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
