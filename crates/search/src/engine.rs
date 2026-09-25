use crate::{
    Status,
    arena::{Arena, NIL, Node},
    deadlock::Deadlock,
    heuristic::{Heuristic, ParentGroup},
    reach::Reach,
};
use sokomind_core::{ACTIONS, Board, MAX_ROUTE, NONE, OPPOSITE, State};

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
    /// Let a cheaper path re-add a state whose node was already expanded
    /// ([`Node::CLOSED`]). A cheaper path to a state still open always
    /// replaces it.
    reopen_closed: bool,
    /// Also skip a popped node when `g + h >= best`, with the h it was
    /// queued with; every child would fail the child-level prune anyway.
    /// Exact leaves it off: there a goal pops before any such node, and the
    /// check stays out of the kernel that proves.
    prune_popped_estimate: bool,
}
impl Policy {
    /// Admissible A*; exact soundness never depends on consistency.
    pub(crate) const EXACT: Self = Self {
        weight: 1,
        stop_on_goal_pop: true,
        stop_on_goal_push: false,
        reopen_closed: true,
        prune_popped_estimate: false,
    };
    /// First route wins. Without reopening, weighted A* keeps its
    /// suboptimality bound under a consistent h, and never proves anyway.
    pub(crate) const FAST: Self = Self {
        weight: 5,
        stop_on_goal_pop: true,
        stop_on_goal_push: true,
        reopen_closed: false,
        prune_popped_estimate: true,
    };
    /// Keeps improving the incumbent until the queue empties or a limit hits.
    pub(crate) const QUALITY: Self = Self {
        weight: 3,
        stop_on_goal_pop: false,
        stop_on_goal_push: false,
        reopen_closed: true,
        prune_popped_estimate: true,
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
    incumbent: Option<u32>,
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
        let arena = Arena::new(cells, board.labels.len(), max_states, memory_mib)?;
        let heuristic = Heuristic::new(&board);
        let deadlock = Deadlock::new(&board);
        let h = heuristic.estimate(&start);
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
            incumbent: None,
            interrupted_g: None,
        };
        let (slot, _) = search.arena.find(&start);
        let root = search.arena.insert(
            Node {
                state: start,
                g: 0,
                parent: NIL,
                direction: 0,
                flags: 0,
                h: h.map_or(u16::MAX, Node::store_h),
            },
            slot,
        );
        if let Some(h) = h {
            search
                .arena
                .enqueue(h as u64 * policy.weight as u64, h, root);
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
        self.arena.len() as u32
    }
    pub fn reserved_bytes(&self) -> usize {
        self.arena.reserved_bytes()
    }
    /// Ends a running search from outside: `reason` is a limit or
    /// `Cancelled`, never a terminal verdict the search did not reach.
    pub fn stop(&mut self, reason: Status) {
        debug_assert!(!matches!(
            reason,
            Status::Running | Status::Solved | Status::Exhausted
        ));
        if self.status == Status::Running {
            self.status = reason;
        }
    }
    /// The arena is full while expanding a node of cost `g`.
    fn stop_at_limit(&mut self, g: u32) {
        self.interrupted_g = Some(g);
        self.status = self.arena.limit_status();
    }
    /// Work is sliced by queue pops so a worker can yield, report, or cancel.
    /// Stale, solved and dominated pops count toward `pops` without
    /// expanding anything.
    pub fn advance(&mut self, pops: u32) {
        if self.status != Status::Running {
            return;
        }
        for _ in 0..pops {
            let Some((index, parent_h)) = self.arena.dequeue() else {
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
            // A cheaper duplicate has since taken over this state's slot.
            if self.arena.find(&node.state).1 != Some(index) {
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
            if self.best_moves().is_some_and(|best| {
                node.g >= best
                    || (self.policy.prune_popped_estimate
                        && node.g as u64 + parent_h as u64 >= best as u64)
            }) {
                continue;
            }
            self.expanded += 1;
            self.arena.close(index);
            self.reach.fill(&self.board, &node.state);
            self.deadlock
                .refresh(&node.state.boxes[..self.board.labels.len()]);
            let mut parent_group = ParentGroup::EMPTY;
            for i in 0..self.board.labels.len() {
                let from = node.state.boxes[i];
                for (d, &opposite) in OPPOSITE.iter().enumerate() {
                    let to = self.board.neighbors[from as usize][d];
                    let stand = self.board.neighbors[from as usize][opposite];
                    if to == NONE
                        || stand == NONE
                        || self.reach.blocked(to)
                        || self.reach.distance(stand) == NONE
                        || self.heuristic.dead(i, to)
                    {
                        // A box pushed onto a dead cell would only fail the
                        // estimate below, and every check in between just
                        // skips the child, so dropping it here changes no
                        // count or result.
                        continue;
                    }
                    let g = node.g + self.reach.distance(stand) as u32 + 1;
                    if self.best_moves().is_some_and(|best| g >= best) {
                        continue;
                    }
                    if self.deadlock.is_dead_after_push(&self.board, from, to) {
                        continue;
                    }
                    let mut next = node.state;
                    next.player = from;
                    next.boxes[i] = to;
                    self.board.canonicalize(&mut next);
                    let (slot, previous) = self.arena.find(&next);
                    let previous = previous.map(|previous| self.arena.node(previous));
                    if previous.is_some_and(|previous| {
                        previous.g <= g || (!self.policy.reopen_closed && previous.is_closed())
                    }) {
                        continue;
                    }
                    // A cheaper duplicate reuses the stored estimate; otherwise
                    // the parent's group is solved lazily, only once a child
                    // gets this far.
                    let known = previous.and_then(|previous| previous.known_h());
                    let Some(h) = known.or_else(|| {
                        self.heuristic.child_estimate(
                            parent_h,
                            &mut parent_group,
                            &node.state,
                            i,
                            to,
                        )
                    }) else {
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
                        direction: d as u8,
                        flags: 0,
                        h: Node::store_h(h),
                    };
                    if self.arena.is_full() {
                        // Keep a solution discovered at the exact limit.
                        if goal {
                            self.incumbent = Some(self.arena.insert(child, slot));
                        }
                        self.stop_at_limit(node.g);
                        return;
                    }
                    let id = self.arena.insert(child, slot);
                    self.arena
                        .enqueue(g as u64 + self.policy.weight as u64 * h as u64, h, id);
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
    /// Rebuilds the incumbent's full route and replays it from the start.
    /// Costs O(route) plus one flood per push, so call it once per improved
    /// incumbent.
    pub fn solution(&mut self) -> Result<Option<String>, String> {
        let Some(id) = self.incumbent else {
            return Ok(None);
        };
        let mut node = self.arena.node(id);
        let expected = node.g as usize;
        if expected > MAX_ROUTE {
            return Err(format!(
                "Solution exceeds the {MAX_ROUTE}-move replay limit"
            ));
        }
        // Direction indices, back to front: each push, then the walk before it.
        let mut route = Vec::with_capacity(expected);
        while node.parent != NIL {
            let parent = self.arena.node(node.parent);
            route.push(node.direction);
            let stand =
                self.board.neighbors[node.state.player as usize][OPPOSITE[node.direction as usize]];
            self.reach.fill(&self.board, &parent.state);
            self.reach
                .append_walk_reversed(&self.board, stand, &mut route);
            node = parent;
        }
        route.reverse();
        let mut replay = self.start;
        for &direction in &route {
            if self.board.step(&mut replay, direction as usize).is_none() {
                return Err("Internal route replay failed".into());
            }
        }
        if !self.board.solved(&replay) || route.len() != expected {
            return Err("Internal solution counters failed".into());
        }
        Ok(Some(
            route
                .iter()
                .map(|&direction| ACTIONS[direction as usize] as char)
                .collect(),
        ))
    }
}
