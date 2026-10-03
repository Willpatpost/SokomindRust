use crate::{
    SearchError, SearchStats, SolutionError, Status, StopReason,
    arena::{Arena, Key, MAX_QUEUED_H, NIL, Node, TableCounters},
    corral::Corral,
    deadlock::Deadlock,
    heuristic::{Heuristic, ParentGroup},
    push::{self, Child, Kernel, Parent, Prunes, Push, SkipCounters, canonicalize},
    reach::Reach,
};
use sokomind_core::{ACTIONS, Board, MAX_ROUTE, OPPOSITE, State};
use std::ops::ControlFlow;

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
    /// Where filling the arena starts over instead of ending the search:
    /// the arena is emptied in place, except for the incumbent's path, and
    /// the start re-seeded under that policy, which then runs as a fresh
    /// search with the incumbent, if any, as the bound to beat.
    restart: Option<&'static Policy>,
}
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
    /// First route wins, with no bound on its length. Fast never proves.
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
    /// Quality mode's second phase: [`Self::QUALITY`], except that when its
    /// arena fills the search starts over as plain [`Self::QUALITY`],
    /// bounded by the route it has.
    pub(crate) const QUALITY_RESTART: Self = Self {
        restart: Some(&Self::QUALITY),
        ..Self::QUALITY
    };
    /// Quality mode: [`Self::FAST_THEN_QUALITY`], except that the first time
    /// the arena fills, in either phase and with a route or without, the
    /// search starts over once as plain [`Self::QUALITY`] in the emptied
    /// arena, which keeps only its route's path, as the bound to beat. Fast
    /// then Quality would have stopped at that same fill with that same
    /// route, so the result is never longer than Fast then Quality's; when
    /// Fast fills the arena without a route, it is the plain Quality result.
    /// When the route's path alone leaves no room for the start, it stops
    /// where Fast then Quality does.
    pub(crate) const FAST_THEN_QUALITY_RESTART: Self = Self {
        then: Some(&Self::QUALITY_RESTART),
        restart: Some(&Self::QUALITY),
        ..Self::FAST
    };
    /// Every policy a search can run. The const asserts below read it, so a
    /// new policy must be added here: they check that every `then` and
    /// `restart` target is listed, but cannot see which policies modes start
    /// with.
    pub(crate) const ALL: [Self; 6] = [
        Self::EXACT,
        Self::FAST,
        Self::QUALITY,
        Self::FAST_THEN_QUALITY,
        Self::QUALITY_RESTART,
        Self::FAST_THEN_QUALITY_RESTART,
    ];
}
// Exact soundness assumes one admissible weight for the whole search.
const _: () = assert!(Policy::EXACT.then.is_none() && Policy::EXACT.restart.is_none());
const _: () = assert!(Policy::EXACT.weight == 1);
// Mode's docs (lib.rs) call Quality's weight lighter than Fast's, and the
// arena's key-width test takes Fast's as the largest.
const _: () = assert!(Policy::QUALITY.weight < Policy::FAST.weight);
// Every `then` and `restart` target is itself in `Policy::ALL`, so the
// checks that loop over it cover every policy a search can switch to. A
// restarted policy has no next phase and no restart of its own, so it runs
// to the end and a search restarts at most once.
const _: () = {
    let all = Policy::ALL;
    let mut i = 0;
    while i < all.len() {
        assert!(listed(all[i].then) && listed(all[i].restart));
        if let Some(next) = all[i].restart {
            assert!(next.then.is_none() && next.restart.is_none());
        }
        i += 1;
    }
};
// Queue keys saturate (see `Key`), and a saturated key needs g > MAX_ROUTE
// at every weight: saturation only reorders nodes no replayable route goes
// through, and never raises a stored f above the true one.
const _: () = {
    let all = Policy::ALL;
    let mut i = 0;
    while i < all.len() {
        // Named first: `x as u64 < y` would parse `u64<` as generic arguments.
        let largest = MAX_ROUTE as u64 + all[i].weight as u64 * MAX_QUEUED_H as u64;
        assert!(largest < Key::F_SAT);
        i += 1;
    }
};

/// Whether `target` is `None` or equals a policy in [`Policy::ALL`].
const fn listed(target: Option<&Policy>) -> bool {
    let all = Policy::ALL;
    let mut i = 0;
    while i < all.len() {
        if same(target, Some(&all[i])) {
            return true;
        }
        i += 1;
    }
    target.is_none()
}

/// Field-by-field equality, since const code cannot call `PartialEq`. The
/// destructure names every field, so a new one fails to compile until it is
/// compared here.
const fn same(a: Option<&Policy>, b: Option<&Policy>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            let Policy {
                weight,
                stop_on_goal_pop,
                stop_on_goal_push,
                reopen_closed,
                prune_popped_estimate,
                then,
                restart,
            } = *a;
            weight == b.weight
                && stop_on_goal_pop == b.stop_on_goal_pop
                && stop_on_goal_push == b.stop_on_goal_push
                && reopen_closed == b.reopen_closed
                && prune_popped_estimate == b.prune_popped_estimate
                && same(then, b.then)
                && same(restart, b.restart)
        }
        (None, None) => true,
        _ => false,
    }
}

/// Push search over one budgeted arena. Only [`crate::ExactSearch`] turns its
/// state into bounds or a proof; with a weighted policy results are always
/// optimality unknown.
pub(crate) struct Engine {
    policy: Policy,
    board: Board,
    start: State,
    heuristic: Heuristic,
    reach: Reach,
    deadlock: Deadlock,
    corral: Corral,
    /// Always [`Prunes::ALL`] outside tests.
    prunes: Prunes,
    arena: Arena,
    status: Status,
    expanded: u32,
    skipped: SkipCounters,
    incumbent: Option<u32>,
    /// Records a restart dropped; they still count as generated. The
    /// incumbent's path stays in the arena, so it counts there, once.
    discarded: u32,
    /// g + queued h (at least 1) of a node whose expansion a limit cut short.
    /// Under an admissible h, no route through its unpushed successors along
    /// this path is shorter, so the exact frontier must include it.
    interrupted_f: Option<u64>,
}

