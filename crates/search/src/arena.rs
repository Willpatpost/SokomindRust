use crate::Status;
use sokomind_core::{Cell, MAX_ROUTE, State};
use std::{cmp::Reverse, collections::BinaryHeap, mem::size_of};

#[derive(Clone, Copy)]
pub(crate) struct Node {
    pub state: State,
    pub g: u32,
    pub parent: u32,
    pub box_from: Cell,
    pub direction: u8,
}
pub(crate) type Entry = Reverse<(u64, u32, u32)>;

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
    node_limit: usize,
    reserved_bytes: usize,
    scaled_down: bool,
}
impl Arena {
    pub(crate) fn new(
        cells: usize,
        goals: usize,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, String> {
        if !(1..=1_000_000).contains(&max_states) || !(4..=256).contains(&memory_mib) {
            return Err("Use 1..1000000 states and 4..256 MiB".into());
        }
        // Includes reachability buffers, reverse distances, and route scratch.
        let fixed_bytes = cells * (9 + goals * 2) + 2 * MAX_ROUTE + 64 * 1024;
        let budget = memory_mib * 1024 * 1024;
        // One spare node keeps a solution found at the exact limit reachable.
        let bytes_for = |count: usize| {
            fixed_bytes
                + (count + 1) * (size_of::<Node>() + size_of::<Entry>() + size_of::<u32>())
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
        table.resize(table_size, u32::MAX);
        Ok(Self {
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
    /// Callers must bind the node's table slot. Only one node may be pushed
    /// past the limit, into the spare slot: a solution discovered at the exact
    /// moment the arena filled.
    pub(crate) fn push(&mut self, node: Node) -> u32 {
        debug_assert!(self.nodes.len() <= self.node_limit);
        let id = self.nodes.len() as u32;
        self.nodes.push(node);
        id
    }
    pub(crate) fn node(&self, id: u32) -> Node {
        self.nodes[id as usize]
    }
    pub(crate) fn slot(&self, state: &State, boxes: usize) -> usize {
        let mask = self.table.len() - 1;
        let mut slot = hash(state, boxes) & mask;
        while self.table[slot] != u32::MAX && self.nodes[self.table[slot] as usize].state != *state
        {
            slot = (slot + 1) & mask;
        }
        slot
    }
    pub(crate) fn entry(&self, slot: usize) -> u32 {
        self.table[slot]
    }
    pub(crate) fn bind(&mut self, slot: usize, id: u32) {
        self.table[slot] = id;
    }
    pub(crate) fn push_entry(&mut self, entry: Entry) {
        self.heap.push(entry);
    }
    pub(crate) fn pop_entry(&mut self) -> Option<Entry> {
        self.heap.pop()
    }
    pub(crate) fn peek_priority(&self) -> Option<u64> {
        self.heap.peek().map(|Reverse((priority, _, _))| *priority)
    }
}
