use crate::{
    arena::{Arena, Node},
    certificate::Proof,
    deadlock::Deadlock,
    heuristic::Heuristic,
    reach::Reach,
    Status,
};
use sokomind_core::{ACTIONS, Board, NONE, OPPOSITE, State};
use std::cmp::Reverse;

/// Admissible move-optimal A* with reopenings. The only engine that may
/// produce a [`Proof`].
pub struct ExactSearch {
    board: Board,
    start: State,
    heuristic: Heuristic,
    reach: Reach,
    deadlock: Deadlock,
    arena: Arena,
    status: Status,
    expanded: u32,
    generated: u32,
    incumbent: Option<u32>,
    /// Dual repair only pays on large label groups; otherwise children
    /// re-solve their own changed group.
    incremental: bool,
    /// Cost of a node whose expansion a limit cut short. Its unpushed
    /// successors have f >= g + 1, which the frontier bound must include.
    interrupted_g: Option<u32>,
}

impl ExactSearch {
    pub fn new(
        board: Board,
        mut start: State,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, String> {
        board.canonicalize(&mut start);
        let cells = board.tiles.len();
        let arena = Arena::new(cells, board.goals.len(), max_states, memory_mib)?;
        let heuristic = Heuristic::new(&board);
        let deadlock = Deadlock::new(&board);
        let h = heuristic.estimate(&start);
        let incremental = heuristic.repairs_worthwhile();
        let mut search = Self {
            reach: Reach::new(cells),
            arena,
            board,
            start,
            heuristic,
            deadlock,
            status: Status::Running,
            expanded: 0,
            generated: 1,
            incumbent: None,
            incremental,
            interrupted_g: None,
        };
        search.arena.push(Node {
            state: start,
            g: 0,
            parent: u32::MAX,
            box_from: NONE,
            direction: 0,
        });
        let slot = search.arena.slot(&start, search.board.labels.len());
        search.arena.bind(slot, 0);
        if let Some(h) = h {
            search.arena.push_entry(Reverse((h as u64, h, 0)));
        } else {
            search.status = Status::Exhausted;
        }
        Ok(search)
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn best_moves(&self) -> Option<u32> {
        self.incumbent.map(|i| self.arena.node(i).g)
    }
    pub fn expanded(&self) -> u32 {
        self.expanded
    }
    pub fn generated(&self) -> u32 {
        self.generated
    }
    pub fn reserved_bytes(&self) -> usize {
        self.arena.reserved_bytes()
    }
    pub fn stop(&mut self, reason: Status) {
        if self.status == Status::Running {
            self.status = reason;
        }
    }
    /// Live certified lower bound on the optimal move count from the start.
    pub fn lower_bound(&self) -> Option<u32> {
        if let Some(proof) = self.proof() {
            return match proof {
                Proof::Bounded { lower_bound, .. } | Proof::Optimal { moves: lower_bound } => {
                    Some(lower_bound)
                }
                Proof::Unsolvable => None,
            };
        }
        let frontier = self.frontier();
        match self.best_moves() {
            Some(best) if frontier >= best as u64 => Some(best),
            _ => (frontier < u32::MAX as u64).then_some(frontier as u32),
        }
    }
    /// Terminal proof; `None` while running or without a sound certificate.
    pub fn proof(&self) -> Option<Proof> {
        match self.status {
            Status::Running => None,
            Status::Solved => self.best_moves().map(|moves| Proof::Optimal { moves }),
            Status::Exhausted => Some(Proof::Unsolvable),
            _ => self.bounded_proof(),
        }
    }
    /// A limit or cancellation keeps every bound computed so far.
    fn bounded_proof(&self) -> Option<Proof> {
        let upper_bound = self.best_moves()?;
        let frontier = self.frontier();
        if frontier >= upper_bound as u64 {
            return Some(Proof::Optimal { moves: upper_bound });
        }
        Some(Proof::Bounded {
            lower_bound: frontier as u32,
            upper_bound,
        })
    }
    /// Minimum f over everything not yet expanded, including the unpushed
    /// successors of an interrupted expansion.
    fn frontier(&self) -> u64 {
        self.arena
            .peek_priority()
            .unwrap_or(u64::MAX)
            .min(self.interrupted_g.map_or(u64::MAX, |g| g as u64 + 1))
    }
    /// Work is sliced by expansions so a worker can yield, report, or cancel.
    pub fn advance(&mut self, expansions: u32) {
        if self.status != Status::Running {
            return;
        }
        for _ in 0..expansions {
            let Some(Reverse((_, _, index))) = self.arena.pop_entry() else {
                // The queue emptied: every state was popped, dominated, or
                // pruned by an admissible rule, so the incumbent is optimal.
                self.status = if self.incumbent.is_some() {
                    Status::Solved
                } else {
                    Status::Exhausted
                };
                return;
            };
            let node = self.arena.node(index);
            if self.arena.entry(self.arena.slot(&node.state, self.board.labels.len())) != index {
                continue;
            }
            if self.board.solved(&node.state) {
                if self.best_moves().is_none_or(|best| node.g < best) {
                    self.incumbent = Some(index);
                }
                self.status = Status::Solved;
                return;
            }
            if self.best_moves().is_some_and(|best| node.g >= best) {
                continue;
            }
            self.expanded += 1;
            self.reach.fill(&self.board, &node.state);
            self.deadlock
                .refresh(&node.state.boxes[..self.board.labels.len()]);
            if !self.board.solved(&node.state) && !self.reach.has_legal_push(&self.board, &node.state)
            {
                continue;
            }
            let parent_assignment = if self.incremental {
                self.heuristic.assignment(&node.state)
            } else {
                None
            };
            for i in 0..self.board.labels.len() {
                let from = node.state.boxes[i];
                for (d, &opposite) in OPPOSITE.iter().enumerate() {
                    let to = self.board.neighbors[from as usize][d];
                    let stand = self.board.neighbors[from as usize][opposite];
                    if to == NONE
                        || stand == NONE
                        || self.reach.blocked(to)
                        || self.reach.distance(stand) == NONE
                    {
                        continue;
                    }
                    let g = node.g + self.reach.distance(stand) as u32 + 1;
                    if self.best_moves().is_some_and(|best| g >= best) {
                        continue;
                    }
                    let mut next = node.state;
                    next.player = from;
                    next.boxes[i] = to;
                    self.board.canonicalize(&mut next);
                    if self.deadlock.is_dead_after_push(
                        &self.board,
                        &next.boxes[..self.board.labels.len()],
                        i,
                        from,
                        to,
                    ) {
                        continue;
                    }
                    let slot = self.arena.slot(&next, self.board.labels.len());
                    let previous = self.arena.entry(slot);
                    if previous != u32::MAX && self.arena.node(previous).g <= g {
                        continue;
                    }
                    let estimate = match &parent_assignment {
                        Some(parent) => self.heuristic.estimate_from(parent, &next),
                        None => self.heuristic.estimate(&next),
                    };
                    let Some(h) = estimate else {
                        continue;
                    };
                    if self
                        .best_moves()
                        .is_some_and(|best| g as u64 + h as u64 >= best as u64)
                    {
                        continue;
                    }
                    if self.arena.is_full() {
                        // Keep a solution discovered at the exact limit.
                        if h == 0
                            && self.board.solved(&next)
                            && self.best_moves().is_none_or(|best| g < best)
                        {
                            let id = self.arena.push_final(Node {
                                state: next,
                                g,
                                parent: index,
                                box_from: from,
                                direction: d as u8,
                            });
                            self.arena.bind(slot, id);
                            self.generated += 1;
                            self.incumbent = Some(id);
                        }
                        self.interrupted_g = Some(node.g);
                        self.status = self.arena.limit_status();
                        return;
                    }
                    let id = self.arena.push(Node {
                        state: next,
                        g,
                        parent: index,
                        box_from: from,
                        direction: d as u8,
                    });
                    self.arena.bind(slot, id);
                    self.generated += 1;
                    self.arena.push_entry(Reverse((g as u64 + h as u64, h, id)));
                    // Keep a solution even if a limit occurs before its pop.
                    if h == 0
                        && self.board.solved(&next)
                        && self.best_moves().is_none_or(|best| g < best)
                    {
                        self.incumbent = Some(id);
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
        let expected = self.arena.node(id).g;
        if expected as usize > sokomind_core::MAX_ROUTE {
            return Err("Solution exceeds the 100000-move replay limit".into());
        }
        let mut chain = Vec::with_capacity((expected as usize).min(self.arena.len()));
        while self.arena.node(id).parent != u32::MAX {
            chain.push(id);
            id = self.arena.node(id).parent;
        }
        let mut route = Vec::with_capacity(expected as usize);
        for id in chain.into_iter().rev() {
            let node = self.arena.node(id);
            let parent = self.arena.node(node.parent).state;
            let stand =
                self.board.neighbors[node.box_from as usize][OPPOSITE[node.direction as usize]];
            self.reach.fill_to(&self.board, &parent, stand);
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