/// What [`Engine::pop`] made of one queue entry.
enum Popped {
    /// An unsolved node within the bound, not yet counted or closed.
    Expand(Parent),
    /// A stale, solved or dominated entry; the search goes on.
    Skip,
    /// The search ended: the queue emptied or a solved pop stopped it.
    Stop,
}

/// What [`Engine::insert`] did with an admitted child.
enum Inserted {
    /// Stored and queued; the expansion goes on.
    Queued,
    /// The arena was full and the search started over, dropping the rest
    /// of its parent's expansion. A solved child was kept first and is the
    /// incumbent the restart carries; any other child is dropped too.
    Restarted,
    /// The search ended: a limit or a solved push stopped it.
    Stopped,
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
        let corral = Corral::new(&board);
        let mut search = Self {
            policy,
            reach: Reach::new(cells),
            arena,
            board,
            start,
            heuristic,
            deadlock,
            corral,
            prunes: Prunes::ALL,
            status: Status::Running,
            expanded: 0,
            skipped: SkipCounters::default(),
            incumbent: None,
            discarded: 0,
            interrupted_f: None,
        };
        search.seed();
        Ok(search)
    }
    /// Inserts and queues the start in an arena that is empty or holds only
    /// a restart's kept path, or ends the search when the start has no goal
    /// assignment. No kept record is in the table, so the start is always a
    /// new state.
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
        // An empty arena, or one cleared at its limit, has room for the
        // start and the next insert already.
        debug_assert!(!self.arena.starved());
        if let Some(h) = h {
            let total_h = push::root_estimate(&self.board, &self.heuristic, &self.start, h);
            self.arena
                .enqueue(total_h as u64 * self.policy.weight as u64, total_h, root);
        } else {
            self.status = Status::Exhausted;
        }
    }
    pub(crate) fn status(&self) -> Status {
        self.status
    }
    pub(crate) fn best_moves(&self) -> Option<u32> {
        self.incumbent.map(|i| self.arena.meta(i).g)
    }
    pub(crate) fn expanded(&self) -> u32 {
        self.expanded
    }
    /// Records inserted, including any a restart discarded.
    pub(crate) fn generated(&self) -> u32 {
        self.discarded + self.arena.len() as u32
    }
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.arena.reserved_bytes()
    }
    /// The arena's counters and the engine's in one value. The destructures
    /// and the literal name every field, so a counter added to any of the
    /// three structs fails to compile until it is wired here.
    pub(crate) fn stats(&self) -> SearchStats {
        let TableCounters {
            unique_states,
            duplicate_improvements,
            reopened_states,
            peak_queue,
        } = *self.arena.counters();
        let SkipCounters {
            stale_pops,
            pruned_dead_cells,
            pruned_deadlocks,
            pruned_duplicates,
            pruned_assignment,
            pruned_bound,
            pruned_corrals,
        } = self.skipped;
        SearchStats {
            unique_states,
            duplicate_improvements,
            reopened_states,
            stale_pops,
            peak_queue,
            pruned_dead_cells,
            pruned_deadlocks,
            pruned_duplicates,
            pruned_assignment,
            pruned_bound,
            pruned_corrals,
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
    pub(crate) fn stop(&mut self, reason: StopReason) {
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
    /// Starts over under the policy's restart: the arena is emptied in
    /// place but for the incumbent's path, so the rest of the run is a fresh
    /// search bounded by the incumbent, whose records add to the dropped
    /// ones. False when there is no restart, or when the path would leave no
    /// room below the limit for the start, which a fresh search needs.
    fn restart(&mut self) -> bool {
        let Some(&next) = self.policy.restart else {
            return false;
        };
        let before = self.arena.len() as u32;
        if let Some(id) = self.incumbent {
            let Some(kept) = self.arena.clear_keeping(id) else {
                return false;
            };
            self.incumbent = Some(kept);
        } else {
            self.arena.clear();
        }
        // The kept path is still in the arena, so generated counts it once.
        self.discarded += before - self.arena.len() as u32;
        self.policy = next;
        self.seed();
        true
    }
    /// The arena is full, or could not grow, while expanding a node whose
    /// g + queued h is `f`.
    fn stop_at_limit(&mut self, f: u64) {
        self.interrupted_f = Some(f);
        self.status = self.arena.limit_status();
    }
    /// Work is sliced by queue pops so a worker can yield, report, or cancel.
    /// Stale, solved and dominated pops count toward `pops` without
    /// expanding anything.
    pub(crate) fn advance(&mut self, pops: u32) {
        if self.status != Status::Running {
            return;
        }
        // Each pop runs these steps in order, and the first check that
        // rejects something skips the rest for it:
        // - pop: an empty queue ends the search. A stale entry (stale_pops)
        //   is skipped, a solved node is recorded and skipped or ends the
        //   search, and a node the bound prunes (pruned_bound) is skipped.
        // - expand: count and close the node, flood its keeper, refresh the
        //   deadlock occupancy, and skip every push of a state with a
        //   sealed corral (pruned_corrals). Otherwise try every push,
        //   box-major and in direction order. A push into a wall or a box,
        //   or from a stand the keeper cannot reach, is dropped uncounted.
        // - admit, per push: dead cell (pruned_dead_cells), g bound
        //   (pruned_bound), freeze deadlock (pruned_deadlocks), then the
        //   settled child's duplicate (pruned_duplicates), assignment
        //   (pruned_assignment), g + h bound and stand walk (pruned_bound).
        // - insert: store and queue the child and record a solved one. A
        //   full arena starts the search over or ends it, and so may a
        //   solved push.
        for _ in 0..pops {
            let parent = match self.pop() {
                Popped::Expand(parent) => parent,
                Popped::Skip => continue,
                Popped::Stop => return,
            };
            if self.expand(parent).is_break() {
                return;
            }
        }
    }
    /// Dequeues one entry and says what to do with it: expand its node,
    /// skip it, or stop because the search ended.
    #[inline]
    fn pop(&mut self) -> Popped {
        let Some(key) = self.arena.dequeue() else {
            // The queue emptied: every state was popped, dominated, or
            // pruned by an admissible rule. Under the exact policy that
            // makes the incumbent optimal.
            self.status = if self.incumbent.is_some() {
                Status::Solved
            } else {
                Status::Exhausted
            };
            return Popped::Stop;
        };
        let (index, queued_h) = (key.id(), key.h());
        // A cheaper duplicate has since taken over this state's slot.
        let superseded = self.arena.is_superseded(index);
        debug_assert_eq!(
            superseded,
            self.arena.find(&self.arena.node(index).state).1 != Some(index)
        );
        if superseded {
            self.skipped.stale_pops += 1;
            return Popped::Skip;
        }
        let node = self.arena.node(index);
        if self.board.solved(&node.state) {
            if self.best_moves().is_none_or(|best| node.g < best) {
                self.incumbent = Some(index);
            }
            if self.policy.stop_on_goal_pop && !self.next_phase() {
                self.status = Status::Solved;
                return Popped::Stop;
            }
            return Popped::Skip;
        }
        if self.best_moves().is_some_and(|best| {
            node.g >= best
                || (self.policy.prune_popped_estimate
                    && node.g as u64 + queued_h as u64 >= best as u64)
        }) {
            self.skipped.pruned_bound += 1;
            return Popped::Skip;
        }
        Popped::Expand(Parent {
            index,
            node,
            queued_h,
        })
    }
    /// Expands a popped node: each push the static check ([`push::legal`])
    /// passes goes to [`Self::admit`], and each child it admits to
    /// [`Self::insert`], unless the sealed-corral check finds no solution
    /// from the node, which then gets no children. Breaks when the search
    /// ended during the expansion.
    #[inline]
    fn expand(&mut self, parent: Parent) -> ControlFlow<()> {
        self.expanded += 1;
        self.arena.close(parent.index);
        self.reach.fill(&self.board, &parent.node.state);
        let boxes = &parent.node.state.boxes[..self.board.labels().len()];
        self.deadlock.refresh(boxes);
        if self.prunes.corral
            && self.corral.is_dead(
                &self.board,
                boxes,
                &self.reach,
                &self.deadlock,
                &self.heuristic,
                self.prunes.dead_pair.then_some(&self.heuristic),
            )
        {
            // No child of a state without a solution has one; the node
            // stays expanded and closed, so it counts as before.
            self.skipped.pruned_corrals += 1;
            return ControlFlow::Continue(());
        }
        let parent_h = parent.node.known_h().unwrap_or_else(|| {
            self.heuristic
                .estimate(&parent.node.state)
                .expect("queued state has an assignment")
        });
        let mut parent_group = ParentGroup::EMPTY;
        // The bound and the reopen flag, read again after each insert, the
        // only call here that changes the incumbent or the policy: it can
        // record a shorter route, or hand a solved push to a next phase and
        // still queue it.
        let mut best = self.best_moves();
        let mut reopen_closed = self.policy.reopen_closed;
        for i in 0..self.board.labels().len() {
            let from = parent.node.state.boxes[i];
            for d in 0..4 {
                let Some(push) = push::legal(&self.board, &self.reach, i, from, d) else {
                    continue;
                };
                let Some(child) = self.admit(
                    &parent,
                    parent_h,
                    &mut parent_group,
                    push,
                    reopen_closed,
                    best,
                ) else {
                    continue;
                };
                match self.insert(&parent, child) {
                    Inserted::Queued => {
                        best = self.best_moves();
                        reopen_closed = self.policy.reopen_closed;
                    }
                    // The arena was emptied but for the incumbent's path;
                    // the next pop takes the re-seeded start.
                    Inserted::Restarted => return ControlFlow::Continue(()),
                    Inserted::Stopped => return ControlFlow::Break(()),
                }
            }
        }
        ControlFlow::Continue(())
    }
    /// Runs one push through [`push::admit`] with a [`Kernel`] of this
    /// engine's parts, lent per push because each insert needs the whole
    /// engine. `reopen_closed` and `best` are the values [`Self::expand`]
    /// keeps current across its inserts.
    #[inline]
    fn admit(
        &mut self,
        parent: &Parent,
        parent_h: u32,
        parent_group: &mut ParentGroup,
        push: Push,
        reopen_closed: bool,
        best: Option<u32>,
    ) -> Option<Child> {
        debug_assert_eq!(
            (reopen_closed, best),
            (self.policy.reopen_closed, self.best_moves())
        );
        let mut kernel = Kernel {
            board: &self.board,
            heuristic: &self.heuristic,
            reach: &self.reach,
            deadlock: &self.deadlock,
            arena: &self.arena,
            prunes: self.prunes,
            skipped: &mut self.skipped,
        };
        push::admit(
            &mut kernel,
            parent,
            parent_h,
            parent_group,
            push,
            reopen_closed,
            best,
        )
    }
    /// Stores and queues an admitted child, recording it when it is solved.
    /// At a full arena a solved child is kept as the incumbent, in the spare
    /// slot, and then the search starts over or ends. Below the limit a
    /// solved child may end it, and so does a failed growth allocation,
    /// with [`Status::MemoryLimit`].
    #[inline]
    fn insert(&mut self, parent: &Parent, child: Child) -> Inserted {
        let Child { node, h, slot } = child;
        // admit's g + h prune left g + h < best, so a solved child always
        // improves the incumbent.
        let goal = h == 0 && self.board.solved(&node.state);
        if self.arena.is_full() {
            // Keep a solution discovered at the exact limit, for the restart
            // to carry or the stop to report.
            if goal {
                self.incumbent = Some(self.arena.insert(node, slot));
            }
            // The rest of this expansion belongs to the discarded arena.
            if self.restart() {
                return Inserted::Restarted;
            }
            // An expanded node is unsolved, so at least one move remains.
            self.stop_at_limit(parent.node.g as u64 + parent.queued_h.max(1) as u64);
            return Inserted::Stopped;
        }
        let id = self.arena.insert(node, slot);
        self.arena
            .enqueue(node.g as u64 + self.policy.weight as u64 * h as u64, h, id);
        // Keep a solution even if a limit occurs before its pop.
        if goal {
            self.incumbent = Some(id);
            // A next phase takes over the rest of this expansion.
            if self.policy.stop_on_goal_push && !self.next_phase() {
                self.status = Status::Solved;
                return Inserted::Stopped;
            }
        }
        // The arena could not grow for the next insert, and its table may be
        // gone, so the search ends here, before another find, as at a full
        // arena but with no restart: a fresh search would need the same
        // allocation.
        if self.arena.starved() {
            self.stop_at_limit(parent.node.g as u64 + parent.queued_h.max(1) as u64);
            return Inserted::Stopped;
        }
        Inserted::Queued
    }
    /// Rebuilds the incumbent's full route and replays it from the start.
    /// Costs O(route) plus one flood per push, so call it once per improved
    /// incumbent.
    pub(crate) fn solution(&mut self) -> Result<Option<String>, SolutionError> {
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
impl Engine {
    /// Replaces [`Prunes::ALL`] with `prunes`, the only way to turn a
    /// detector off. Seeding runs no detector, so on an engine that has not
    /// expanded anything the new flags govern the whole run. A setter rather
    /// than a constructor argument, so it also reaches the engine inside
    /// [`crate::ExactSearch`].
    fn set_prunes(&mut self, prunes: Prunes) {
        assert_eq!(
            self.expanded, 0,
            "flags change only before the first expansion"
        );
        self.prunes = prunes;
    }
}

#[cfg(test)]
mod tests {
    use super::{Engine, Policy};
    use crate::{
        SearchStats, Status,
        exact::ExactSearch,
        heuristic::Heuristic,
        push::Prunes,
        testkit::{Lcg, catalog, explored_catalog, random_room},
    };
    use sokomind_core::{Board, Game};

    /// Small enough for debug builds. At this limit Fast finds 30 catalog
    /// routes, and the second phase must shorten at least `MIN_IMPROVED` of
    /// them.
    const STATES: usize = 1_000;
    const MIN_IMPROVED: usize = 10;

    /// Runs `policy` to a terminal status one pop at a time. Also returns
    /// the moves and expanded count when a route first appeared.
    fn run(board: &Board, policy: Policy, max_states: usize) -> (Engine, Option<(u32, u32)>) {
        let mut engine =
            Engine::new(board.clone(), board.initial(), policy, max_states, 16).unwrap();
        let first = drive(&mut engine).map(|(moves, expanded, _)| (moves, expanded));
        (engine, first)
    }

    /// Advances `engine` to a terminal status one pop at a time. Returns the
    /// moves, expanded and generated counts when a route first appeared,
    /// read after the pop that found it.
    fn drive(engine: &mut Engine) -> Option<(u32, u32, u32)> {
        let mut first = None;
        while engine.status() == Status::Running {
            engine.advance(1);
            if first.is_none() {
                first = engine
                    .best_moves()
                    .map(|moves| (moves, engine.expanded(), engine.generated()));
            }
        }
        first
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

    /// How a Quality run relates to the Fast then Quality run at the same
    /// limit, which it follows pop for pop up to its first full arena.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Restart {
        /// The same run: Fast then Quality ended on its own, or its route's
        /// path left no room below the limit for a fresh start.
        Never,
        /// Fast filled the arena without a route; plain Quality took over.
        Fresh,
        /// The arena filled with a route, which the fresh Quality run kept
        /// as the bound to beat.
        Carrying,
    }

    /// Checks Quality mode against Fast then Quality at one limit. Up to the
    /// first full arena the two make the same pops and inserts; there Fast
    /// then Quality stops and Quality starts over, so Quality generates more
    /// exactly when it restarted. Its route then ends no longer, and when
    /// Fast filled the arena without a route the rest is a fresh Quality
    /// run. Returns the Quality run, the Fast then Quality run and how they
    /// relate.
    fn check_restart(context: &str, board: &Board, max_states: usize) -> (Engine, Engine, Restart) {
        let (both, _) = run(board, Policy::FAST_THEN_QUALITY, max_states);
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
        // Two arenas at most, each with at most the spare node past the limit.
        assert!(
            restart.generated() as usize <= 2 * (max_states + 1),
            "{context}"
        );
        let filled = matches!(both.status(), Status::StateLimit | Status::MemoryLimit);
        let how = if restart.generated() <= both.generated() {
            Restart::Never
        } else if both.best_moves().is_none() {
            Restart::Fresh
        } else {
            Restart::Carrying
        };
        match how {
            Restart::Never => {
                // Only a route's path can leave no room to start over.
                assert!(!filled || both.best_moves().is_some(), "{context}");
                assert_eq!(outcome(&restart), outcome(&both), "{context}");
            }
            Restart::Fresh => {
                assert!(filled, "{context}");
                let (quality, _) = run(board, Policy::QUALITY, max_states);
                assert_eq!(
                    outcome(&restart),
                    (
                        quality.status(),
                        quality.best_moves(),
                        both.expanded() + quality.expanded(),
                        both.generated() + quality.generated()
                    ),
                    "{context}"
                );
            }
            Restart::Carrying => {
                assert!(filled, "{context}");
                let (moves, before) = (restart.best_moves().unwrap(), both.best_moves().unwrap());
                assert!(moves <= before, "{context}: {moves} > {before}");
                assert!(restart.expanded() >= both.expanded(), "{context}");
            }
        }
        (restart, both, how)
    }

    /// Quality mode on the whole catalog. Fast misses 27 boards at this
    /// limit; the restarted Quality run still finds expert-maze, 65 moves
    /// after 2,000 records in all, where Fast given 10,000 states needs
    /// 2,920 records for a 79-move route. A Python replica of the engine
    /// with the sealed-corral check (pruning/replicas/corral_port.py, not
    /// tracked) gives these counts. Its extension with Quality's phases and
    /// restart (pruning/replicas/quality_restart.py, not tracked) has Fast
    /// then Quality fill the arena with 14 of Fast's 30 routes, and Quality
    /// start over with each, shortening 2.
    #[test]
    fn quality_restarts_once_when_the_arena_first_fills() {
        let (mut rescued, mut carried, mut shortened) = (Vec::new(), 0, 0);
        for (id, board) in catalog() {
            let (restart, both, how) = check_restart(&id, &board, STATES);
            match how {
                Restart::Fresh if restart.best_moves().is_some() => rescued.push(id),
                Restart::Carrying => {
                    carried += 1;
                    shortened += usize::from(restart.best_moves() < both.best_moves());
                }
                _ => {}
            }
        }
        assert!(rescued.iter().any(|id| id == "expert-maze"), "{rescued:?}");
        assert!(carried > 0 && shortened > 0, "{carried} {shortened}");
    }

    /// Sweeps every limit on two small boards, through the one where Fast's
    /// route takes the spare node, in slices of one pop and of 2^20. On
    /// tiny, Quality starts over without a route at the smallest limits and
    /// carries Fast's route once it fits, the spare node included. At one
    /// state, ultra-tiny's one-push route takes the spare node with no room
    /// left for a fresh start, so Quality stops where Fast then Quality does.
    #[test]
    fn restart_survives_state_limit_boundaries() {
        let mut seen = Vec::new();
        let boards = catalog().into_iter();
        for (id, board) in boards.filter(|(id, _)| id == "ultra-tiny" || id == "tiny") {
            let (fast, _) = run(&board, Policy::FAST, STATES);
            for max_states in 1..=fast.generated() as usize + 2 {
                let context = format!("{id} at {max_states} states");
                let (sliced, both, how) = check_restart(&context, &board, max_states);
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
                // Fast then Quality's route took the spare node.
                let spare =
                    both.best_moves().is_some() && both.generated() as usize == max_states + 1;
                seen.push((how, spare));
            }
        }
        for wanted in [
            (Restart::Fresh, false),
            (Restart::Carrying, true),
            (Restart::Never, true),
        ] {
            assert!(seen.contains(&wanted), "{wanted:?} in {seen:?}");
        }
    }

    /// An arena that cannot grow ends the search at once with
    /// `MemoryLimit` and the f of the expansion it cut short, and never
    /// restarts, even under a restart policy: a fresh search would need the
    /// same allocation. Growth is refused from the start, so the search
    /// stops at its third record, when the four-record first chunk of unit
    /// tests asks for the next. The board is the one of
    /// `corral_check_prunes_a_goal_sealed_too_early`, whose route needs 18
    /// moves.
    #[test]
    fn a_refused_growth_stops_with_memory_limit() {
        let board = Board::parse("OOOOOOO\nO     O\nO BRA O\nOOOaOOO\nO  b  O\nOOOOOOO").unwrap();
        for policy in [
            Policy::EXACT,
            Policy::FAST,
            Policy::FAST_THEN_QUALITY_RESTART,
        ] {
            let mut engine =
                Engine::new(board.clone(), board.initial(), policy, STATES, 16).unwrap();
            engine.arena.refuse_growth(true);
            drive(&mut engine);
            assert_eq!(
                (engine.status(), engine.generated(), engine.discarded),
                (Status::MemoryLimit, 3, 0)
            );
            assert!(engine.interrupted_f.is_some() && engine.best_moves().is_none());
        }
    }

    /// One row per detector: its name, every detector on but that one, and
    /// how many runs turning it off must change at least, counting a run
    /// changed when any `SearchStats` counter differs, so a flag that never
    /// reaches its detector fails instead of passing on identical runs.
    /// Zero until a detector reads the flag. Full literals rather than
    /// `..Prunes::ALL`, so a new flag fails to compile here until every row
    /// sets it. The minimum is the replica's count of changed runs that
    /// generate fewer records. With the sealed-corral check on in both runs,
    /// `dead_pair` changes 9 runs in `corral_port.py live` (cited at
    /// `FINISH`), all on catalog boards, 6 of them with fewer records
    /// generated. `corral` changes 186 there, 2 finishing catalog runs, 112
    /// finishing room runs and 72 capped ones, 73 of them with fewer
    /// records generated.
    const TOGGLES: [(&str, Prunes, usize); 2] = [
        (
            "dead_pair",
            Prunes {
                dead_pair: false,
                corral: true,
            },
            6,
        ),
        (
            "corral",
            Prunes {
                dead_pair: true,
                corral: false,
            },
            73,
        ),
    ];
    /// Enough for every finishing-leg run to end on its own. A Python
    /// replica of `pruning_never_changes_a_live_run`
    /// (pruning/replicas/live_run_replica.py, not tracked) puts the push
    /// states reachable on those boards, before any pruning, at 1,841 at
    /// most on an explored board and 13,045 on a room. With the
    /// sealed-corral row added (pruning/replicas/corral_port.py live, not
    /// tracked), no finishing run generates more than 163 records.
    const FINISH: usize = 20_000;
    /// Rooms in the finishing leg.
    const ROOMS: usize = 100;
    /// The capped leg's limit.
    const CAPPED: usize = 2_000;

    /// Finished above capped above cancelled or running, as `rank` in
    /// scripts/bench-gate.mjs orders statuses.
    fn rank(status: Status) -> u8 {
        match status {
            Status::Solved | Status::Exhausted => 2,
            Status::StateLimit | Status::MemoryLimit | Status::TimeLimit => 1,
            Status::Running | Status::Cancelled => 0,
        }
    }

    /// What one run in `pruning_never_changes_a_live_run` ends with.
    struct Trace {
        status: Status,
        /// Moves, pushes and the route itself, as a fresh game replays it.
        route: Option<(u32, u32, String)>,
        /// Moves, expanded and generated when a route first appeared.
        first: Option<(u32, u32, u32)>,
        expanded: u32,
        generated: u32,
        stats: SearchStats,
        /// Exact's lower bound, `u64::MAX` when it has none because nothing
        /// is left to search; `None` under Fast, so one comparison serves
        /// both modes.
        lower_bound: Option<u64>,
    }

    /// Runs `board` from its start to a terminal status under Exact or Fast
    /// with `prunes`.
    fn trace(board: &Board, exact: bool, max_states: usize, prunes: Prunes) -> Trace {
        let start = board.initial();
        if exact {
            let mut search = ExactSearch::new(board.clone(), start, max_states, 16).unwrap();
            let mut traced = observe(board, search.engine_mut(), prunes);
            traced.lower_bound = Some(search.lower_bound().map_or(u64::MAX, u64::from));
            traced
        } else {
            let mut engine =
                Engine::new(board.clone(), start, Policy::FAST, max_states, 16).unwrap();
            observe(board, &mut engine, prunes)
        }
    }

    /// Installs `prunes`, drives `engine` to a terminal status and records
    /// what it ends with, replaying its route in a fresh game.
    fn observe(board: &Board, engine: &mut Engine, prunes: Prunes) -> Trace {
        engine.set_prunes(prunes);
        let first = drive(engine);
        let route = engine.solution().unwrap().map(|route| {
            let mut game = Game::new(board.clone());
            game.replay(&route).unwrap();
            assert!(game.solved());
            (game.moves(), game.pushes(), route)
        });
        Trace {
            status: engine.status(),
            route,
            first,
            expanded: engine.expanded(),
            generated: engine.generated(),
            stats: engine.stats(),
            lower_bound: None,
        }
    }

    /// The counters a detector never raises on a finished live run; see
    /// `pruning_never_changes_a_live_run`. The destructure names every
    /// `SearchStats` field, so a new counter fails to compile here until it
    /// is classified.
    fn bounded(trace: &Trace) -> [(&'static str, u64); 11] {
        let SearchStats {
            unique_states,
            duplicate_improvements,
            reopened_states,
            stale_pops,
            peak_queue,
            pruned_dead_cells,
            pruned_deadlocks: _,
            pruned_duplicates,
            pruned_assignment,
            pruned_bound,
            pruned_corrals: _,
        } = trace.stats;
        [
            ("expanded", u64::from(trace.expanded)),
            ("generated", u64::from(trace.generated)),
            ("unique_states", u64::from(unique_states)),
            ("duplicate_improvements", u64::from(duplicate_improvements)),
            ("reopened_states", u64::from(reopened_states)),
            ("stale_pops", u64::from(stale_pops)),
            ("peak_queue", u64::from(peak_queue)),
            ("pruned_dead_cells", pruned_dead_cells),
            ("pruned_duplicates", pruned_duplicates),
            ("pruned_assignment", pruned_assignment),
            ("pruned_bound", pruned_bound),
        ]
    }

    /// Checks a run with every detector on against the same run with one
    /// off, claim by claim as `pruning_never_changes_a_live_run` lists them.
    fn compare(context: &str, off: &Trace, on: &Trace) {
        assert!(
            rank(on.status) >= rank(off.status),
            "{context}: {} after {}",
            on.status.as_str(),
            off.status.as_str()
        );
        if let Some(&(moves, ..)) = off.route.as_ref() {
            let on_moves = on.route.as_ref().map(|&(got, ..)| got);
            assert!(
                on_moves.is_some_and(|got| got <= moves),
                "{context}: route {on_moves:?} after {moves}"
            );
        }
        assert!(
            on.lower_bound >= off.lower_bound,
            "{context}: lower bound {:?} after {:?}",
            on.lower_bound,
            off.lower_bound
        );
        if let Some((moves, expanded, generated)) = off.first {
            let Some((on_moves, on_expanded, on_generated)) = on.first else {
                panic!("{context}: no first route after {:?}", off.first);
            };
            // A capped off run whose first route came from the expansion the
            // cap interrupted may have stopped before a shorter goal child of
            // that expansion, which the on run, with fewer records, still
            // reaches, possibly in the spare node.
            let interrupted = rank(off.status) == 1 && expanded == off.expanded;
            let same_event =
                on_moves == moves && on_expanded <= expanded && on_generated <= generated;
            assert!(
                same_event || (interrupted && on_moves <= moves && on_expanded <= expanded),
                "{context}: first route {:?} after {:?}",
                on.first,
                off.first
            );
        }
        if rank(off.status) == 2 {
            assert_eq!(
                (on.status, &on.route),
                (off.status, &off.route),
                "{context}"
            );
            if off.route.is_some() {
                for ((name, on_count), (_, off_count)) in bounded(on).into_iter().zip(bounded(off))
                {
                    assert!(
                        on_count <= off_count,
                        "{context}: {name} {on_count} after {off_count}"
                    );
                }
            }
        }
    }

    /// The in-repo form of the identity theorem: a dead-state detector
    /// never changes a live run. Each board runs under Exact and Fast, one
    /// pop at a time, with every detector on and then with each one off
    /// (`TOGGLES`), in two legs: at `FINISH`, where the off run must end on
    /// its own, every explored catalog board and the first `ROOMS` rooms
    /// `random_room` draws from `Lcg(0xd1ff)` whose start has an
    /// assignment; at `CAPPED`, every catalog board. A room without one ends
    /// at its seed before any detector runs, so it is skipped: the Python
    /// replicas cited at `FINISH` both need 1,465 draws for the 100 rooms.
    /// Where the off run ends on its own and the on run generated fewer
    /// records, a boundary leg reruns both at the on run's generated count,
    /// and each `TOGGLES` row must change at least its minimum of runs.
    ///
    /// Call a detector entry-complete when the set E of states the on run
    /// calls dead is closed under every push that passes the dead-cell
    /// check, every push the off run flags lands in E, and the on run flags
    /// every push from outside E into E. From a start outside E, the on run
    /// is then the off run with E's states deleted: the same events in the
    /// same order, since queue ties break by insertion order, which
    /// deleting records keeps. A push into E that reaches the freeze check
    /// counts one `pruned_deadlocks` instead of what the off run did with
    /// that child. The freeze rule with its dead-pair case is entry-complete
    /// in the admit chain. Its E holds the states with a box held off its
    /// goal, a superset of what the rule without the case holds, and a held
    /// box never moves: a wall or held box on its axis makes the push
    /// illegal, and two dead neighbors make it land on a dead cell, which
    /// the dead-cell check prunes first. If the held box's component after a
    /// push from outside E did not contain the pushed box, none of its boxes
    /// moved and they held it the same way before. The freeze check takes
    /// the greatest fixpoint over the pushed box's whole component, the same
    /// there as over the board, so it flags the push. A start with a
    /// solution is outside any sound E, so the capped leg relies on every
    /// catalog board having one.
    ///
    /// The claims, and why they hold:
    /// - Status rank on >= off. The on run's records are the off run's
    ///   minus E's at every matched point, so it never fills the arena
    ///   first, and where the off run ends on its own, the on run ends at
    ///   the same event.
    /// - Wherever the off run has a route, the on run has one no longer.
    ///   The off run's route reaches only live states, so the on run finds
    ///   it too before any cap.
    /// - The first route is the same live event in both runs: equal moves,
    ///   with expanded and generated no higher, read after the pop that
    ///   found it. The bench reads `first_route_*` only every
    ///   `POPS_PER_CHECK` pops, so there they can still rise by one slice.
    /// - Exact's lower bound on >= off. Where the off run stops, the on
    ///   run's queue is the off run's minus E's entries, so its frontier is
    ///   no lower, and with consistent keys no child queues below its
    ///   parent, so the frontier never falls later. The incumbent cap cannot
    ///   pull it under either: the off bound is at most the optimum, which
    ///   no route beats.
    /// - Where the off run ends on its own, the same status and the same
    ///   route: moves, pushes and the route string.
    /// - At the boundary, the on run ends as before with the same route,
    ///   and the off run, which needs more records, hits the cap: the on
    ///   run never inserts at a full arena, so it is the same run, while the
    ///   off run's first insert past the cap stops it. The claims above
    ///   then compare a capped off run with a finished on run, the case
    ///   where rank and lower bound can rise. The lower bound is asserted
    ///   only no lower: the cap can fall in an expansion whose key already
    ///   equals the optimum.
    ///
    /// One exception narrows the first-route claim: in Exact, when the cap
    /// interrupts the expansion that found the off run's first route, the
    /// on run, which stores fewer of that expansion's children, may go on
    /// to a later goal child with a shorter walk, possibly in the spare
    /// node. There only its moves and expanded are bounded, no higher.
    ///
    /// Counters are compared only where the off run ends on its own with a
    /// route, the case the deletion argument settles. `expanded`,
    /// `generated`, `unique_states`, `duplicate_improvements`,
    /// `reopened_states` and `stale_pops` count events, of which the on run
    /// has a subset. `peak_queue` is a maximum over queues that are the off
    /// run's minus E's entries. `pruned_dead_cells`, `pruned_duplicates`,
    /// `pruned_assignment` and `pruned_bound` count rejections, which match
    /// on live states and happen on E's states only in the off run.
    /// `pruned_deadlocks` has no bound: the on run adds a flag per push into
    /// E, the off run flags pushes made inside E, and either can be larger.
    /// Nor has `pruned_corrals`, which only the on run of its row counts.
    /// Where the off run hits the cap, the on run goes on past that point,
    /// so no counter is compared. Without a route the start may lie in E.
    /// Both runs then see only dead states, not always the same ones, and
    /// either may count more, so only status and route are compared, and
    /// `FINISH` leaves both room to end. The replica finds 72 of the rooms
    /// solvable; the other 28 expand before they end Exhausted.
    ///
    /// A detector that is not entry-complete, such as one run at expansion
    /// time or one that flags only some pushes into its E, can add records
    /// or hit the cap first. Its flag must re-derive these claims before it
    /// gets a row in `TOGGLES`.
    ///
    /// The sealed-corral check is such a detector: it runs at expansion,
    /// so a dead state is still expanded, and only its children go. Its
    /// live events still match until either run fills the arena. Every
    /// parent of a state with a solution has one, and a pruned state has
    /// none, so both runs store the same live records with the same g and
    /// pop them in the same order: keys are unchanged and ties break by
    /// insertion order, which skipping other records keeps. Where both runs
    /// end on their own, they end at the same event with the same route.
    /// The dead records differ: a dead child the off run first stores from
    /// a pruned parent the on run may store later from another parent, at
    /// a higher g, and explore from there. So the remaining claims, which
    /// count records or compare a capped run, rest on the replica, not on
    /// the deletion argument. So do the `dead_pair` row's: its flag also
    /// drops the dead-pair case from the corral check's freeze, an
    /// expansion-time change on top of the admit chain's. corral_port.py
    /// live checks every claim of every run in both rows, the 76 boundary
    /// legs included, with 0 failures. A failure here after a change
    /// elsewhere means a claim needs a new argument, not that the detector
    /// flags a live state; the oracle tests in corral.rs check that.
    ///
    /// Cost: per board and mode, the all-on run plus one run per flag, one
    /// pop at a time, and two more per boundary. The capped leg dominates
    /// with at most 57 x 2 x 3 x 2,001 records, about 684k, around six
    /// times `fast_then_quality_starts_as_fast_and_never_ends_longer`; the
    /// replica counts about 439k. The finishing leg generates about 13k and
    /// the boundary legs about 19k.
    #[test]
    fn pruning_never_changes_a_live_run() {
        let mut rng = Lcg(0xd1ff);
        let rooms = std::iter::repeat_with(move || Board::parse(&random_room(&mut rng)).unwrap())
            .filter(|board| Heuristic::new(board).estimate(&board.initial()).is_some())
            .take(ROOMS)
            .enumerate()
            .map(|(n, board)| (format!("room {n}"), board));
        let finishing = explored_catalog()
            .iter()
            .map(|(id, board, ..)| (id.clone(), board.clone()))
            .chain(rooms)
            .map(|(id, board)| (id, board, FINISH, true));
        let capped = catalog()
            .into_iter()
            .map(|(id, board)| (id, board, CAPPED, false));
        let mut changed = [0; TOGGLES.len()];
        for (id, board, max_states, must_finish) in finishing.chain(capped) {
            for (mode, exact) in [("exact", true), ("fast", false)] {
                let on = trace(&board, exact, max_states, Prunes::ALL);
                for (row, (name, prunes, _)) in TOGGLES.into_iter().enumerate() {
                    let context = format!("{id} {mode} at {max_states} states, {name} off");
                    let off = trace(&board, exact, max_states, prunes);
                    if must_finish {
                        assert_eq!(
                            rank(off.status),
                            2,
                            "{context}: {}, raise FINISH",
                            off.status.as_str()
                        );
                    }
                    compare(&context, &off, &on);
                    changed[row] += usize::from(on.stats != off.stats);
                    if rank(off.status) == 2 && on.generated < off.generated {
                        let at = on.generated as usize;
                        let context = format!("{context}, at {at} states");
                        let on_at = trace(&board, exact, at, Prunes::ALL);
                        let off_at = trace(&board, exact, at, prunes);
                        assert!(
                            rank(on_at.status) == 2 && on_at.route == on.route,
                            "{context}: {} {:?} instead of the finished {:?}",
                            on_at.status.as_str(),
                            on_at.route,
                            on.route
                        );
                        assert_eq!(rank(off_at.status), 1, "{context}");
                        compare(&context, &off_at, &on_at);
                    }
                }
            }
        }
        for ((name, _, min_changed), changed) in TOGGLES.into_iter().zip(changed) {
            assert!(
                changed >= min_changed,
                "{name} off changed {changed} runs, below {min_changed}"
            );
        }
    }

    /// The board of `an_empty_goal_behind_a_settled_corral_is_dead` in the
    /// corral tests: A pushed into the corridor before B has passed seals
    /// B's goal off. The sealed-corral check prunes one such state in each
    /// mode, and both modes keep their 18-move route with fewer records.
    /// The counts are pruning/replicas/corral_port.py fixtures, line f3
    /// (not tracked).
    #[test]
    fn corral_check_prunes_a_goal_sealed_too_early() {
        let board = Board::parse("OOOOOOO\nO     O\nO BRA O\nOOOaOOO\nO  b  O\nOOOOOOO").unwrap();
        let without = Prunes {
            dead_pair: true,
            corral: false,
        };
        // Generated, expanded and the first route's moves, expanded and
        // generated, off then on.
        for (exact, counts_off, counts_on) in [
            (true, (12, 11, (18, 9, 12)), (10, 9, (18, 8, 10))),
            (false, (11, 8, (18, 8, 11)), (9, 7, (18, 7, 9))),
        ] {
            let off = trace(&board, exact, STATES, without);
            let on = trace(&board, exact, STATES, Prunes::ALL);
            for (traced, (generated, expanded, first), corrals) in
                [(&off, counts_off, 0), (&on, counts_on, 1)]
            {
                assert_eq!(
                    (
                        traced.generated,
                        traced.expanded,
                        traced.first,
                        traced.stats.pruned_corrals
                    ),
                    (generated, expanded, Some(first), corrals),
                    "exact {exact}"
                );
                let moves = traced
                    .route
                    .as_ref()
                    .map(|&(moves, pushes, _)| (moves, pushes));
                assert_eq!(moves, Some((18, 5)), "exact {exact}");
            }
            assert_eq!(on.route, off.route, "exact {exact}");
            compare(&format!("exact {exact}"), &off, &on);
        }
    }
}
