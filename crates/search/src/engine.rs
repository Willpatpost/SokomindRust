use crate::{
    SearchError, SearchStats, SolutionError, Status, StopReason,
    arena::{Arena, Key, MAX_QUEUED_H, NIL, Node},
    deadlock::Deadlock,
    heuristic::{Heuristic, ParentGroup},
    reach::Reach,
};
#[cfg(feature = "o3b")]
use sokomind_core::Cell;
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
    /// ([`Arena::close`]). A cheaper path to a state still open always
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
    /// Where filling the arena with no route starts over instead of ending
    /// the search: the arena is emptied in place and the start re-seeded
    /// under that policy, which then runs exactly as a fresh search.
    restart: Option<&'static Policy>,
}
// Exact soundness assumes one admissible weight for the whole search. A
// restarted policy runs to the end, so a search restarts at most once.
const _: () = {
    assert!(Policy::EXACT.then.is_none() && Policy::EXACT.restart.is_none());
    if let Some(next) = Policy::FAST_THEN_QUALITY_RESTART.restart {
        assert!(next.then.is_none() && next.restart.is_none());
    }
};
// Queue keys saturate (see `Key`), and a saturated key needs g > MAX_ROUTE
// at every weight: saturation only reorders nodes no replayable route goes
// through, and never raises a stored f above the true one. The list must
// name every policy a search can run, `then` and `restart` targets included.
const _: () = {
    let weights = [
        Policy::EXACT.weight,
        Policy::FAST.weight,
        Policy::QUALITY.weight,
        Policy::FAST_THEN_QUALITY.weight,
        Policy::FAST_THEN_QUALITY_RESTART.weight,
    ];
    let mut i = 0;
    while i < weights.len() {
        // Named first: `x as u64 < y` would parse `u64<` as generic arguments.
        let largest = MAX_ROUTE as u64 + weights[i] as u64 * MAX_QUEUED_H as u64;
        assert!(largest < Key::F_SAT);
        i += 1;
    }
};
impl Policy {
    /// Admissible A*; exact soundness never depends on consistency.
    pub(crate) const EXACT: Self = Self {
        weight: 1,
        stop_on_goal_pop: true,
        stop_on_goal_push: false,
        reopen_closed: true,
        prune_popped_estimate: false,
        then: None,
        restart: None,
    };
    /// First route wins, with no bound on its length: weighted A* without
    /// reopening keeps its suboptimality bound only under a consistent h,
    /// which the side-aware `o2` table (5.2) is not. Fast never proves.
    pub(crate) const FAST: Self = Self {
        weight: 5,
        stop_on_goal_pop: true,
        stop_on_goal_push: true,
        reopen_closed: false,
        prune_popped_estimate: true,
        then: None,
        restart: None,
    };
    /// Keeps improving the incumbent until the queue empties or a limit hits.
    pub(crate) const QUALITY: Self = Self {
        weight: 3,
        stop_on_goal_pop: false,
        stop_on_goal_push: false,
        reopen_closed: true,
        prune_popped_estimate: true,
        then: None,
        restart: None,
    };
    /// Exactly [`Self::FAST`] until its first route, then [`Self::QUALITY`]
    /// in the same arena. The incumbent only improves, so the result is never
    /// longer than Fast's, and its bound prunes from the first Quality pop.
    pub(crate) const FAST_THEN_QUALITY: Self = Self {
        then: Some(&Self::QUALITY),
        ..Self::FAST
    };
    /// Quality mode: [`Self::FAST_THEN_QUALITY`], except that when Fast
    /// fills the arena without a route the search starts over as plain
    /// [`Self::QUALITY`]. Each result is then the Fast-then-Quality result or
    /// the plain Quality one, with the same reservation.
    pub(crate) const FAST_THEN_QUALITY_RESTART: Self = Self {
        restart: Some(&Self::QUALITY),
        ..Self::FAST_THEN_QUALITY
    };
}

