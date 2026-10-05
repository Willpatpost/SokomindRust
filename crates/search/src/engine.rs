use crate::{
    SearchError, SearchStats, SolutionError, Status, StopReason,
    arena::{Arena, Key, MAX_QUEUED_H, NIL, Node, TableCounters},
    corral::Corral,
    deadlock::Deadlock,
    heuristic::{Heuristic, ParentGroup},
    push::{self, Child, Parent, Parts, Prunes, Push, SkipCounters, canonicalize},
    reach::Reach,
    stage::{self, Done, Ladder},
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
    /// Whether a search with no incumbent hands its arena to the stage
    /// ladder (stage.rs) at its stage point: when the arena holds
    /// [`stage::FILL_MIB`]'s capacity or fills, whichever comes first. A
    /// full arena then stages instead of restarting or stopping. Fast-phase
    /// only: the ladder ends at its first route, so it needs
    /// stop_on_goal_push.
    stage: bool,
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
        stage: false,
    };
    /// First route wins, with no bound on its length. Fast never proves.
    /// When the search reaches its stage point with no route, the stage
    /// ladder searches again from the start in the same arena, and its first
    /// route wins. When it gives up on a full arena, the search stops at the
    /// limit as it would have unstaged; when it gives up below the limit,
    /// the plain search starts over with no stage point and ends as the
    /// unstaged search would. Below [`stage::MIN_STATES`] states at the
    /// stage point, or when the ladder's buffers cannot be reserved, the
    /// ladder declines and the plain search goes on to the full arena.
    pub(crate) const FAST: Self = Self {
        weight: 5,
        stop_on_goal_pop: true,
        stop_on_goal_push: true,
        reopen_closed: false,
        prune_popped_estimate: true,
        then: None,
        restart: None,
        stage: true,
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
        stage: false,
    };
    /// Exactly [`Self::FAST`] until its first route, then [`Self::QUALITY`]
    /// in the same arena. The incumbent only improves, so the result is never
    /// longer than Fast's, and its bound prunes from the first Quality pop.
    /// After a staged route, Quality runs in place on the ladder's last
    /// stage, and that run is incomplete: nodes it closed skipped the boxes
    /// the stage froze, and rises past the 128 an expansion holds, like
    /// rises accept rejected, sit in the table unqueued, so Quality reaches
    /// their states again only through a strictly cheaper duplicate
    /// (`reopen_closed`).
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
    /// the arena fills or the stage ladder ends, in either phase and with a
    /// route or without, the search starts over once as plain
    /// [`Self::QUALITY`] in the emptied arena, which keeps only its route's
    /// path, as the bound to beat. A fill in the Quality phase finds Fast
    /// then Quality stopping there with that same route, so the result is
    /// never longer than Fast then Quality's would be. When Fast reaches its
    /// stage point without a route, the stage ladder runs first, as under
    /// [`Self::FAST`], and the restart carries its route, so the result is
    /// never longer than the staged route; a ladder that gives up leaves the
    /// plain Quality result. Below [`stage::MIN_STATES`] states at the stage
    /// point, where the ladder declines, the result is never longer than
    /// Fast then Quality's, and a routeless fill gives the plain Quality
    /// result. When the route's path alone leaves no room for the start, it
    /// does what Fast then Quality does there.
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
// Nothing the stage ladder finds may feed a proof (see stage.rs).
const _: () = assert!(!Policy::EXACT.stage);
// Fast and the policies that start as Fast stage; Quality phases never
// do.
const _: () = assert!(Policy::FAST.stage && Policy::FAST_THEN_QUALITY.stage);
const _: () = assert!(Policy::FAST_THEN_QUALITY_RESTART.stage);
const _: () = assert!(!Policy::QUALITY.stage && !Policy::QUALITY_RESTART.stage);
// Mode's docs (lib.rs) call Quality's weight lighter than Fast's, and the
// arena's key-width test takes Fast's as the largest.
const _: () = assert!(Policy::QUALITY.weight < Policy::FAST.weight);
// Every `then` and `restart` target is itself in `Policy::ALL`, so the
// checks that loop over it cover every policy a search can switch to. A
// restarted policy has no next phase and no restart of its own, so it runs
// to the end and a search restarts at most once. A staging policy stops on
// a solved push, since the ladder ends at its first route, and is weighted.
// No `then` or `restart` target stages, so only a root policy stages, while
// it runs its first arena: spend_stage runs at the stage point, at the full
// arena, at next_phase and at every start_over, and a resume keeps the stage
// spent, so a search stages at most once. The ladder's end hands over to a
// `then` or `restart` target, resumes the root policy, or ends the search.
// A search resumes at most once (FAST or FTQ after a ladder that gave up
// below the limit) and restarts at most once.
const _: () = {
    let all = Policy::ALL;
    let mut i = 0;
    while i < all.len() {
        let p = all[i];
        assert!(listed(p.then) && listed(p.restart));
        if let Some(next) = p.restart {
            assert!(next.then.is_none() && next.restart.is_none());
            assert!(!next.stage);
        }
        if let Some(next) = p.then {
            assert!(!next.stage);
        }
        if p.stage {
            assert!(p.stop_on_goal_push && p.weight > 1);
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
                stage,
            } = *a;
            weight == b.weight
                && stop_on_goal_pop == b.stop_on_goal_pop
                && stop_on_goal_push == b.stop_on_goal_push
                && reopen_closed == b.reopen_closed
                && prune_popped_estimate == b.prune_popped_estimate
                && same(then, b.then)
                && same(restart, b.restart)
                && stage == b.stage
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
    /// Records a restart or the stage ladder's reseeds dropped; they still
    /// count as generated. A kept path stays in the arena, so it counts
    /// there, once.
    discarded: u32,
    /// g + queued h (at least 1) of a node whose expansion a limit cut short.
    /// Under an admissible h, no route through its unpushed successors along
    /// this path is shorter, so the exact frontier must include it.
    interrupted_f: Option<u64>,
    /// Some while the stage ladder runs the search; never with an incumbent.
    staging: Option<Staging>,
    /// The arena length at which a staging search hands over to the ladder:
    /// the [`stage::FILL_MIB`] capacity, or the node limit once the stage is
    /// spent or when the policy never stages.
    fill_at: usize,
    /// Whether the root policy may still stage. A resumed search keeps a
    /// policy with `stage` set, so the flag lives here.
    may_stage: bool,
    /// The last ladder's tally, for tests.
    #[cfg(test)]
    staged: Option<stage::Tally>,
}

/// The stage ladder running a search that reached its stage point, or filled
/// its arena, with no route.
struct Staging {
    ladder: Ladder,
    /// g + queued h (at least 1) of the parent whose insert reached the stage
    /// point or filled the arena. For a full-arena stage, the ladder's first
    /// reseed drops that parent, so a ladder that gives up stops with the
    /// same `interrupted_f` an unstaged stop would.
    trigger_f: u64,
    /// Staged at the stage point, below the limit.
    early: bool,
}

/// g + queued h (at least 1) of `parent`, the f a limit that cuts its
/// expansion short records: an expanded node is unsolved, so at least one
/// move remains.
fn limit_f(parent: &Parent) -> u64 {
    parent.node.g as u64 + parent.queued_h.max(1) as u64
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
    /// The arena reached its stage point, or filled, with no route, and the
    /// stage ladder took over; the parent is gone.
    Staged,
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
        let boxes = board.labels().len();
        let arena = Arena::new(cells, boxes, max_states, memory_mib)?;
        let node_limit = arena.node_limit();
        let fill_at = if policy.stage {
            Arena::capacity(cells, boxes, max_states, stage::FILL_MIB)
                .map_or(node_limit, |c| c.limit.min(node_limit))
        } else {
            node_limit
        };
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
            staging: None,
            fill_at,
            may_stage: policy.stage,
            #[cfg(test)]
            staged: None,
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
                .enqueue(push::key(0, total_h, self.policy.weight), total_h, root);
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
    /// `Cancelled`, never a terminal verdict the search did not reach. A
    /// running stage ladder is dropped with its unfinished stage.
    pub(crate) fn stop(&mut self, reason: StopReason) {
        if self.status == Status::Running {
            self.staging = None;
            self.status = match reason {
                StopReason::Cancelled => Status::Cancelled,
                StopReason::TimeLimit => Status::TimeLimit,
            };
        }
    }
    /// Hands a goal stop over to the policy's next phase, re-keying the queue
    /// for its weight, and spends the stage. False when there is none and
    /// the search ends.
    fn next_phase(&mut self) -> bool {
        let Some(&next) = self.policy.then else {
            return false;
        };
        self.arena.reweight(self.policy.weight, next.weight);
        self.spend_stage();
        self.policy = next;
        true
    }
    /// Empties the arena, keeping the incumbent's path, and starts a fresh
    /// search under `next`: the rest of the run is a fresh search bounded by
    /// the incumbent, whose records add to the dropped ones. False, with
    /// nothing changed, when the path would leave no room below the limit
    /// for the start (clear_keeping refuses).
    fn start_over(&mut self, next: Policy) -> bool {
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
        self.spend_stage();
        self.policy = next;
        self.seed();
        true
    }
    /// Starts over under the policy's restart. False when there is none, or
    /// when start_over refuses.
    fn restart(&mut self) -> bool {
        let Some(&next) = self.policy.restart else {
            return false;
        };
        self.start_over(next)
    }
    /// After a FAST or FTQ ladder that gave up at the stage point, below the
    /// limit: runs the plain search again from the start under the same
    /// policy, now with no stage point, so the rest of the run is the
    /// unstaged search. FTQR restarts into Quality instead, as at a full
    /// arena.
    fn resume_unstaged(&mut self) {
        debug_assert!(self.policy.restart.is_none() && self.incumbent.is_none());
        let resumed = self.start_over(self.policy);
        debug_assert!(resumed);
    }
    /// Spends the search's one stage: no stage point remains, only the full
    /// arena. Returns whether the stage was still available.
    fn spend_stage(&mut self) -> bool {
        self.fill_at = self.arena.node_limit();
        std::mem::replace(&mut self.may_stage, false)
    }
    /// The arena is full, or could not grow, while expanding a node whose
    /// g + queued h is `f`.
    fn stop_at_limit(&mut self, f: u64) {
        self.interrupted_f = Some(f);
        self.status = self.arena.limit_status();
    }
    /// Hands the search to the stage ladder when the arena reached its stage
    /// point or filled, while expanding `parent`, with no route: builds the
    /// ladder from the start with the work [`stage::work`] gives for `at`,
    /// the arena's length, and begins it, which empties the arena but for a
    /// fresh copy of the start. The next [`Self::advance`] call runs it.
    /// False, with nothing changed, when the arena is starved, when its
    /// length is below [`stage::MIN_STATES`], or when the ladder's buffers
    /// cannot be reserved; the search then goes on, restarts or stops as it
    /// would unstaged.
    // Cold and out of line: it runs at most once per search. Without these
    // attributes, here and on advance_staged, plain searches ran slower with
    // the same work; A/B job 46990 measured them recovering a sixth to a half
    // of that (half on large Optimal), and the inline attributes on
    // Parts::open, push::admit and stand_walk recover most of the rest.
    #[cold]
    #[inline(never)]
    fn start_staging(&mut self, parent: &Parent) -> bool {
        debug_assert_eq!(self.status, Status::Running);
        debug_assert!(self.staging.is_none() && self.incumbent.is_none());
        let at = self.arena.len();
        // The non-full insert path already stops a starved search, so this
        // is a guard: a starved arena could not hold the ladder's stages.
        if self.arena.starved() || at < stage::MIN_STATES {
            return false;
        }
        let Ok(mut ladder) = Ladder::new(
            &self.board,
            &self.heuristic,
            &self.start,
            stage::work(at),
            self.policy.weight,
            self.policy.reopen_closed,
        ) else {
            return false;
        };
        // Only a root policy stages, so no restart or resume has re-seeded
        // the arena, and the start is still the record new's seed inserted
        // first.
        debug_assert!(self.arena.node(0).state == self.start);
        // begin cannot fail here, so the ladder never gives up before its
        // first stage: insert runs only in a running search, and seed ends
        // the search when the start has no assignment, so the start has an
        // estimate; clear_keeping(0) keeps one record, below a length of at
        // least MIN_STATES; and a starved arena was declined above.
        ladder.begin(&mut self.parts());
        self.discarded += ladder.take_dropped();
        self.staging = Some(Staging {
            ladder,
            trigger_f: limit_f(parent),
            early: at < self.arena.node_limit(),
        });
        true
    }
    /// Runs the stage ladder one unit per pop until it ends or the pops run
    /// out, and returns the pops left: 0 while the ladder still runs. A
    /// ladder that ends hands its route, or its lack of one, to the policy
    /// as a full arena would have, or resumes the plain search after an
    /// early stage.
    // Cold and out of line, as start_staging: advance calls it only while
    // a ladder runs.
    #[cold]
    #[inline(never)]
    fn advance_staged(&mut self, mut pops: u32) -> u32 {
        let Some(mut staging) = self.staging.take() else {
            return pops;
        };
        let mut done = None;
        while done.is_none() && pops > 0 {
            pops -= 1;
            done = staging.ladder.step(&mut self.parts());
        }
        self.discarded += staging.ladder.take_dropped();
        let Some(done) = done else {
            self.staging = Some(staging);
            return 0;
        };
        #[cfg(test)]
        {
            self.staged = Some(staging.ladder.tally());
        }
        let trigger_f = staging.trigger_f;
        let early = staging.early;
        // The ladder goes before its route becomes the incumbent, so the
        // two never coexist.
        drop(staging);
        // An arena that cannot grow seeds no fresh search and runs no more
        // Quality: it ends the search, as it does an unstaged one.
        let starved = self.arena.starved();
        match done {
            Done::Solved(id) => {
                self.incumbent = Some(id);
                // FTQR starts over as plain Quality, the staged route the
                // bound to beat.
                let restarted = !starved && self.restart();
                if !restarted {
                    if !self.next_phase() {
                        // FAST: the staged route ends the search, as a
                        // solved push does.
                        self.status = Status::Solved;
                    } else if starved {
                        // FTQ, or FTQR, keeps the route but stops there.
                        self.stop_at_limit(trigger_f);
                    }
                    // Otherwise FTQ's Quality phase, or FTQR's whose route
                    // left no room for a fresh start, goes on in place.
                }
            }
            // FTQR starts over as plain Quality with no route; FAST and FTQ
            // resume the plain search after an early stage, or stop where
            // they would have unstaged after a full one.
            Done::GaveUp => {
                if starved {
                    self.stop_at_limit(trigger_f);
                } else if !self.restart() {
                    if early {
                        self.resume_unstaged();
                    } else {
                        self.stop_at_limit(trigger_f);
                    }
                }
            }
        }
        pops
    }
    /// Work is sliced by pops so a worker can yield, report, or cancel. A
    /// pop is one queue pop, or one stage ladder unit while the ladder runs
    /// the search; a search that hands over to the ladder returns at once.
    /// Stale, solved and dominated pops count toward `pops` without
    /// expanding anything.
    pub(crate) fn advance(&mut self, mut pops: u32) {
        debug_assert!(self.staging.is_none() || self.incumbent.is_none());
        if self.status != Status::Running {
            return;
        }
        // Each pop runs these steps in order, and the first check that
        // rejects something skips the rest for it:
        // - stage: while the stage ladder runs the search, a pop is one of
        //   its units instead: one rank unit, one movers unit, one stage
        //   expansion, one accept unit (a ban or one check), one commit, or
        //   one fail pass. When the ladder ends, its route, or a full arena
        //   without one, goes to the policy, and the pops left go on below.
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
        // - insert: store and queue the child and record a solved one. The
        //   stage point, or a full arena, hands the search to the stage
        //   ladder; a full arena may instead start it over or end it, and so
        //   may a solved push.
        if self.staging.is_some() {
            pops = self.advance_staged(pops);
            if self.status != Status::Running || self.staging.is_some() {
                return;
            }
        }
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
        let Some(parent) = Parent::of(&self.arena, key) else {
            self.skipped.stale_pops += 1;
            return Popped::Skip;
        };
        let node = &parent.node;
        if self.board.solved(&node.state) {
            if self.best_moves().is_none_or(|best| node.g < best) {
                self.incumbent = Some(parent.index);
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
                    && node.g as u64 + parent.queued_h as u64 >= best as u64)
        }) {
            self.skipped.pruned_bound += 1;
            return Popped::Skip;
        }
        Popped::Expand(parent)
    }
    /// This engine's [`Parts`], borrowed field by field: lent once to open
    /// each expansion and once per push for its kernel, since each insert
    /// needs the whole engine.
    #[inline]
    fn parts(&mut self) -> Parts<'_> {
        Parts {
            board: &self.board,
            heuristic: &self.heuristic,
            reach: &mut self.reach,
            deadlock: &mut self.deadlock,
            corral: &mut self.corral,
            arena: &mut self.arena,
            prunes: self.prunes,
            skipped: &mut self.skipped,
            expanded: &mut self.expanded,
        }
    }
    /// Expands a popped node: [`Parts::open`] runs the prologue, then each
    /// push the static check ([`push::legal`]) passes goes to
    /// [`Self::admit`], and each child it admits to [`Self::insert`], unless
    /// the prologue's sealed-corral check finds no solution from the node,
    /// which then gets no children. Breaks when the search ended or handed
    /// over to the stage ladder during the expansion.
    #[inline]
    fn expand(&mut self, parent: Parent) -> ControlFlow<()> {
        let Some(parent_h) = self.parts().open(&parent) else {
            return ControlFlow::Continue(());
        };
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
                    // The arena now holds the ladder's start, and the next
                    // advance call runs the ladder.
                    Inserted::Staged | Inserted::Stopped => return ControlFlow::Break(()),
                }
            }
        }
        ControlFlow::Continue(())
    }
    /// Runs one push through [`push::admit`] with the kernel of this
    /// engine's [`Parts`], lent per push because each insert needs the
    /// whole engine. `reopen_closed` and `best` are the values
    /// [`Self::expand`] keeps current across its inserts.
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
        push::admit(
            &mut self.parts().kernel(),
            parent,
            parent_h,
            parent_group,
            push,
            reopen_closed,
            best,
        )
    }
    /// Stores and queues an admitted child, recording it when it is solved.
    /// At the stage point, below the limit, an unsolved child hands the
    /// search to the stage ladder, unless staging declines. At a full arena
    /// a solved child is kept as the incumbent, in the spare slot; then,
    /// with no incumbent, a policy that has not spent its stage hands the
    /// search to the ladder, and otherwise the search starts over or ends.
    /// Below the limit a solved child may end it, and so does a failed
    /// growth allocation, with [`Status::MemoryLimit`].
    #[inline]
    fn insert(&mut self, parent: &Parent, child: Child) -> Inserted {
        debug_assert!(self.staging.is_none() || self.incumbent.is_none());
        // admit's g + h prune left g + h < best, so a solved child always
        // improves the incumbent.
        let goal = child.is_goal(&self.board);
        let Child { node, h, slot } = child;
        // One compare on the hot path: fill_at is the node limit unless the
        // stage point lies below it, so this is is_full() for every search
        // that cannot stage early.
        if self.arena.len() >= self.fill_at || self.arena.starved() {
            if !self.arena.is_full() {
                // The stage point, below the limit: stage, or fall through to
                // a normal insert when the child solves or staging declines.
                if self.stage_early(parent, goal) {
                    return Inserted::Staged;
                }
            } else {
                // Keep a solution discovered at the exact limit, for the
                // restart to carry or the stop to report.
                if goal {
                    self.incumbent = Some(self.arena.insert(node, slot));
                }
                // The rest of this expansion belongs to the discarded arena.
                if self.spend_stage() && self.incumbent.is_none() && self.start_staging(parent) {
                    return Inserted::Staged;
                }
                if self.restart() {
                    return Inserted::Restarted;
                }
                self.stop_at_limit(limit_f(parent));
                return Inserted::Stopped;
            }
        }
        let id = self.arena.insert(node, slot);
        self.arena
            .enqueue(push::key(node.g, h, self.policy.weight), h, id);
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
            self.stop_at_limit(limit_f(parent));
            return Inserted::Stopped;
        }
        Inserted::Queued
    }
    /// The stage point: spends the stage and hands the search to the ladder
    /// unless the child solves or staging declines.
    #[cold]
    #[inline(never)]
    fn stage_early(&mut self, parent: &Parent, goal: bool) -> bool {
        debug_assert!(self.may_stage && self.incumbent.is_none() && self.staging.is_none());
        self.spend_stage();
        !goal && self.start_staging(parent)
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
    use super::{Engine, Policy, same};
    use crate::{
        SearchStats, Status, StopReason,
        exact::ExactSearch,
        heuristic::Heuristic,
        push::Prunes,
        stage::{self, Tally},
        testkit::{Lcg, catalog, explored_catalog, huge, random_room},
    };
    use sokomind_core::{Board, Game};
    use std::time::Instant;

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
        // The bounds and the Fresh equality below hold only unstaged.
        assert!(
            max_states < stage::MIN_STATES,
            "{context}: the ladder would stage"
        );
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

    /// The memory budget of the staged tests, the probe's.
    const STAGED_MIB: usize = 64;
    /// The limit `staged_generated_bound` and `staged_slicing_is_deterministic`
    /// give a staging policy on huge: at least `stage::MIN_STATES`, so the
    /// arena, which fills with no route, stages, and small enough for a cheap
    /// fill.
    const STAGED: usize = 8_192;

    /// A fresh engine for `policy` on huge from its start, at `max_states`
    /// states and `STAGED_MIB`.
    fn fresh_on_huge(policy: Policy, max_states: usize) -> Engine {
        let board = huge();
        Engine::new(
            board.clone(),
            board.initial(),
            policy,
            max_states,
            STAGED_MIB,
        )
        .unwrap()
    }

    /// Runs `policy` on huge from its start at `max_states` states and
    /// `STAGED_MIB`, in slices of `pops`, to a terminal status.
    fn on_huge(policy: Policy, max_states: usize, pops: u32) -> Engine {
        let mut engine = fresh_on_huge(policy, max_states);
        while engine.status() == Status::Running {
            engine.advance(pops);
        }
        engine
    }

    /// Rebuilds `engine`'s route and replays it in a fresh game on `board`,
    /// which it must solve in the moves the engine counts. Returns its moves
    /// and pushes.
    fn replay(engine: &mut Engine, board: Board) -> (u32, u32) {
        let route = engine.solution().unwrap().expect("a route");
        let mut game = Game::new(board);
        game.replay(&route).unwrap();
        assert!(game.solved());
        assert_eq!(Some(game.moves()), engine.best_moves());
        (game.moves(), game.pushes())
    }

    /// `replay` on huge.
    fn replay_on_huge(engine: &mut Engine) -> (u32, u32) {
        replay(engine, huge())
    }

    /// Drives `engine` to a terminal status one pop at a time, checking that
    /// it never stages, and that it ends with no route.
    fn drive_unstaged(engine: &mut Engine) {
        while engine.status() == Status::Running {
            engine.advance(1);
            assert!(engine.staging.is_none());
        }
        assert!(engine.staged.is_none() && engine.best_moves().is_none());
    }

    /// Checks the counter identities `engine` must keep, staged or not.
    fn assert_counters(engine: &Engine) {
        let stats = engine.stats();
        assert_eq!(
            stats.unique_states + stats.duplicate_improvements,
            engine.generated()
        );
        assert!(engine.expanded() <= engine.generated());
        assert!(stats.stale_pops <= stats.duplicate_improvements);
    }

    /// What a run ends with, for comparing runs.
    #[derive(Debug, PartialEq, Eq)]
    struct Ended {
        status: Status,
        route: Option<String>,
        expanded: u32,
        generated: u32,
        stats: SearchStats,
        /// Whether a ladder ran to its end.
        staged: bool,
    }

    /// What `engine`, which has ended, ends with; rebuilding its route also
    /// replays it.
    fn ended(engine: &mut Engine) -> Ended {
        Ended {
            status: engine.status(),
            route: engine.solution().unwrap(),
            expanded: engine.expanded(),
            generated: engine.generated(),
            stats: engine.stats(),
            staged: engine.staged.is_some(),
        }
    }

    /// Fast on huge at `max_states` states and `STAGED_MIB`, in slices of
    /// 2^20 pops (slicing changes no result, see
    /// `staged_slicing_is_deterministic`): the arena fills with no route, and
    /// the staged route, the probe's 812 moves of 268 pushes, ends the search.
    /// Prints the checks spent and the time taken, and returns the ladder's
    /// tally, whose checks budget must not have ended it.
    fn staged_fast_on_huge(max_states: usize) -> Tally {
        let started = Instant::now();
        let mut engine = on_huge(Policy::FAST, max_states, 1 << 20);
        let elapsed = started.elapsed().as_millis();
        assert_eq!(engine.status(), Status::Solved);
        assert_eq!(replay_on_huge(&mut engine), (812, 268));
        let tally = engine.staged.expect("a staged search");
        assert!(!tally.checks_bound);
        println!(
            "checks {} for work {}, {elapsed} ms",
            tally.checks, tally.work
        );
        tally
    }

    /// Staged Fast at the probe's 64 MiB settings matches probe-46758. The
    /// ladder's first reseed keeps only the start, so it runs as the probe's
    /// did from a fresh arena, and the scaled limit gives the probe's 2^20
    /// work.
    #[test]
    #[ignore = "release only: run on SLURM with --ignored --nocapture"]
    fn staged_fast_routes_huge_at_64_mib() {
        let Tally {
            work,
            commits,
            failures,
            backtracks,
            deepest,
            accepts,
            rejects,
            checks: _,
            checks_bound: _,
        } = staged_fast_on_huge(60_000_000);
        assert_eq!(
            (work, commits, failures, backtracks, deepest, accepts),
            (617_192, 26, 10, 3, 22, 21_940)
        );
        assert_eq!(rejects, [187, 17_530, 0, 1_706, 2_487, 4]);
    }

    /// Staged Fast at the gate's limit, `GATE` states, matches probe-46758.
    #[test]
    #[ignore = "release only: run on SLURM with --ignored --nocapture"]
    fn staged_fast_routes_huge_at_the_gate() {
        let Tally {
            work,
            commits,
            failures,
            backtracks,
            deepest,
            accepts,
            rejects,
            checks: _,
            checks_bound: _,
        } = staged_fast_on_huge(GATE);
        println!("deepest {deepest}");
        assert_eq!(
            (work, commits, failures, backtracks, accepts),
            (192_499, 26, 10, 3, 7_766)
        );
        assert_eq!(rejects, [187, 5_956, 0, 1_593, 0, 4]);
    }

    /// Quality mode on huge at 60M states and 64 MiB, in slices of 2^20 pops
    /// but one pop at a time while the ladder runs, so the test stops at the
    /// pop that ends it and reads the route before Quality starts: Fast
    /// fills the arena with no route, the ladder routes it (the probe's 812
    /// moves of 268 pushes), and the pop that ends the ladder starts plain
    /// Quality over with that route as its bound. The fresh search fills the
    /// arena and ends there, and its route replays and is no longer. Prints
    /// the time to the first route and to the end.
    #[test]
    #[ignore = "release only: run on SLURM with --ignored --nocapture"]
    fn staged_quality_routes_huge_at_64_mib() {
        let started = Instant::now();
        let mut engine = fresh_on_huge(Policy::FAST_THEN_QUALITY_RESTART, 60_000_000);
        // The slice that fills the arena returns at once with the ladder
        // running; single pops then stop at the pop that ends it.
        while engine.staging.is_none() {
            assert_eq!(engine.status(), Status::Running);
            engine.advance(1 << 20);
        }
        while engine.staged.is_none() {
            assert!(engine.staging.is_some());
            engine.advance(1);
        }
        let first = started.elapsed().as_millis();
        assert!(same(Some(&engine.policy), Some(&Policy::QUALITY)));
        assert_eq!(replay_on_huge(&mut engine), (812, 268));
        assert!(!engine.staged.as_ref().unwrap().checks_bound);
        while engine.status() == Status::Running {
            engine.advance(1 << 20);
        }
        let total = started.elapsed().as_millis();
        assert_eq!(engine.status(), Status::MemoryLimit);
        assert_counters(&engine);
        let (moves, _) = replay_on_huge(&mut engine);
        assert!(moves <= 812, "{moves} > 812");
        println!("first route {first} ms, ended {total} ms, {moves} moves");
    }

    /// The state limit of the benchmark gate.
    const GATE: usize = 20_000;
    /// The ladder's whole work budget at the gate, `stage::work(GATE)`.
    const GATE_WORK: u32 = 640_000;
    /// A board's catalog id, the ladder's work on it, and its route's moves
    /// and pushes.
    type GateBoard = (&'static str, u32, Option<(u32, u32)>);

    /// The catalog boards plain Fast leaves unrouted at the gate, with the
    /// work probe-46758's ladder (accept tests lane, sink, e, f and g) spent
    /// on each and the route it found, or `None` where it gave up. The probe
    /// ran with a budget of 2^20; the six boards that spent the whole of it
    /// give up here at `GATE_WORK`, and every other board took the same work.
    const GATE_STAGED: [GateBoard; 23] = [
        ("gen-v2-320041-027a9b04", 280, Some((141, 58))),
        ("gen-v2-320050-4eb88a2d", 28_791, Some((230, 68))),
        ("gen-v2-320050-78319f43", 337, Some((181, 55))),
        ("gen-v2-320050-ece6c64b", 306, Some((170, 52))),
        ("gen-v2-320081-b59cab95", 652, Some((153, 50))),
        ("gen-v2-330146-44c41505", 83_809, Some((178, 62))),
        ("gen-v2-340053-466f0008", 527_793, Some((175, 57))),
        ("gen-v2-340106-b3558338", 395, Some((197, 80))),
        ("gen-v2-340114-a8b9d02a", 196_557, Some((229, 80))),
        ("gen-v2-340187-cd08a50a", 63_798, Some((219, 77))),
        ("gen-v2-340272-fd819d0e", 711, Some((343, 121))),
        ("gen-v2-350070-6be05c02", 323_274, Some((309, 112))),
        ("gen-v2-350108-80cb0432", 72_983, Some((289, 99))),
        ("gen-v2-360372-b5687375", 50_772, Some((397, 138))),
        ("gen-v2-370002-8ea0b852", 58_500, Some((491, 162))),
        ("huge", 192_499, Some((812, 268))),
        ("gen-v2-340046-c9f5ef3b", GATE_WORK, None),
        ("gen-v2-340048-b5d312a6", 149_724, None),
        ("gen-v2-340053-6d913910", GATE_WORK, None),
        ("gen-v2-350001-a996dcbc", GATE_WORK, None),
        ("gen-v2-350108-b13869bc", GATE_WORK, None),
        ("gen-v2-360313-16158b3b", GATE_WORK, None),
        ("gen-v2-370193-88873f15", GATE_WORK, None),
    ];

    /// Fast on every catalog board at the gate's limit and `STAGED_MIB`, in
    /// slices of 2^20 pops. Each board in `GATE_STAGED` stages and ends as
    /// the probe's ladder did: after the same work, with the same moves and
    /// pushes or at the state limit with no route. Every other board routes
    /// before its arena fills and never stages. Prints each staged board's
    /// checks and time.
    #[test]
    #[ignore = "release only: run on SLURM with --ignored --nocapture"]
    fn staged_gate_routes_match_probe() {
        assert_eq!(stage::work(GATE), GATE_WORK);
        let mut staged = 0;
        for (id, board) in catalog() {
            let started = Instant::now();
            let mut engine = Engine::new(
                board.clone(),
                board.initial(),
                Policy::FAST,
                GATE,
                STAGED_MIB,
            )
            .unwrap();
            while engine.status() == Status::Running {
                engine.advance(1 << 20);
            }
            let elapsed = started.elapsed().as_millis();
            let Some(&(_, work, expected)) = GATE_STAGED.iter().find(|entry| entry.0 == id) else {
                assert!(engine.staged.is_none(), "{id}");
                assert_eq!(engine.status(), Status::Solved, "{id}");
                continue;
            };
            staged += 1;
            let tally = engine.staged.as_ref().expect(&id);
            assert!(!tally.checks_bound, "{id}");
            assert_eq!(tally.work, work, "{id}");
            println!(
                "{id}: checks {} for work {}, {elapsed} ms",
                tally.checks, tally.work
            );
            match expected {
                Some(route) => {
                    assert_eq!(engine.status(), Status::Solved, "{id}");
                    assert_eq!(replay(&mut engine, board), route, "{id}");
                }
                None => {
                    assert_eq!(engine.status(), Status::StateLimit, "{id}");
                    assert_eq!(engine.best_moves(), None, "{id}");
                }
            }
        }
        assert_eq!(staged, GATE_STAGED.len());
    }

    /// Exact never stages: at a limit where Fast would, its full arena on
    /// huge ends the search with no route.
    #[test]
    fn exact_never_stages() {
        let mut engine = fresh_on_huge(Policy::EXACT, 20_000);
        drive_unstaged(&mut engine);
        assert!(matches!(
            engine.status(),
            Status::StateLimit | Status::MemoryLimit
        ));
    }

    /// Below `stage::MIN_STATES` states the ladder declines: Fast fills the
    /// arena on huge with no route and stops there, as it does unstaged.
    #[test]
    fn stage_declines_below_min_states() {
        let mut engine = fresh_on_huge(Policy::FAST, stage::MIN_STATES - 1);
        drive_unstaged(&mut engine);
        assert_eq!(engine.status(), Status::StateLimit);
    }

    /// An arena that cannot grow never stages, at a limit where a full one
    /// would: the search stops at once with `MemoryLimit`, as in
    /// `a_refused_growth_stops_with_memory_limit`.
    #[test]
    fn stage_declines_when_starved() {
        let mut engine = fresh_on_huge(Policy::FAST, stage::MIN_STATES);
        engine.arena.refuse_growth(true);
        drive_unstaged(&mut engine);
        assert_eq!(engine.status(), Status::MemoryLimit);
    }

    /// A staged search's records stay within the bound that keeps every
    /// counter in u32 (section 4.6 of slurm/reports/stage-port-plan.md, not
    /// tracked): two arenas of at most L + 1 records, the ladder's work, and
    /// one live copy of a root per reseed, at most 12,870 of them. The
    /// counter identities hold across the ladder's reseeds.
    #[test]
    fn staged_generated_bound() {
        let engine = on_huge(Policy::FAST, STAGED, 1);
        assert!(engine.staged.is_some());
        let limit = STAGED as u64;
        let bound = 2 * (limit + 1) + u64::from(stage::work(STAGED)) + 12_870;
        let generated = engine.generated();
        assert!(u64::from(generated) <= bound, "{generated} > {bound}");
        assert_counters(&engine);
    }

    /// A staged search ends the same in slices of one pop as in slices of
    /// 2^20: the ladder runs one unit per pop, and the pops a slice has left
    /// when it ends go to the search it hands back to. Quality mode starts
    /// over when the ladder ends, with its route or without, so it always
    /// hands back.
    #[test]
    fn staged_slicing_is_deterministic() {
        let policy = Policy::FAST_THEN_QUALITY_RESTART;
        let one = ended(&mut on_huge(policy, STAGED, 1));
        assert!(one.staged);
        assert_eq!(ended(&mut on_huge(policy, STAGED, 1 << 20)), one);
    }

    /// The three staging policies on huge at 20,000 states, where Fast fills
    /// the arena with no route. Fast ends with the staged route, the
    /// probe's 812 moves. Fast then Quality goes on in place from it, and
    /// Quality mode starts over as plain Quality with it as the bound, at
    /// the pop that ends the ladder; no route ends longer, every one
    /// replays, and the counter identities hold after either handoff.
    #[test]
    fn fast_ftq_ftqr_handoffs() {
        const LIMIT: usize = 20_000;
        let mut fast = on_huge(Policy::FAST, LIMIT, 1);
        assert_eq!(fast.status(), Status::Solved);
        assert!(fast.staged.is_some());
        assert_eq!(replay_on_huge(&mut fast), (812, 268));
        let staged = fast.best_moves().unwrap();
        let mut both = on_huge(Policy::FAST_THEN_QUALITY, LIMIT, 1);
        assert!(both.staged.is_some());
        assert!(same(Some(&both.policy), Some(&Policy::QUALITY)));
        assert_counters(&both);
        let (moves, _) = replay_on_huge(&mut both);
        assert!(moves <= staged, "{moves} > {staged}");
        let mut restart = fresh_on_huge(Policy::FAST_THEN_QUALITY_RESTART, LIMIT);
        while restart.staged.is_none() {
            assert_eq!(restart.status(), Status::Running);
            restart.advance(1);
        }
        // The pop that ended the ladder started over as plain Quality, not
        // as Quality mode's second phase in place, with the staged route.
        assert!(same(Some(&restart.policy), Some(&Policy::QUALITY)));
        assert_eq!(restart.best_moves(), Some(staged));
        while restart.status() == Status::Running {
            restart.advance(1);
        }
        assert_counters(&restart);
        let (moves, _) = replay_on_huge(&mut restart);
        assert!(moves <= staged, "{moves} > {staged}");
    }

    /// A stop while the ladder runs drops it with its unfinished stage and
    /// ends the search with the stop's status and no route, and a later
    /// advance does nothing. At `stage::MIN_STATES` states the fill before
    /// it is cheap.
    #[test]
    fn stop_during_staging() {
        for (reason, status) in [
            (StopReason::TimeLimit, Status::TimeLimit),
            (StopReason::Cancelled, Status::Cancelled),
        ] {
            let mut engine = fresh_on_huge(Policy::FAST, stage::MIN_STATES);
            while engine.staging.is_none() {
                assert_eq!(engine.status(), Status::Running);
                engine.advance(1);
            }
            // Some ladder units, past the first stage's rank and movers
            // units, so the stop lands mid-stage with stage records in the
            // arena.
            engine.advance(4_096);
            assert!(engine.staging.is_some(), "the ladder ended early");
            assert!(engine.arena.len() > 1);
            engine.stop(reason);
            assert!(engine.staging.is_none() && engine.staged.is_none());
            assert_eq!((engine.status(), engine.best_moves()), (status, None));
            let generated = engine.generated();
            engine.advance(1);
            assert_eq!((engine.status(), engine.generated()), (status, generated));
        }
    }
}
