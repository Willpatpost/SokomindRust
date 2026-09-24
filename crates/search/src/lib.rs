//! Incremental, platform-independent push A*. All hot-path storage is reserved once.
mod heuristic;
mod reach;
use heuristic::Heuristic;
use reach::Reach;
use sokomind_core::{ACTIONS, Board, Cell, NONE, OPPOSITE, State};
use std::{cmp::Reverse, collections::BinaryHeap, mem::size_of};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fast,
    Quality,
    Optimal,
}
impl Mode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "fast" => Ok(Self::Fast),
            "quality" => Ok(Self::Quality),
            "optimal" => Ok(Self::Optimal),
            _ => Err("Mode must be fast, quality, or optimal".into()),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Solved,
    Exhausted,
    StateLimit,
    MemoryLimit,
    TimeLimit,
    Cancelled,
}
impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Solved => "solved",
            Self::Exhausted => "exhausted",
            Self::StateLimit => "state_limit",
            Self::MemoryLimit => "memory_limit",
            Self::TimeLimit => "time_limit",
            Self::Cancelled => "cancelled",
        }
    }
}
#[derive(Clone, Copy)]
struct Node {
    state: State,
    g: u32,
    parent: u32,
    box_from: Cell,
    direction: u8,
}
type Entry = Reverse<(u64, u32, u32)>;

pub struct Search {
    board: Board,
    start: State,
    mode: Mode,
    heuristic: Heuristic,
    reach: Reach,
    nodes: Vec<Node>,
    heap: BinaryHeap<Entry>,
    /// Open-addressed table stores arena indices, never duplicated box arrays.
    table: Vec<u32>,
    node_limit: usize,
    limit_status: Status,
    pub status: Status,
    pub expanded: u32,
    pub generated: u32,
    pub reserved_bytes: usize,
    incumbent: Option<u32>,
    pub proven: bool,
}

