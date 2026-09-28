use crate::{
    SearchError, SearchStats, Status, StopReason,
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
    /// Where a goal stop hands over instead of ending the search: the
    /// search continues in the same arena under that policy, with the queue
    /// re-keyed at its weight.
    then: Option<&'static Policy>,
}
// Exact soundness assumes one admissible weight for the whole search.
const _: () = assert!(Policy::EXACT.then.is_none());
impl Policy {
    /// Admissible A*; exact soundness never depends on consistency.
    pub(crate) const EXACT: Self = Self {
        weight: 1,
        stop_on_goal_pop: true,
        stop_on_goal_push: false,
        reopen_closed: true,
        prune_popped_estimate: false,
        then: None,
    };
    /// First route wins. Without reopening, weighted A* keeps its
    /// suboptimality bound under a consistent h, and never proves anyway.
    pub(crate) const FAST: Self = Self {
        weight: 5,
        stop_on_goal_pop: true,
        stop_on_goal_push: true,
        reopen_closed: false,
        prune_popped_estimate: true,
        then: None,
    };
    /// Keeps improving the incumbent until the queue empties or a limit hits.
    pub(crate) const QUALITY: Self = Self {
        weight: 3,
        stop_on_goal_pop: false,
        stop_on_goal_push: false,
        reopen_closed: true,
        prune_popped_estimate: true,
        then: None,
    };
    /// Experiment 5.1 (O5), behind the `o5` feature: exactly [`Self::FAST`]
    /// until its first route, then [`Self::QUALITY`] in the same arena. The
    /// incumbent only improves, so the result is never longer than Fast's,
    /// and its bound prunes from the first Quality pop.
    pub(crate) const FAST_THEN_QUALITY: Self = Self {
        then: Some(&Self::QUALITY),
        ..Self::FAST
    };
}

/// Push search over one reserved arena. Only [`crate::ExactSearch`] turns its
/// state into bounds or a proof; with a weighted policy results are always
/// optimality unknown.
pub(crate) struct Engine {
    policy: Policy,
    board: Board,
    start: State,
    heuristic: Heuristic,
    reach: Reach,
    deadlock: Deadlock,
    pub(crate) arena: Arena,
    status: Status,
    expanded: u32,
    stats: SearchStats,
    incumbent: Option<u32>,
    /// g + queued h (at least 1) of a node whose expansion a limit cut short.
    /// Under an admissible h, no route through its unpushed successors along
    /// this path is shorter, so the exact frontier must include it.
    pub(crate) interrupted_f: Option<u64>,
}