/// Sorts interchangeable same-label boxes so states compare canonically.
/// Labels are grouped in slot order (see [`State`]), so sorting each run of
/// equal labels keeps every box on a slot of its own label. Only call this
/// on search-owned state copies: a live `Game` must keep its box order,
/// because undo records box indices.
pub(crate) fn canonicalize(board: &Board, state: &mut State) {
    let labels = board.labels();
    let mut begin = 0;
    while begin < labels.len() {
        let mut end = begin + 1;
        while end < labels.len() && labels[end] == labels[begin] {
            end += 1;
        }
        state.boxes[begin..end].sort_unstable();
        begin = end;
    }
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
    arena: Arena,
    status: Status,
    expanded: u32,
    stats: SearchStats,
    incumbent: Option<u32>,
    /// Records a restart discarded; they still count as generated.
    discarded: u32,
    /// g + queued h (at least 1) of a node whose expansion a limit cut short.
    /// Under an admissible h, no route through its unpushed successors along
    /// this path is shorter, so the exact frontier must include it.
    interrupted_f: Option<u64>,
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
        canonicalize(&board, &mut start);
        let cells = board.tiles().len();
        let arena = Arena::new(cells, board.labels().len(), max_states, memory_mib)?;
        let heuristic = Heuristic::new(&board);
        let deadlock = Deadlock::new(&board);
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
            discarded: 0,
            interrupted_f: None,
        };
        search.seed();
        Ok(search)
    }
    /// Inserts and queues the start in an empty arena, or ends the search
    /// when the start has no goal assignment.
    fn seed(&mut self) {
        let h = self.heuristic.estimate(&self.start);
        let (slot, _) = self.arena.find(&self.start);
        let root = self.arena.insert(
            Node {
                state: self.start,
                g: 0,
                parent: NIL,
                direction: 0,
                h: h.map_or(u16::MAX, Node::store_h),
            },
            slot,
        );
        if let Some(h) = h {
            let total_h = self.root_estimate(&self.start, h);
            self.arena
                .enqueue(total_h as u64 * self.policy.weight as u64, total_h, root);
        } else {
            self.status = Status::Exhausted;
        }
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn best_moves(&self) -> Option<u32> {
        self.incumbent.map(|i| self.arena.meta(i).g)
    }
    pub fn expanded(&self) -> u32 {
        self.expanded
    }
    /// Records inserted, including any a restart discarded.
    pub fn generated(&self) -> u32 {
        self.discarded + self.arena.len() as u32
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
    /// Minimum f over everything not yet expanded, or `u64::MAX` when the
    /// queue is empty and no expansion was cut short. An interrupted
    /// expansion contributes its own f, which bounds its unpushed successors.
    /// Queue keys are `g + weight * h`, so this is a lower bound only under
    /// the exact policy's weight of 1; only [`crate::ExactSearch`] reads it.
    pub(crate) fn frontier(&self) -> u64 {
        debug_assert_eq!(self.policy.weight, 1, "weighted keys bound nothing");
        self.arena
            .min_f()
            .unwrap_or(u64::MAX)
            .min(self.interrupted_f.unwrap_or(u64::MAX))
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
    /// Starts over under the policy's restart while there is no route: the
    /// arena is emptied in place, so the rest of the run is a fresh search
    /// whose records add to the discarded ones. False when there is none.
    fn restart(&mut self) -> bool {
        let (None, Some(&next)) = (self.incumbent, self.policy.restart) else {
            return false;
        };
        self.discarded += self.arena.len() as u32;
        self.arena.clear();
        self.policy = next;
        self.seed();
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
        'pops: for _ in 0..pops {
            let Some(key) = self.arena.dequeue() else {
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
            let (index, queued_h) = (key.id(), key.h());
            // A cheaper duplicate has since taken over this state's slot.
            let superseded = self.arena.is_superseded(index);
            debug_assert_eq!(
                superseded,
                self.arena.find(&self.arena.node(index).state).1 != Some(index)
            );
            if superseded {
                self.stats.stale_pops += 1;
                continue;
            }
            let node = self.arena.node(index);
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
                    canonicalize(&self.board, &mut next);
                    let (slot, previous) = self.arena.find(&next);
                    let previous = previous.map(|previous| self.arena.meta(previous));
                    if previous.is_some_and(|previous| {
                        previous.g <= g || (!self.policy.reopen_closed && previous.closed)
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
                    // 5.6 O3b: before its first push the child's keeper walks
                    // to the stand of a statically legal push, so g + h + that
                    // walk still bounds every route through the child. A prune
                    // only: queue keys and stored estimates stay push-only.
                    #[cfg(feature = "o3b")]
                    if h > 0
                        && let Some(best) = self.best_moves()
                    {
                        // At least 1, since the prune above failed.
                        let need = best - g - h;
                        let boxes = &next.boxes[..self.board.labels().len()];
                        // The parent's flood marks its boxes, and the pushed
                        // box left `from` for `to`.
                        let occupied =
                            |cell: Cell| cell == to || (cell != from && self.reach.blocked(cell));
                        // Pushing the same box on again starts from `from`.
                        let ahead = self.board.neighbors()[to as usize][d];
                        let onward =
                            ahead != NONE && !occupied(ahead) && !self.heuristic.dead(i, ahead);
                        if !onward && self.stand_walk(from, boxes, occupied, need) >= need {
                            self.stats.pruned_bound += 1;
                            continue;
                        }
                    }
                    // Past the prune above g + h < best, so a solved child
                    // always improves the incumbent.
                    let goal = h == 0 && self.board.solved(&next);
                    let child = Node {
                        state: next,
                        g,
                        parent: index,
                        direction: d as u8,
                        h: Node::store_h(h),
                    };
                    if self.arena.is_full() {
                        // The rest of this expansion belongs to the discarded arena.
                        if !goal && self.restart() {
                            continue 'pops;
                        }
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
        // 5.6 O3b: the first push needs the keeper on its stand, not just
        // next to a box.
        #[cfg(feature = "o3b")]
        let walk = {
            let boxes = &state.boxes[..self.board.labels().len()];
            walk.max(self.stand_walk(state.player, boxes, |cell| boxes.contains(&cell), 1) as usize)
        };
        pushes + walk as u32
    }
    /// 5.6 O3b: the Manhattan distance from `player` to the nearest stand of
    /// a statically legal push, or 0 when there is none. Pushing box `j` in
    /// direction `e` is statically legal when the cell ahead of it and the
    /// stand behind it are floor that `occupied` leaves free, and the cell
    /// ahead is not dead for the box. The first push of every route from an
    /// unsolved state is one of these, made with exactly these boxes, so the
    /// walk to its stand bounds the route's walking moves, which the
    /// assignment's pushes do not count. Callers skip solved states.
    ///
    /// Stops early once the answer is known to be below `enough` (0 asks for
    /// the exact walk): the result is below `enough` exactly when the exact
    /// walk is, and equals the exact walk otherwise. O(boxes * 4).
    #[cfg(feature = "o3b")]
    fn stand_walk(
        &self,
        player: Cell,
        boxes: &[Cell],
        occupied: impl Fn(Cell) -> bool,
        enough: u32,
    ) -> u32 {
        let width = self.board.width();
        let (x, y) = (player as usize % width, player as usize / width);
        let walk = |cell: Cell| {
            (x.abs_diff(cell as usize % width) + y.abs_diff(cell as usize / width)) as u32
        };
        let neighbors = self.board.neighbors();
        let mut best = u32::MAX;
        for (j, &cell) in boxes.iter().enumerate() {
            // A stand is next to its box, so at most one step closer.
            if walk(cell) > best {
                continue;
            }
            for (e, &opposite) in OPPOSITE.iter().enumerate() {
                let ahead = neighbors[cell as usize][e];
                let stand = neighbors[cell as usize][opposite];
                if ahead == NONE
                    || stand == NONE
                    || occupied(ahead)
                    || occupied(stand)
                    || self.heuristic.dead(j, ahead)
                {
                    continue;
                }
                best = best.min(walk(stand));
                if best < enough {
                    return best;
                }
            }
        }
        if best == u32::MAX { 0 } else { best }
    }
    /// Rebuilds the incumbent's full route and replays it from the start.
    /// Costs O(route) plus one flood per push, so call it once per improved
    /// incumbent.
    pub fn solution(&mut self) -> Result<Option<String>, SolutionError> {
        let Some(id) = self.incumbent else {
            return Ok(None);
        };
        let mut node = self.arena.node(id);
        let expected = node.g as usize;
        if expected > MAX_ROUTE {
            return Err(SolutionError::TooLong);
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
                return Err(SolutionError::Replay);
            }
        }
        if !self.board.solved(&replay) || route.len() != expected {
            return Err(SolutionError::Counters);
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
    use super::{Engine, Policy, canonicalize};
    use crate::Status;
    use sokomind_core::Board;

    /// Small enough for debug builds. At this limit Fast finds 28 catalog
    /// routes and the second phase shortens 13 of them.
    const STATES: usize = 1_000;
    const MIN_IMPROVED: usize = 10;

    /// Every catalog board with its id.
    fn catalog() -> Vec<(String, Board)> {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../../data/puzzles.json")).unwrap();
        catalog
            .as_array()
            .unwrap()
            .iter()
            .map(|puzzle| {
                let rows: Vec<&str> = puzzle["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row.as_str().unwrap())
                    .collect();
                let board = Board::parse(&rows.join("\n")).unwrap();
                (puzzle["id"].as_str().unwrap().to_owned(), board)
            })
            .collect()
    }

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

    /// The result and counters two equivalent runs share.
    fn outcome(engine: &Engine) -> (Status, Option<u32>, u32, u32) {
        (
            engine.status(),
            engine.best_moves(),
            engine.expanded(),
            engine.generated(),
        )
    }

    /// Fast then Quality on the whole catalog: the first phase is Fast
    /// itself, and the second only improves on Fast's route.
    #[test]
    fn fast_then_quality_starts_as_fast_and_never_ends_longer() {
        let mut improved = 0;
        for (id, board) in catalog() {
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

    /// Checks Quality mode against its sources at one limit: the Fast then
    /// Quality run when Fast finds a route, otherwise Fast's discarded arena
    /// followed by a fresh Quality run. Returns the restarted run.
    fn check_restart(context: &str, board: &Board, max_states: usize) -> Engine {
        let (fast, _) = run(board, Policy::FAST, max_states);
        let (mut restart, _) = run(board, Policy::FAST_THEN_QUALITY_RESTART, max_states);
        let stats = restart.stats();
        assert_eq!(
            stats.unique_states + stats.duplicate_improvements,
            restart.generated(),
            "{context}"
        );
        if let Some(moves) = restart.best_moves() {
            let route = restart.solution().unwrap().unwrap();
            assert_eq!(route.len(), moves as usize, "{context}");
        }
        if fast.best_moves().is_some() {
            let (both, _) = run(board, Policy::FAST_THEN_QUALITY, max_states);
            assert_eq!(outcome(&restart), outcome(&both), "{context}");
        } else if fast.status() == Status::Exhausted {
            // No route is reachable, so there is nothing to restart for.
            assert_eq!(outcome(&restart), outcome(&fast), "{context}");
        } else {
            let (quality, _) = run(board, Policy::QUALITY, max_states);
            assert_eq!(
                outcome(&restart),
                (
                    quality.status(),
                    quality.best_moves(),
                    fast.expanded() + quality.expanded(),
                    fast.generated() + quality.generated()
                ),
                "{context}"
            );
        }
        restart
    }

    /// Quality mode on the whole catalog. Fast misses 29 boards at this
    /// limit; the restarted Quality run still finds gen-v2-320041-e16f5a47,
    /// where Fast has no route even at 1M states.
    #[test]
    fn quality_restarts_fresh_only_when_fast_fills_the_arena() {
        let mut rescued = Vec::new();
        for (id, board) in catalog() {
            let restart = check_restart(&id, &board, STATES);
            if restart.best_moves().is_some() && restart.generated() as usize > STATES + 1 {
                rescued.push(id);
            }
        }
        assert!(
            rescued.iter().any(|id| id == "gen-v2-320041-e16f5a47"),
            "{rescued:?}"
        );
    }

    /// Sweeps every limit on a small board, through the one where Fast's
    /// route takes the spare node, in slices of one pop and of 2^20.
    #[test]
    fn restart_survives_state_limit_boundaries() {
        let (id, board) = catalog().into_iter().find(|(id, _)| id == "tiny").unwrap();
        let (fast, _) = run(&board, Policy::FAST, STATES);
        let (mut restarted, mut spare) = (0, 0);
        for max_states in 1..=fast.generated() as usize + 2 {
            let context = format!("{id} at {max_states} states");
            let sliced = check_restart(&context, &board, max_states);
            let mut bulk = Engine::new(
                board.clone(),
                board.initial(),
                Policy::FAST_THEN_QUALITY_RESTART,
                max_states,
                16,
            )
            .unwrap();
            while bulk.status() == Status::Running {
                bulk.advance(1 << 20);
            }
            assert_eq!(outcome(&bulk), outcome(&sliced), "{context}");
            let generated = sliced.generated() as usize;
            restarted += usize::from(generated > max_states + 1);
            spare += usize::from(sliced.best_moves().is_some() && generated == max_states + 1);
        }
        assert!(restarted > 0 && spare > 0, "{restarted} {spare}");
    }

    /// Sorting stays inside each label group: a global sort would interleave
    /// the A and B boxes, and inactive slots keep `NONE`.
    #[test]
    fn canonicalize_sorts_each_label_group() {
        let board = Board::parse("OOOOOOO\nOR    O\nOABAB O\nOaabb O\nOOOOOOO").unwrap();
        assert_eq!(board.labels(), b"AABB");
        let start = board.initial();
        let [a1, a2, b1, b2] = [0, 1, 2, 3].map(|i| start.boxes[i]);
        assert!(a1 < b1 && b1 < a2 && a2 < b2);
        let mut state = start;
        state.boxes[..4].copy_from_slice(&[a2, a1, b2, b1]);
        canonicalize(&board, &mut state);
        assert_eq!(state, start);
    }

    /// 5.6 O3b: the stand walk against exact distances on the catalog.
    #[cfg(feature = "o3b")]
    mod o3b {
        use super::{Engine, Policy, STATES, catalog};
        use sokomind_core::{Board, Cell, NONE, State};
        use std::collections::{HashMap, VecDeque};

        /// Primitive states per board at most; 8 catalog boards fit.
        const CAP: usize = 20_000;
        /// Successor state and, for a push, the box index and direction.
        type Edge = (usize, Option<(usize, usize)>);

        /// Every primitive state reachable from the start, without
        /// expanding solved ones, or `None` past `CAP` states. Box order is
        /// the board's: estimates and walks ignore order inside a group.
        fn explore(board: &Board) -> Option<(Vec<State>, Vec<Vec<Edge>>)> {
            let key = |state: &State| (state.player, state.boxes);
            let mut states = vec![board.initial()];
            let mut index = HashMap::from([(key(&board.initial()), 0)]);
            let mut edges = Vec::new();
            while edges.len() < states.len() {
                let state = states[edges.len()];
                let mut out = Vec::new();
                for direction in 0..4 {
                    let mut next = state;
                    if board.solved(&state) || board.step(&mut next, direction).is_none() {
                        continue;
                    }
                    let pushed = (0..board.labels().len())
                        .find(|&i| next.boxes[i] != state.boxes[i])
                        .map(|i| (i, direction));
                    let id = *index.entry(key(&next)).or_insert_with(|| {
                        states.push(next);
                        states.len() - 1
                    });
                    out.push((id, pushed));
                }
                edges.push(out);
                if states.len() > CAP {
                    return None;
                }
            }
            Some((states, edges))
        }

        /// Exact moves to a solved state, `u32::MAX` without one.
        fn remaining(board: &Board, states: &[State], edges: &[Vec<Edge>]) -> Vec<u32> {
            let mut reverse = vec![Vec::new(); states.len()];
            for (from, out) in edges.iter().enumerate() {
                for &(to, _) in out {
                    reverse[to].push(from);
                }
            }
            let mut exact = vec![u32::MAX; states.len()];
            let mut queue = VecDeque::new();
            for (id, state) in states.iter().enumerate() {
                if board.solved(state) {
                    exact[id] = 0;
                    queue.push_back(id);
                }
            }
            while let Some(to) = queue.pop_front() {
                for &from in &reverse[to] {
                    if exact[from] == u32::MAX {
                        exact[from] = exact[to] + 1;
                        queue.push_back(from);
                    }
                }
            }
            exact
        }

        /// The exact walk over `boxes`, occupancy read from the boxes.
        fn walk(engine: &Engine, player: Cell, boxes: &[Cell]) -> u32 {
            engine.stand_walk(player, boxes, |cell| boxes.contains(&cell), 0)
        }

        /// (h, h') with h' = h + walk, and 0 when solved; `None` without an
        /// assignment.
        fn estimates(engine: &Engine, state: &State) -> Option<(u32, u32)> {
            let h = engine.heuristic.estimate(state)?;
            if h == 0 {
                return Some((0, 0));
            }
            let boxes = &state.boxes[..engine.board.labels().len()];
            Some((h, h + walk(engine, state.player, boxes)))
        }

        /// Admissible: never above the exact remaining moves. Consistent: a
        /// move lowers h' by at most 1 wherever it lowers h by at most 1,
        /// which with the plain table is every move (3.3); the o2 table alone
        /// may break that. On every catalog board whose primitive state space
        /// fits `CAP`.
        #[test]
        fn stand_walk_is_admissible_and_consistent() {
            let mut checked = Vec::new();
            for (id, board) in catalog() {
                let Some((states, edges)) = explore(&board) else {
                    continue;
                };
                let engine =
                    Engine::new(board.clone(), board.initial(), Policy::EXACT, STATES, 16).unwrap();
                let exact = remaining(&board, &states, &edges);
                let values: Vec<_> = states
                    .iter()
                    .map(|state| estimates(&engine, state))
                    .collect();
                for (from, state) in states.iter().enumerate() {
                    let Some((h, value)) = values[from] else {
                        assert_eq!(exact[from], u32::MAX, "{id}: no assignment at {state:?}");
                        continue;
                    };
                    assert!(
                        value <= exact[from],
                        "{id}: {value} > {} at {state:?}",
                        exact[from]
                    );
                    for &(to, _) in &edges[from] {
                        if let Some((next_h, next)) = values[to]
                            && h <= next_h + 1
                        {
                            assert!(value <= next + 1, "{id}: {value} then {next} at {state:?}");
                        }
                    }
                }
                checked.push(id);
            }
            assert_eq!(checked.len(), 8, "{checked:?}");
        }

        /// The child prune's inputs: occupancy from the parent's flood plus
        /// the pushed box gives the child's own walk, an early stop only
        /// answers "below enough", and the onward exit only skips a walk
        /// of 0.
        #[test]
        fn stand_walk_after_a_push_matches_a_fresh_scan() {
            let (mut pushes, mut onward) = (0, 0);
            for (id, board) in catalog() {
                let Some((states, edges)) = explore(&board) else {
                    continue;
                };
                let mut engine =
                    Engine::new(board.clone(), board.initial(), Policy::EXACT, STATES, 16).unwrap();
                let n = board.labels().len();
                for (parent, state) in states.iter().enumerate() {
                    engine.reach.fill(&engine.board, state);
                    for &(child, push) in &edges[parent] {
                        let Some((i, d)) = push else {
                            continue;
                        };
                        let (from, to) = (state.boxes[i], states[child].boxes[i]);
                        let boxes = &states[child].boxes[..n];
                        let occupied =
                            |cell: Cell| cell == to || (cell != from && engine.reach.blocked(cell));
                        let exact = walk(&engine, from, boxes);
                        for enough in 0..=exact + 1 {
                            let got = engine.stand_walk(from, boxes, occupied, enough);
                            assert_eq!(
                                got < enough,
                                exact < enough,
                                "{id}: {got} {exact} {enough}"
                            );
                            assert!(got < enough || got == exact, "{id}: {got} {exact} {enough}");
                        }
                        let ahead = board.neighbors()[to as usize][d];
                        if ahead != NONE && !occupied(ahead) && !engine.heuristic.dead(i, ahead) {
                            assert_eq!(exact, 0, "{id} at {:?}", states[child]);
                            onward += 1;
                        }
                        pushes += 1;
                    }
                }
            }
            assert!(onward > 0 && onward < pushes, "{onward} {pushes}");
        }

        /// The root estimate is pushes + max(box walk, stand walk). It rises
        /// over pushes + box walk by exactly these gains, on exactly these
        /// catalog boards (o3b_root.txt), under o2 too (o3b_root_o2.txt). The
        /// gain reads only cells and dead masks, not the push count.
        #[test]
        fn o3b_root_estimates_match_prevalidation() {
            const RAISED: [(&str, u32); 7] = [
                ("tutorial-push", 2),
                ("beginner-detour", 2),
                ("garden-2", 2),
                ("classic-1", 2),
                ("adv-gallery", 2),
                ("theme-parking", 2),
                ("expert-maze", 2),
            ];
            let mut raised = 0;
            for (id, board) in catalog() {
                let engine =
                    Engine::new(board.clone(), board.initial(), Policy::EXACT, STATES, 16).unwrap();
                let start = engine.start;
                let pushes = engine.heuristic.estimate(&start).unwrap();
                let width = board.width();
                let (x, y) = (start.player as usize % width, start.player as usize / width);
                let box_walk = start.boxes[..board.labels().len()]
                    .iter()
                    .map(|&cell| {
                        x.abs_diff(cell as usize % width) + y.abs_diff(cell as usize / width) - 1
                    })
                    .min()
                    .unwrap() as u32;
                let gain = RAISED
                    .iter()
                    .find(|&&(raised_id, _)| raised_id == id)
                    .map_or(0, |&(_, gain)| gain);
                let root = engine.root_estimate(&start, pushes);
                assert_eq!(root, pushes + box_walk + gain, "{id}");
                raised += usize::from(root > pushes + box_walk);
            }
            assert_eq!(raised, RAISED.len());
        }
    }
}