impl Search {
    pub fn new(
        board: Board,
        mut start: State,
        mode: Mode,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, String> {
        if !(1..=1_000_000).contains(&max_states) || !(4..=256).contains(&memory_mib) {
            return Err("Use 1..1000000 states and 4..256 MiB".into());
        }
        board.canonicalize(&mut start);
        let cells = board.tiles.len();
        // Includes topology, reverse distances, reusable flood buffers and route scratch.
        let fixed_bytes =
            cells * (32 + board.goals.len() * 2) + 2 * sokomind_core::MAX_ROUTE + 64 * 1024;
        let budget = memory_mib * 1024 * 1024;
        let bytes_for = |count: usize| {
            fixed_bytes
                + count * (size_of::<Node>() + size_of::<Entry>() + size_of::<u32>())
                + (count * 2).next_power_of_two() * size_of::<u32>()
        };
        let mut limit = max_states;
        while limit > 0 && bytes_for(limit) > budget {
            limit = limit * 9 / 10;
        }
        if limit == 0 {
            return Err("Memory budget is too small".into());
        }
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(limit)
            .map_err(|_| "Cannot reserve node arena")?;
        let mut heap = BinaryHeap::new();
        heap.try_reserve_exact(limit)
            .map_err(|_| "Cannot reserve search queue")?;
        let mut table = Vec::new();
        let table_size = (limit * 2).next_power_of_two();
        table
            .try_reserve_exact(table_size)
            .map_err(|_| "Cannot reserve state table")?;
        table.resize(table_size, u32::MAX);
        let heuristic = Heuristic::new(&board);
        let h = heuristic.estimate(&board, &start);
        let mut search = Self {
            reach: Reach::new(cells),
            board,
            start,
            mode,
            heuristic,
            nodes,
            heap,
            table,
            node_limit: limit,
            limit_status: if limit < max_states {
                Status::MemoryLimit
            } else {
                Status::StateLimit
            },
            status: Status::Running,
            expanded: 0,
            generated: 1,
            reserved_bytes: bytes_for(limit),
            incumbent: None,
            proven: false,
        };
        search.nodes.push(Node {
            state: start,
            g: 0,
            parent: u32::MAX,
            box_from: NONE,
            direction: 0,
        });
        let slot = search.slot(&start);
        search.table[slot] = 0;
        if let Some(h) = h {
            search
                .heap
                .push(Reverse((h as u64 * search.weight(), h, 0)));
        } else {
            search.status = Status::Exhausted;
        }
        Ok(search)
    }
    fn weight(&self) -> u64 {
        match self.mode {
            Mode::Fast => 5,
            Mode::Quality => 3,
            Mode::Optimal => 1,
        }
    }
    fn hash(&self, state: &State) -> usize {
        let mut h = (state.player as u64).wrapping_add(0x9e3779b97f4a7c15);
        for &cell in &state.boxes[..self.board.labels.len()] {
            h ^= cell as u64;
            h = h.wrapping_mul(0x100000001b3);
            h ^= h >> 29;
        }
        (h ^ (h >> 32)) as usize
    }
    fn slot(&self, state: &State) -> usize {
        let mask = self.table.len() - 1;
        let mut slot = self.hash(state) & mask;
        while self.table[slot] != u32::MAX && self.nodes[self.table[slot] as usize].state != *state
        {
            slot = (slot + 1) & mask;
        }
        slot
    }
    pub fn best_moves(&self) -> Option<u32> {
        self.incumbent.map(|i| self.nodes[i as usize].g)
    }
    pub fn stop(&mut self, reason: Status) {
        if self.status == Status::Running {
            self.status = reason;
        }
    }
    /// Work is sliced by expansions so a worker can yield, report, or cancel.
    pub fn advance(&mut self, expansions: u32) {
        if self.status != Status::Running {
            return;
        }
        for _ in 0..expansions {
            let Some(Reverse((_, _, index))) = self.heap.pop() else {
                self.status = if self.incumbent.is_some() {
                    Status::Solved
                } else {
                    Status::Exhausted
                };
                return;
            };
            let node = self.nodes[index as usize];
            if self.table[self.slot(&node.state)] != index {
                continue;
            }
            if self.board.solved(&node.state) {
                if self.best_moves().is_none_or(|best| node.g < best) {
                    self.incumbent = Some(index);
                }
                if self.mode != Mode::Quality {
                    self.proven = self.mode == Mode::Optimal;
                    self.status = Status::Solved;
                    return;
                }
                continue;
            }
            if self.best_moves().is_some_and(|best| node.g >= best) {
                continue;
            }
            self.expanded += 1;
            self.reach.fill(&self.board, &node.state);
            for i in 0..self.board.labels.len() {
                let from = node.state.boxes[i];
                for (d, &opposite) in OPPOSITE.iter().enumerate() {
                    let to = self.board.neighbors[from as usize][d];
                    let stand = self.board.neighbors[from as usize][opposite];
                    if to == NONE
                        || stand == NONE
                        || self.reach.occupied[to as usize]
                        || self.reach.distances[stand as usize] == NONE
                    {
                        continue;
                    }
                    let g = node.g + self.reach.distances[stand as usize] as u32 + 1;
                    if self.best_moves().is_some_and(|best| g >= best) {
                        continue;
                    }
                    let mut next = node.state;
                    next.player = from;
                    next.boxes[i] = to;
                    self.board.canonicalize(&mut next);
                    let slot = self.slot(&next);
                    let previous = self.table[slot];
                    if previous != u32::MAX && self.nodes[previous as usize].g <= g {
                        continue;
                    }
                    let Some(h) = self.heuristic.estimate(&self.board, &next) else {
                        continue;
                    };
                    if self
                        .best_moves()
                        .is_some_and(|best| g as u64 + h as u64 >= best as u64)
                    {
                        continue;
                    }
                    // Check before EVERY allocation, including reopened states.
                    if self.nodes.len() >= self.node_limit {
                        self.status = self.limit_status;
                        return;
                    }
                    let id = self.nodes.len() as u32;
                    self.nodes.push(Node {
                        state: next,
                        g,
                        parent: index,
                        box_from: from,
                        direction: d as u8,
                    });
                    self.table[slot] = id;
                    self.generated += 1;
                    self.heap
                        .push(Reverse((g as u64 + self.weight() * h as u64, h, id)));
                    // Keep a solution even if a resource limit occurs before its heap pop.
                    if h == 0
                        && self.board.solved(&next)
                        && self.best_moves().is_none_or(|best| g < best)
                    {
                        self.incumbent = Some(id);
                        if self.mode == Mode::Fast {
                            self.status = Status::Solved;
                            return;
                        }
                    }
                }
            }
        }
    }
    /// Reconstruct walks only once per reported incumbent, then independently replay.
    pub fn solution(&mut self) -> Result<Option<String>, String> {
        let Some(mut id) = self.incumbent else {
            return Ok(None);
        };
        let expected = self.nodes[id as usize].g;
        if expected as usize > sokomind_core::MAX_ROUTE {
            return Err("Solution exceeds the 100000-move replay limit".into());
        }
        let mut chain = Vec::with_capacity((expected as usize).min(self.nodes.len()));
        while self.nodes[id as usize].parent != u32::MAX {
            chain.push(id);
            id = self.nodes[id as usize].parent;
        }
        let mut route = Vec::with_capacity(expected as usize);
        for id in chain.into_iter().rev() {
            let node = self.nodes[id as usize];
            let parent = self.nodes[node.parent as usize].state;
            self.reach.fill(&self.board, &parent);
            let stand =
                self.board.neighbors[node.box_from as usize][OPPOSITE[node.direction as usize]];
            self.reach.append_path(&self.board, stand, &mut route);
            route.push(ACTIONS[node.direction as usize]);
        }
        let mut replay = self.start;
        for &action in &route {
            let direction = ACTIONS.iter().position(|&a| a == action).unwrap();
            if self.board.step(&mut replay, direction).is_none() {
                return Err("Internal route replay failed".into());
            }
        }
        if !self.board.solved(&replay) || route.len() != expected as usize {
            return Err("Internal solution counters failed".into());
        }
        Ok(Some(String::from_utf8(route).unwrap()))
    }
}