impl Engine {
    pub(crate) fn new(
        board: Board,
        mut start: State,
        policy: Policy,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, SearchError> {
        board
            .validate_state(&start)
            .map_err(SearchError::InvalidState)?;
        board.canonicalize(&mut start);
        let cells = board.tiles().len();
        let arena = Arena::new(cells, board.labels().len(), max_states, memory_mib)
            .map_err(SearchError::Configuration)?;
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
            stats: SearchStats::default(),
            incumbent: None,
            interrupted_f: None,
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
            let total_h = search.root_estimate(&start, h);
            search
                .arena
                .enqueue(total_h as u64 * policy.weight as u64, total_h, root);
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
    pub fn stats(&self) -> SearchStats {
        let arena = self.arena.stats();
        SearchStats {
            unique_states: arena.unique_states,
            duplicate_improvements: arena.duplicate_improvements,
            reopened_states: arena.reopened_states,
            peak_queue: arena.peak_queue,
            ..self.stats
        }
    }
    /// Ends a running search from outside: `reason` is a limit or
    /// `Cancelled`, never a terminal verdict the search did not reach.
    pub fn stop(&mut self, reason: StopReason) {
        if self.status == Status::Running {
            self.status = match reason {
                StopReason::Cancelled => Status::Cancelled,
                StopReason::TimeLimit => Status::TimeLimit,
            };
        }
    }
    /// Hands a goal stop over to the policy's next phase, re-keying the queue
    /// for its weight. False when there is none and the search ends.
    fn next_phase(&mut self) -> bool {
        let Some(&next) = self.policy.then else {
            return false;
        };
        self.arena.reweight(self.policy.weight, next.weight);
        self.policy = next;
        true
    }
    /// The arena is full while expanding a node whose g + queued h is `f`.
    fn stop_at_limit(&mut self, f: u64) {
        self.interrupted_f = Some(f);
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
            let Some((index, queued_h)) = self.arena.dequeue() else {
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
                self.stats.stale_pops += 1;
                continue;
            }
            if self.board.solved(&node.state) {
                if self.best_moves().is_none_or(|best| node.g < best) {
                    self.incumbent = Some(index);
                }
                if self.policy.stop_on_goal_pop && !self.next_phase() {
                    self.status = Status::Solved;
                    return;
                }
                continue;
            }
            if self.best_moves().is_some_and(|best| {
                node.g >= best
                    || (self.policy.prune_popped_estimate
                        && node.g as u64 + queued_h as u64 >= best as u64)
            }) {
                self.stats.pruned_bound += 1;
                continue;
            }
            self.expanded += 1;
            self.arena.close(index);
            self.reach.fill(&self.board, &node.state);
            self.deadlock
                .refresh(&node.state.boxes[..self.board.labels().len()]);
            let parent_h = node.known_h().unwrap_or_else(|| {
                self.heuristic
                    .estimate(&node.state)
                    .expect("queued state has an assignment")
            });
            let mut parent_group = ParentGroup::EMPTY;
            for i in 0..self.board.labels().len() {
                let from = node.state.boxes[i];
                for (d, &opposite) in OPPOSITE.iter().enumerate() {
                    let to = self.board.neighbors()[from as usize][d];
                    let stand = self.board.neighbors()[from as usize][opposite];
                    if to == NONE
                        || stand == NONE
                        || self.reach.blocked(to)
                        || self.reach.distance(stand) == NONE
                    {
                        // A box pushed onto a dead cell would only fail the
                        // estimate below, and every check in between just
                        // skips the child, so dropping it here changes no
                        // count or result.
                        continue;
                    }
                    if self.heuristic.dead(i, to) {
                        self.stats.pruned_dead_cells += 1;
                        continue;
                    }
                    let g = node.g + self.reach.distance(stand) as u32 + 1;
                    if self.best_moves().is_some_and(|best| g >= best) {
                        self.stats.pruned_bound += 1;
                        continue;
                    }
                    if self.deadlock.is_dead_after_push(&self.board, from, to) {
                        self.stats.pruned_deadlocks += 1;
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
                        self.stats.pruned_duplicates += 1;
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
                        self.stats.pruned_assignment += 1;
                        continue;
                    };
                    if self
                        .best_moves()
                        .is_some_and(|best| g as u64 + h as u64 >= best as u64)
                    {
                        self.stats.pruned_bound += 1;
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
                        // An expanded node is unsolved, so at least one move remains.
                        self.stop_at_limit(node.g as u64 + queued_h.max(1) as u64);
                        return;
                    }
                    let id = self.arena.insert(child, slot);
                    self.arena
                        .enqueue(g as u64 + self.policy.weight as u64 * h as u64, h, id);
                    // Keep a solution even if a limit occurs before its pop.
                    if goal {
                        self.incumbent = Some(id);
                        // A next phase takes over the rest of this expansion.
                        if self.policy.stop_on_goal_push && !self.next_phase() {
                            self.status = Status::Solved;
                            return;
                        }
                    }
                }
            }
        }
    }
    /// Every unsolved route walks to a box before its first push. Ignoring
    /// walls and all other boxes can only shorten that walk. These walking
    /// moves are disjoint from the assignment's required pushes, so they add
    /// to its admissible estimate. Keep assignment costs separately cached.
    /// Only the root needs this: a pushed child's player stands next to the
    /// box it just pushed, so the walk term is always 0 there.
    fn root_estimate(&self, state: &State, pushes: u32) -> u32 {
        if pushes == 0 && self.board.solved(state) {
            return 0;
        }
        let width = self.board.width();
        let x = state.player as usize % width;
        let y = state.player as usize / width;
        let walk = state.boxes[..self.board.labels().len()]
            .iter()
            .map(|&cell| x.abs_diff(cell as usize % width) + y.abs_diff(cell as usize / width) - 1)
            .min()
            .unwrap_or(0);
        pushes + walk as u32
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
            let stand = self.board.neighbors()[node.state.player as usize]
                [OPPOSITE[node.direction as usize]];
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

#[cfg(test)]
mod tests {
    use super::{Engine, Policy};
    use crate::Status;
    use sokomind_core::Board;

    /// Small enough for debug builds. At this limit Fast finds 28 catalog
    /// routes and the second phase shortens 13 of them.
    const STATES: usize = 1_000;
    const MIN_IMPROVED: usize = 10;

    /// Runs `policy` to a terminal status one pop at a time. Also returns
    /// the moves and expanded count when a route first appeared.
    fn run(board: &Board, policy: Policy, max_states: usize) -> (Engine, Option<(u32, u32)>) {
        let mut engine =
            Engine::new(board.clone(), board.initial(), policy, max_states, 16).unwrap();
        let mut first = None;
        while engine.status() == Status::Running {
            engine.advance(1);
            if first.is_none() {
                first = engine.best_moves().map(|moves| (moves, engine.expanded()));
            }
        }
        (engine, first)
    }

    /// Experiment 5.1 (O5) on the whole catalog: the first phase is Fast
    /// itself, and the second only improves on Fast's route.
    #[test]
    fn fast_then_quality_starts_as_fast_and_never_ends_longer() {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../../data/puzzles.json")).unwrap();
        let mut improved = 0;
        for puzzle in catalog.as_array().unwrap() {
            let id = puzzle["id"].as_str().unwrap();
            let rows: Vec<&str> = puzzle["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row.as_str().unwrap())
                .collect();
            let board = Board::parse(&rows.join("\n")).unwrap();
            let (fast, _) = run(&board, Policy::FAST, STATES);
            let (mut both, first) = run(&board, Policy::FAST_THEN_QUALITY, STATES);
            let Some(moves) = fast.best_moves() else {
                // No route, no second phase: the runs are the same.
                assert_eq!(
                    (both.status(), both.best_moves()),
                    (fast.status(), None),
                    "{id}"
                );
                assert_eq!(
                    (both.expanded(), both.generated()),
                    (fast.expanded(), fast.generated()),
                    "{id}"
                );
                continue;
            };
            assert_eq!(first, Some((moves, fast.expanded())), "{id}");
            let best = both.best_moves().unwrap();
            assert!(best <= moves, "{id}: {best} > {moves}");
            improved += usize::from(best < moves);
            let route = both.solution().unwrap().unwrap();
            assert_eq!(route.len(), best as usize, "{id}");
        }
        assert!(improved >= MIN_IMPROVED, "{improved}");
    }
}
