use crate::{
    Status,
    arena::{Arena, Node},
    deadlock::Deadlock,
    heuristic::Heuristic,
    reach::Reach,
};
use sokomind_core::{ACTIONS, Board, NONE, OPPOSITE, State};
use std::cmp::Reverse;

/// What separates the modes. Everything else, from child order to pruning
/// and limit handling, is shared.
#[derive(Clone, Copy)]
pub(crate) struct Policy {
    /// Queue priority is `g + weight * h`; 1 keeps f admissible.
    weight: u32,
    /// Stop as soon as a solved state is popped, instead of recording it and
    /// continuing to improve the incumbent.
    stop_on_goal_pop: bool,
    /// Stop as soon as a solved child is generated.
    stop_on_goal_push: bool,
    /// Let a cheaper path re-add a state already in the table. The engine
    /// keeps no closed bit, so `false` would also refuse cheaper paths to
    /// states still open; every mode reopens today.
    reopen_closed: bool,
}
impl Policy {
    /// Admissible A*; exact soundness never depends on consistency.
    pub(crate) const EXACT: Self = Self {
        weight: 1,
        stop_on_goal_pop: true,
        stop_on_goal_push: false,
        reopen_closed: true,
    };
    /// First route wins.
    pub(crate) const FAST: Self = Self {
        weight: 5,
        stop_on_goal_pop: true,
        stop_on_goal_push: true,
        reopen_closed: true,
    };
    /// Keeps improving the incumbent until the queue empties or a limit hits.
    pub(crate) const QUALITY: Self = Self {
        weight: 3,
        stop_on_goal_pop: false,
        stop_on_goal_push: false,
        reopen_closed: true,
    };
}

/// Push search over one reserved arena. Only [`crate::ExactSearch`] turns its
/// state into bounds or a proof; with a weighted policy results are always
/// optimality unknown.
pub struct Engine {
    policy: Policy,
    board: Board,
    start: State,
    heuristic: Heuristic,
    reach: Reach,
    deadlock: Deadlock,
    pub(crate) arena: Arena,
    status: Status,
    expanded: u32,
    generated: u32,
    incumbent: Option<u32>,
    /// Dual repair only pays on large label groups; otherwise children
    /// re-solve their own changed group.
    incremental: bool,
    /// Cost of a node whose expansion a limit cut short. Its unpushed
    /// successors have f >= g + 1, which the exact frontier must include.
    pub(crate) interrupted_g: Option<u32>,
}

impl Engine {
    pub(crate) fn new(
        board: Board,
        mut start: State,
        policy: Policy,
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
            policy,
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
            search
                .arena
                .push_entry(Reverse((h as u64 * policy.weight as u64, h, 0)));
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
    /// The arena is full while expanding a node of cost `g`.
    fn stop_at_limit(&mut self, g: u32) {
        self.interrupted_g = Some(g);
        self.status = self.arena.limit_status();
    }
    /// Work is sliced by expansions so a worker can yield, report, or cancel.
    pub fn advance(&mut self, expansions: u32) {
        if self.status != Status::Running {
            return;
        }
        for _ in 0..expansions {
            let Some(Reverse((_, _, index))) = self.arena.pop_entry() else {
                // The queue emptied: every state was popped, dominated, or
                // pruned by an admissible rule. Under the exact policy that
                // makes the incumbent optimal.
                self.status = if self.incumbent.is_some() {
                    Status::Solved
                } else {
                    Status::Exhausted
                };
                return;
            };
            let node = self.arena.node(index);
            if self
                .arena
                .entry(self.arena.slot(&node.state, self.board.labels.len()))
                != index
            {
                continue;
            }
            if self.board.solved(&node.state) {
                if self.best_moves().is_none_or(|best| node.g < best) {
                    self.incumbent = Some(index);
                }
                if self.policy.stop_on_goal_pop {
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
            self.deadlock
                .refresh(&node.state.boxes[..self.board.labels.len()]);
            if !self.board.solved(&node.state)
                && !self.reach.has_legal_push(&self.board, &node.state)
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
                    // Deadlock indices follow the parent order `refresh` saw,
                    // so check before canonicalize reorders a label group.
                    if self.deadlock.is_dead_after_push(
                        &self.board,
                        &next.boxes[..self.board.labels.len()],
                        i,
                        from,
                        to,
                    ) {
                        continue;
                    }
                    self.board.canonicalize(&mut next);
                    let slot = self.arena.slot(&next, self.board.labels.len());
                    let previous = self.arena.entry(slot);
                    if previous != u32::MAX
                        && (!self.policy.reopen_closed || self.arena.node(previous).g <= g)
                    {
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
                    // Past the prune above g + h < best, so a solved child
                    // always improves the incumbent.
                    let goal = h == 0 && self.board.solved(&next);
                    let child = Node {
                        state: next,
                        g,
                        parent: index,
                        box_from: from,
                        direction: d as u8,
                    };
                    if self.arena.is_full() {
                        // Keep a solution discovered at the exact limit.
                        if goal {
                            let id = self.arena.push(child);
                            self.arena.bind(slot, id);
                            self.generated += 1;
                            self.incumbent = Some(id);
                        }
                        self.stop_at_limit(node.g);
                        return;
                    }
                    let id = self.arena.push(child);
                    self.arena.bind(slot, id);
                    self.generated += 1;
                    self.arena.push_entry(Reverse((
                        g as u64 + self.policy.weight as u64 * h as u64,
                        h,
                        id,
                    )));
                    // Keep a solution even if a limit occurs before its pop.
                    if goal {
                        self.incumbent = Some(id);
                        if self.policy.stop_on_goal_push {
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
