use crate::{
    MAX_STATES_RANGE, MEMORY_MIB_RANGE, SearchError, SearchStats, SolutionError, Status,
    StopReason,
    arena::{Arena, Key, MAX_QUEUED_H, NIL, Node, TableCounters},
    corral::Corral,
    deadlock::Deadlock,
    heuristic::{Heuristic, ParentGroup},
    polish::{self, Polish},
    push::{self, Child, Parent, Parts, Push, SkipCounters, canonicalize},
    reach::Reach,
    room_bound::{BUILD_UNITS_PER_POP, RoomBound, RoomBuild, RoomParent},
    stage::{self, Done, Ladder},
};
use sokomind_core::{ACTIONS, Board, MAX_CELLS, MAX_ROUTE, OPPOSITE, State};
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
    /// [`stage::STAGE_AT`] states or fills, whichever comes first. A
    /// full arena then stages instead of restarting or stopping. Fast-phase
    /// only: the ladder ends at its first route, so it needs
    /// stop_on_goal_push.
    stage: bool,
    /// Whether the stage ladder's work may grow to the caller's state limit
    /// ([`stage::work`]), so that under a large limit only a route, a
    /// give-up or the caller's deadline ends it, not 2^20 inserts. Fast
    /// mode's only. Job 49443 ran both modes on the catalog with it: Fast at
    /// 30 s routed three more boards (gen-v2-350108-b138 at about 6 s,
    /// 360313-1615 and 370193-8887 at 16 to 17 s), and lost none at 5 s,
    /// though most give-ups then ran to the deadline. Quality at 5 s lost
    /// gen-v2-340053-6d91 at 128 and 256 MiB: there its capped ladder gives
    /// up early, and the restart routes the board within 3.1 s. So Quality
    /// mode's ladder keeps the cap.
    long_ladder: bool,
    /// Whether the estimate counts the pushes a box needs with the keeper
    /// on its current side ([`Heuristic`]), at more table bytes per cell.
    /// It raises the exact search's bounds, but misleads the weighted
    /// searches: under the 2026-10-06 A/B, huge's stage ladder found no
    /// route at any limit from 8,192 to 128,000 states. The heuristic is
    /// built once per search, so a search keeps its root policy's value.
    keeper_sides: bool,
    /// Partial expansion (PEA*) once the arena holds this percent of its
    /// node limit: every expansion but the root's then stores only the
    /// children with f at most the node's queued f, and queues the node
    /// again at the least f it left out, read from the built child or,
    /// before building it, bounded by [`push::excess`]. Every child left
    /// out has f above the old key and at least the new one, so the least
    /// key still bounds the optimum, and each re-pop raises the node's
    /// queued h until [`MAX_QUEUED_H`], where it expands in full. It trades
    /// re-expansions for fewer stored children, which pays only once memory
    /// binds; a record count rather than bytes keeps native and WASM on one
    /// search. Exact only: its arena only grows, so once partial, every
    /// expansion stays so. Probe 48731 at 90 raised Grand Hall's bound from
    /// 240 to 243 at 11M states and 1 GiB, and lowered no open case's bound
    /// at any budget (README, Optimal).
    partial_fill: Option<u8>,
    /// Whether a search that ends on its own with a route polishes it before
    /// its status is final: single-box rescheduling (polish.rs) within the
    /// arena's share of the budget, in several runs with different box
    /// orders, then box pairs, which keeps a route only when it replays
    /// shorter. A search that goes long enough without improving its route
    /// also polishes it in an episode, within the part of the share the
    /// arena does not hold, and then goes on (see [`EPISODE_FLOOR`]).
    /// Route-only, so never Exact's; Fast's first route stands as found.
    polish: bool,
    /// Whether the search builds the room bound (room_bound.rs) before the
    /// search proper, in slices paid as pops, and then queues max(M, S),
    /// with S the room tables' sum plus the hall part. The tables are
    /// charged to the budget: the arena is re-sized for them and the start
    /// queued again. Exact only: it raises bounds, and the build costs time a
    /// weighted search spends finding its route. Job 48775 raised Grand
    /// Hall's root bound from 214 to 255 (docs/room-bound.md).
    room_bound: bool,
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
        long_ladder: false,
        keeper_sides: true,
        partial_fill: Some(90),
        polish: false,
        room_bound: true,
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
        long_ladder: true,
        keeper_sides: false,
        partial_fill: None,
        polish: false,
        room_bound: false,
    };
    /// Keeps improving the incumbent until the queue empties or a limit hits,
    /// then polishes the route; polishes it in episodes on the way when the
    /// search stalls.
    pub(crate) const QUALITY: Self = Self {
        weight: 3,
        stop_on_goal_pop: false,
        stop_on_goal_push: false,
        reopen_closed: true,
        prune_popped_estimate: true,
        then: None,
        restart: None,
        stage: false,
        long_ladder: false,
        keeper_sides: false,
        partial_fill: None,
        polish: true,
        room_bound: false,
    };
    /// Quality mode's second phase at weight 2: [`Self::QUALITY`] with a
    /// lighter weight, the policy [`Self::QUALITY_RESTART`] runs until its
    /// arena fills.
    const QUALITY_LIGHT: Self = Self {
        weight: 2,
        ..Self::QUALITY
    };
    /// Quality mode's second phase: [`Self::QUALITY_LIGHT`], except that
    /// when its arena fills the search starts over as plain [`Self::QUALITY`],
    /// bounded by the route it has.
    const QUALITY_RESTART: Self = Self {
        restart: Some(&Self::QUALITY),
        ..Self::QUALITY_LIGHT
    };
    /// Quality mode: [`Self::FAST`] until its first route, except that its
    /// stage ladder keeps the work cap (see `long_ladder`), then
    /// [`Self::QUALITY_RESTART`] in the same arena. The incumbent only
    /// improves, so the result is never longer than the Fast phase's, and its
    /// bound prunes from the first Quality pop. The first time the arena fills
    /// or the stage ladder ends, in either phase and with a route or without,
    /// the search starts over once as plain [`Self::QUALITY`] in the emptied
    /// arena, which keeps only its route's path, as the bound to beat. When
    /// Fast reaches its stage point without a route, the stage ladder runs
    /// first, with the capped work, and the restart carries its route, so the
    /// result is never longer than the staged route; a ladder that gives up
    /// leaves the plain Quality result. Below [`stage::MIN_STATES`] states at
    /// the stage point the ladder declines, and a routeless fill gives the
    /// plain Quality result. When the route's path alone leaves no room for the
    /// start, the search stops at the fill with that route. Whichever phase
    /// ends the search with a route polishes it, and either polishes a stalled
    /// route in episodes.
    pub(crate) const FAST_THEN_QUALITY_RESTART: Self = Self {
        then: Some(&Self::QUALITY_RESTART),
        restart: Some(&Self::QUALITY),
        long_ladder: false,
        polish: true,
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
        Self::QUALITY_LIGHT,
        Self::QUALITY_RESTART,
        Self::FAST_THEN_QUALITY_RESTART,
    ];
}
// Exact soundness assumes one admissible weight for the whole search.
const _: () = assert!(Policy::EXACT.then.is_none() && Policy::EXACT.restart.is_none());
const _: () = assert!(Policy::EXACT.weight == 1);
// Nothing the stage ladder finds may feed a proof (see stage.rs).
const _: () = assert!(!Policy::EXACT.stage);
// Fast and Quality mode, which starts as Fast, stage; Quality phases never
// do.
const _: () = assert!(Policy::FAST.stage && Policy::FAST_THEN_QUALITY_RESTART.stage);
const _: () = assert!(!Policy::QUALITY.stage && !Policy::QUALITY_RESTART.stage);
// Only Fast mode's ladder grows past the cap (see `long_ladder`).
const _: () = assert!(Policy::FAST.long_ladder && !Policy::FAST_THEN_QUALITY_RESTART.long_ladder);
// Polish only shortens routes, never bounds them, so Exact never runs it;
// Fast's route stands as found. Every policy Quality mode runs polishes,
// whichever phase ends the search.
const _: () = assert!(!Policy::EXACT.polish && !Policy::FAST.polish);
const _: () = assert!(
    Policy::FAST_THEN_QUALITY_RESTART.polish
        && Policy::QUALITY_RESTART.polish
        && Policy::QUALITY.polish
);
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
// below the limit) and restarts at most once. A search keeps its heuristic
// through every switch, so its targets count keeper sides as it does. No
// policy both counts keeper sides and polishes: polish reads per-cell push
// distances (`Heuristic::distance`), which such a heuristic does not keep.
// A partially expanding policy is admissible and runs alone, with no next
// phase, restart or stage, so its arena only grows and every key it
// re-queues bounds a route it left out. A room-bound policy is admissible
// and runs alone too, and is no `then` or `restart` target, so the build
// runs only from `Engine::new`, under the root policy. It counts keeper
// sides, so the hall part, which shares M's moves, is feasible wherever M
// is, and it never stages, since the arena's re-size sets the fill point to
// the new limit.
const _: () = {
    let all = Policy::ALL;
    let mut i = 0;
    while i < all.len() {
        let p = all[i];
        assert!(listed(p.then) && listed(p.restart));
        assert!(!(p.keeper_sides && p.polish));
        if let Some(next) = p.restart {
            assert!(next.then.is_none() && next.restart.is_none());
            assert!(!next.stage && next.keeper_sides == p.keeper_sides);
            assert!(!next.room_bound);
        }
        if let Some(next) = p.then {
            assert!(!next.stage && next.keeper_sides == p.keeper_sides);
            assert!(!next.room_bound);
        }
        if p.stage {
            assert!(p.stop_on_goal_push && p.weight > 1);
        }
        if p.long_ladder {
            assert!(p.stage);
        }
        if let Some(percent) = p.partial_fill {
            assert!(percent <= 100 && p.weight == 1);
            assert!(p.then.is_none() && p.restart.is_none() && !p.stage);
        }
        if p.room_bound {
            assert!(p.weight == 1 && p.then.is_none() && p.restart.is_none());
            assert!(!p.stage && p.keeper_sides);
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
                long_ladder,
                keeper_sides,
                partial_fill,
                polish,
                room_bound,
            } = *a;
            weight == b.weight
                && stop_on_goal_pop == b.stop_on_goal_pop
                && stop_on_goal_push == b.stop_on_goal_push
                && reopen_closed == b.reopen_closed
                && prune_popped_estimate == b.prune_popped_estimate
                && same(then, b.then)
                && same(restart, b.restart)
                && stage == b.stage
                && long_ladder == b.long_ladder
                && keeper_sides == b.keeper_sides
                && match (partial_fill, b.partial_fill) {
                    (Some(a), Some(b)) => a == b,
                    (None, None) => true,
                    _ => false,
                }
                && polish == b.polish
                && room_bound == b.room_bound
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
    /// [`stage::STAGE_AT`] or the node limit, whichever is smaller, or the
    /// node limit once the stage is spent or when the policy never stages.
    fill_at: usize,
    /// The arena length from which expansions are partial (see
    /// `Policy::partial_fill`), or `usize::MAX` for a policy that never
    /// expands partially.
    partial_at: usize,
    /// Whether the root policy may still stage. A resumed search keeps a
    /// policy with `stage` set, so the flag lives here.
    may_stage: bool,
    /// Some while a polish runs: an episode, while a polishing search waits
    /// with its arena and incumbent kept and goes on once the polish ends,
    /// or the end polish, once the search has ended with a route: the
    /// polisher then owns the route, the arena is released, no incumbent
    /// remains, and the status is final once the polish ends.
    polish: Option<Polish>,
    /// The expansion count at which a polishing search polishes its
    /// incumbent in an episode, or `u32::MAX` when none is due: set when the
    /// incumbent improves, cleared when an episode starts, so an incumbent
    /// gets one episode at most.
    episode_at: u32,
    /// The shortest route an episode found, while it is shorter than the
    /// incumbent. It never bounds the search: the search, and the end polish
    /// of its incumbent, run pop for pop as they would with no episode, so
    /// given the pops, the result is never longer than theirs; only a time
    /// limit sees the episode's time. Between polishes it is one of the route
    /// allowance's two routes, since [`Self::solution`] then never rebuilds
    /// the incumbent's longer route, and a polisher's start rebuilds only
    /// the other; while a polisher holds its own route there, it is charged
    /// to the polisher's allowance.
    polished: Option<Vec<u8>>,
    /// The room bound once it is built and charged to the budget (see
    /// `Policy::room_bound`). `None` while it builds, and for good when it
    /// built no table or the arena could not be re-sized for its tables.
    rooms: Option<RoomBound>,
    /// Some while the room bound builds, before the search proper, which
    /// waits with only the start in its arena, queued with M alone.
    room_build: Option<RoomBuild>,
    /// The memory budget, which the arena is re-sized against once the
    /// room bound is built.
    memory_mib: usize,
    /// The caller's state limit, which also sets the stage ladder's work
    /// under a `long_ladder` policy (see [`stage::work`]) and the end
    /// polish's kick work.
    max_states: usize,
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

/// Budget bytes for the longest route and its letter string, `MAX_ROUTE`
/// bytes each, which [`Engine::solution`] allocates on demand. Part of
/// [`FIXED_SCRATCH`], and the most the stage ladder may hold.
const ROUTE_ALLOWANCE: usize = 2 * MAX_ROUTE;

/// Budget bytes charged to every search whatever the board: the route
/// allowance plus a 64 KiB allowance for the fixed-size scratch buffers.
/// The stage ladder reuses the route allowance: it exists only while the
/// search has no route and is freed before one is recorded, and
/// `Ladder::bytes_for(MAX_CELLS)` is asserted to fit. On wasm32 linear
/// memory never shrinks, and the route need not reuse the blocks the ladder
/// frees, so, like the first table that `Arena::grow` frees, they can stay
/// allocated past the budget.
const FIXED_SCRATCH: usize = ROUTE_ALLOWANCE + 64 * 1024;
const _: () = assert!(Ladder::bytes_for(MAX_CELLS) <= ROUTE_ALLOWANCE);

/// When a polishing search's incumbent last improved at `e` expansions, an
/// episode polishes it at `e + max(e, EPISODE_FLOOR)`, and the search goes
/// on once the polish ends. Probe 48691 (docs/route-polish.md) chose this:
/// waiting 4e more instead gained too little at 256 and 1024 MiB; ending the
/// search there instead lost routes whose late improvements came 4-246x
/// after it; and letting the episode's route bound the search pruned the
/// search's own later improvements. The floor keeps episodes off short
/// searches: none starts in CI's catalog at 20k states.
const EPISODE_FLOOR: u32 = 1 << 16;

/// The bytes a search charges its budget besides the arena: the flood,
/// deadlock and corral buffers and the heuristic tables per cell, with or
/// without keeper sides, plus [`FIXED_SCRATCH`]. Counted in u64, since usize
/// is 32 bits on wasm32, and saturating, so a board too large to count
/// never fits instead of wrapping.
fn fixed_bytes(cells: usize, boxes: usize, keeper_sides: bool) -> u64 {
    let per_cell = Reach::BYTES_PER_CELL
        + Deadlock::BYTES_PER_CELL
        + Corral::BYTES_PER_CELL
        + Heuristic::bytes_per_cell(boxes, keeper_sides);
    (cells as u64)
        .saturating_mul(per_cell as u64)
        .saturating_add(FIXED_SCRATCH as u64)
}

/// The arena length from which `policy` expands partially (see
/// `Policy::partial_fill`) in an arena of `node_limit` nodes, or
/// `usize::MAX` when it never does.
fn partial_at(policy: &Policy, node_limit: usize) -> usize {
    policy.partial_fill.map_or(usize::MAX, |percent| {
        (node_limit as u64 * u64::from(percent) / 100) as usize
    })
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
        if !MAX_STATES_RANGE.contains(&max_states) || !MEMORY_MIB_RANGE.contains(&memory_mib) {
            return Err(SearchError::Limits);
        }
        let cells = board.tiles().len();
        let boxes = board.labels().len();
        let fixed = fixed_bytes(cells, boxes, policy.keeper_sides);
        let arena = Arena::new(fixed, boxes, max_states, memory_mib)?;
        let node_limit = arena.node_limit();
        let fill_at = if policy.stage {
            stage::STAGE_AT.min(node_limit)
        } else {
            node_limit
        };
        let partial_at = partial_at(&policy, node_limit);
        let heuristic = Heuristic::new(&board, policy.keeper_sides.then_some(start.player));
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
            status: Status::Running,
            expanded: 0,
            skipped: SkipCounters::default(),
            incumbent: None,
            discarded: 0,
            interrupted_f: None,
            staging: None,
            fill_at,
            partial_at,
            may_stage: policy.stage,
            polish: None,
            episode_at: u32::MAX,
            polished: None,
            rooms: None,
            room_build: None,
            memory_mib,
            max_states,
        };
        search.seed();
        // The build's share is what the budget leaves besides the fixed
        // buffers and the arena holding the start.
        if policy.room_bound && search.status == Status::Running {
            search.room_build = RoomBuild::new(
                &search.board,
                &search.start,
                search.arena.node_limit(),
                memory_mib,
                fixed,
                search.arena.held_bytes(),
            );
        }
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
    /// The best route's length: the shortest of the polisher's, the best
    /// episode's and the incumbent's.
    pub(crate) fn best_moves(&self) -> Option<u32> {
        let held = self.polish.as_ref().map(Polish::moves);
        let polished = self.polished.as_ref().map(|route| route.len() as u32);
        [held, polished, self.incumbent_moves()]
            .into_iter()
            .flatten()
            .min()
    }
    /// The incumbent's g, the bound the search prunes with. The search reads
    /// this rather than [`Self::best_moves`], since no polisher runs while it
    /// does and an episode's route never bounds it.
    fn incumbent_moves(&self) -> Option<u32> {
        self.incumbent.map(|i| self.arena.meta(i).g)
    }
    /// Makes the solved record `id`, shorter than any incumbent, the
    /// incumbent. A polishing search then schedules the incumbent's episode
    /// (see [`EPISODE_FLOOR`]) and drops an episode's route that is no
    /// longer shorter. Starting over keeps the incumbent without this, since
    /// it improves nothing.
    fn record(&mut self, id: u32) {
        self.incumbent = Some(id);
        if self.policy.polish {
            self.episode_at = self
                .expanded
                .saturating_add(self.expanded.max(EPISODE_FLOOR));
            let g = self.arena.meta(id).g;
            self.polished = self
                .polished
                .take()
                .filter(|route| (route.len() as u32) < g);
        }
    }
    pub(crate) fn expanded(&self) -> u32 {
        self.expanded
    }
    /// Records inserted, including any a restart discarded.
    pub(crate) fn generated(&self) -> u32 {
        self.discarded + self.arena.len() as u32
    }
    /// The ceiling on the search's buffers: the whole budget while the room
    /// bound builds, since the build's buffers may fill what the fixed
    /// buffers and the arena leave, and the arena's reservation otherwise,
    /// which counts the room bound's bytes once the arena is re-sized.
    pub(crate) fn reserved_bytes(&self) -> usize {
        if self.room_build.is_some() {
            self.memory_mib << 20
        } else {
            self.arena.reserved_bytes()
        }
    }
    /// The bytes charged to the budget besides the arena: the fixed buffers
    /// and the room bound's tables and hall part.
    fn charged_bytes(&self) -> u64 {
        let (cells, boxes) = (self.board.tiles().len(), self.board.labels().len());
        fixed_bytes(cells, boxes, self.policy.keeper_sides)
            .saturating_add(self.rooms.as_ref().map_or(0, RoomBound::bytes))
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
        assert_eq!(self.policy.weight, 1, "weighted keys bound nothing");
        self.arena
            .min_f()
            .unwrap_or(u64::MAX)
            .min(self.interrupted_f.unwrap_or(u64::MAX))
    }
    /// Ends a running search from outside: `reason` is a limit or
    /// `Cancelled`, never a terminal verdict the search did not reach. A
    /// running stage ladder is dropped with its unfinished stage, a running
    /// polish with its unfinished job, keeping its best route, and a running
    /// room build with its tables, leaving the start queued with M alone.
    pub(crate) fn stop(&mut self, reason: StopReason) {
        if self.status == Status::Running {
            self.staging = None;
            self.room_build = None;
            if let Some(polisher) = &mut self.polish {
                polisher.stop();
            }
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
        self.end(self.arena.limit_status());
    }
    /// Ends the search with `status`, unless the policy polishes and the
    /// incumbent's route rebuilds: the polisher then takes the route and the
    /// status, the arena is released so its share of the budget holds the
    /// polisher's jobs, and the status is final once the polish ends. A
    /// route that does not rebuild ends the search here, and
    /// [`Self::solution`] reports the error. The incumbent is polished even
    /// when an episode polished it before: the end polish has the whole
    /// share, which an episode's runs and pairs may not have had.
    fn end(&mut self, status: Status) {
        debug_assert!(self.polish.is_none());
        if self.policy.polish
            && let Some(id) = self.incumbent
            && let Ok(route) = self.route_to(id)
        {
            let boxes = self.board.labels().len();
            let allowance = (self.arena.reserved_bytes() as u64)
                .saturating_sub(self.charged_bytes())
                .saturating_sub(self.polished_bytes());
            self.incumbent = None;
            self.arena.release();
            // The kicks' work follows the caller's state limit, which bounds
            // their cost when there is no time limit (docs/route-polish.md).
            self.polish = Some(Polish::new(
                route,
                status,
                allowance,
                boxes,
                self.max_states,
            ));
        } else {
            self.status = status;
        }
    }
    /// Hands the search to the stage ladder when the arena reached its stage
    /// point or filled, while expanding `parent`, with no route: builds the
    /// ladder from the start with the work [`stage::work`] gives for `at`,
    /// the arena's length, under the state limit, and begins it, which
    /// empties the arena but for a fresh copy of the start. The next
    /// [`Self::advance`] call runs it. False, with nothing changed, when the
    /// arena is starved, when its length is below [`stage::MIN_STATES`], or
    /// when the ladder's buffers cannot be reserved; the search then goes
    /// on, restarts or stops as it would unstaged.
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
        let limit = if self.policy.long_ladder {
            self.max_states
        } else {
            0
        };
        let Ok(mut ladder) = Ladder::new(
            &self.board,
            &self.heuristic,
            &self.start,
            stage::work(at, limit),
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
    /// Runs the room bound's build for `pops` pop slots of
    /// [`BUILD_UNITS_PER_POP`] units each, and returns the slots left: 0
    /// while it still runs. No pop runs while the build does, so the arena
    /// holds only the start, queued with M. A build that ends with tables
    /// re-sizes the arena with the room bound's bytes charged to the budget
    /// and queues the start again with max(M, S) in place of that entry; its
    /// record keeps M, as every record does. The start is inserted once, so
    /// generated stays within the state limit. With no table, or when the
    /// re-size refuses, the search goes on with M alone.
    // Cold and out of line, as advance_staged: advance calls it only while
    // the build runs.
    #[cold]
    #[inline(never)]
    fn advance_build(&mut self, pops: u32) -> u32 {
        let Some(build) = &mut self.room_build else {
            return pops;
        };
        let mut units = (pops as usize).saturating_mul(BUILD_UNITS_PER_POP);
        let Some(rooms) = build.step(&self.board, &self.start, &self.heuristic, &mut units) else {
            return 0;
        };
        self.room_build = None;
        let left = (units / BUILD_UNITS_PER_POP) as u32;
        let Some(rooms) = rooms else {
            return left;
        };
        // Never `None`: a table whose search never reached a goal from the
        // start is not kept, and the hall part is feasible wherever M is. A
        // missing projection would still prove the start unsolvable, since
        // its relaxed problem has no solution.
        let s = rooms.root(&self.start);
        self.rooms = Some(rooms);
        if !self.arena.recharge(self.charged_bytes(), self.memory_mib) {
            // The arena, unchanged, leaves no room for the tables.
            self.rooms = None;
            return left;
        }
        // Exact never stages, so the fill point is the limit.
        let node_limit = self.arena.node_limit();
        self.fill_at = node_limit;
        self.partial_at = partial_at(&self.policy, node_limit);
        let root = self.arena.dequeue();
        debug_assert!(self.arena.len() == 1 && root.is_some_and(|key| key.id() == 0));
        if let Some(root) = root {
            // The entry's h is M's root estimate.
            if let Some(s) = s {
                let h = root.h().max(s.min(MAX_QUEUED_H));
                self.arena
                    .enqueue(push::key(0, h, self.policy.weight), h, root.id());
            } else {
                self.status = Status::Exhausted;
            }
        }
        left
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
                self.record(id);
                // FTQR starts over as plain Quality, the staged route the
                // bound to beat.
                let restarted = !starved && self.restart();
                if !restarted {
                    if !self.next_phase() {
                        // FAST: the staged route ends the search, as a
                        // solved push does.
                        self.end(Status::Solved);
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
    /// pop is one queue pop, [`BUILD_UNITS_PER_POP`] room build units while
    /// the room bound builds, one stage ladder unit while the ladder runs
    /// the search, or one polish unit while a polish runs; a search that
    /// hands over to the ladder, or starts an episode, returns at once.
    /// Stale, solved and dominated pops count toward `pops` without
    /// expanding anything.
    pub(crate) fn advance(&mut self, mut pops: u32) {
        debug_assert!(self.staging.is_none() || self.incumbent.is_none());
        if self.status != Status::Running {
            return;
        }
        // Each pop runs these steps in order, and the first check that
        // rejects something skips the rest for it:
        // - build: while the room bound builds, before the search proper, a
        //   pop is BUILD_UNITS_PER_POP of its units instead (room_bound.rs).
        //   Once it is built, the arena is re-sized and the start re-queued,
        //   and the pops left go on below.
        // - stage: while the stage ladder runs the search, a pop is one of
        //   its units instead: one rank unit, one movers unit, one stage
        //   expansion, one accept unit (a ban or one check), one commit, or
        //   one fail pass. When the ladder ends, its route, or a full arena
        //   without one, goes to the policy, and the pops left go on below.
        // - pop: an empty queue ends the search. A stale entry (stale_pops)
        //   is skipped, a solved node is recorded and skipped or ends the
        //   search, and a node the bound prunes (pruned_bound) is skipped.
        // - expand: close the node, counting it the first time, flood its
        //   keeper, refresh the deadlock occupancy, and skip every push of a
        //   state with a sealed corral (pruned_corrals). Otherwise try every
        //   push, box-major and in direction order. A push into a wall or a
        //   box, or from a stand the keeper cannot reach, is dropped
        //   uncounted, and so is one a partial expansion leaves out by its
        //   walk bound (push::excess).
        // - admit, per push: dead cell (pruned_dead_cells), g bound
        //   (pruned_bound), freeze deadlock (pruned_deadlocks), then the
        //   settled child's duplicate (pruned_duplicates), assignment
        //   (pruned_assignment), g + h bound and stand walk (pruned_bound),
        //   then with the room bound a missing projection
        //   (pruned_assignment) and the g + S bound (pruned_bound).
        // - insert: a partial expansion leaves out an unsolved child past
        //   the node's queued f, and after its last push queues the node
        //   again at the least f it left out. Every other child is stored
        //   and queued, and a solved one recorded. The stage point, or a
        //   full arena, hands the search to the stage ladder; a full arena
        //   may instead start it over or end it, and so may a solved push.
        // - episode: after an expansion, a polishing search whose incumbent
        //   is due an episode (EPISODE_FLOOR) starts one if a job fits the
        //   part of the budget the arena does not hold.
        // - polish: while a polish runs, a pop is one polish unit instead:
        //   starting a run or a job, one of a job's setup layers, or one of
        //   its pops. When the last run has ended and a full pass over the
        //   pairs gains nothing, an episode keeps its route if shorter and
        //   the pops left go on below; the end polish ends the search with
        //   the status it stopped on.
        if self.room_build.is_some() {
            pops = self.advance_build(pops);
            if self.status != Status::Running || self.room_build.is_some() {
                return;
            }
        }
        if self.staging.is_some() {
            pops = self.advance_staged(pops);
            if self.status != Status::Running || self.staging.is_some() {
                return;
            }
        }
        if self.polish.is_some() {
            pops = self.advance_polish(pops);
            if self.status != Status::Running || self.polish.is_some() {
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
            // One compare per expansion: episode_at is u32::MAX unless a
            // polishing search's incumbent is due an episode.
            if self.expanded >= self.episode_at && self.start_episode() {
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
            self.end(if self.incumbent.is_some() {
                Status::Solved
            } else {
                Status::Exhausted
            });
            return Popped::Stop;
        };
        let Some(parent) = Parent::of(&self.arena, key) else {
            self.skipped.stale_pops += 1;
            return Popped::Skip;
        };
        let node = &parent.node;
        // Every queued h is admissible, so 0 for a solved state, and testing
        // it first skips the box scan on nearly every pop.
        if parent.queued_h == 0 && self.board.solved(&node.state) {
            if self.incumbent_moves().is_none_or(|best| node.g < best) {
                self.record(parent.index);
            }
            if self.policy.stop_on_goal_pop && !self.next_phase() {
                self.end(Status::Solved);
                return Popped::Stop;
            }
            return Popped::Skip;
        }
        if self.incumbent_moves().is_some_and(|best| {
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
            skipped: &mut self.skipped,
            expanded: &mut self.expanded,
        }
    }
    /// Expands a popped node: [`Parts::open`] runs the prologue, then each
    /// push the static check ([`push::legal`]) passes goes to
    /// [`Self::admit`], and each child it admits to [`Self::insert`], unless
    /// the prologue's sealed-corral check finds no solution from the node,
    /// which then gets no children. A partial expansion (see
    /// `Policy::partial_fill`) leaves out each push whose child's f lies
    /// past the node's queued f, by [`push::excess`] or once admitted, and
    /// queues the node again at the least such f. Breaks when the search
    /// ended or handed over to the stage ladder during the expansion.
    #[inline]
    fn expand(&mut self, parent: Parent) -> ControlFlow<()> {
        let Some(parent_h) = self.parts().open(&parent) else {
            return ControlFlow::Continue(());
        };
        // Never the root, whose keeper may carry a charge (see push::excess),
        // nor a node whose queued h cannot rise further.
        let partial = self.arena.len() >= self.partial_at
            && parent.node.g > 0
            && parent.queued_h < MAX_QUEUED_H;
        debug_assert!(!partial || self.heuristic.charge(parent.node.state.player) == 0);
        let g = u64::from(parent.node.g);
        let limit = g + u64::from(parent.queued_h);
        // The least f of a child left out.
        let mut deferred = None::<u64>;
        let mut parent_group = ParentGroup::EMPTY;
        // Set up by the first child the room bound prices, so a search
        // without it never fills one in.
        let mut room_parent = None::<RoomParent>;
        // The bound and the reopen flag, read again after each insert, the
        // only call here that changes the incumbent or the policy: it can
        // record a shorter route, or hand a solved push to a next phase and
        // still queue it.
        let mut best = self.incumbent_moves();
        let mut reopen_closed = self.policy.reopen_closed;
        for i in 0..self.board.labels().len() {
            let from = parent.node.state.boxes[i];
            for d in 0..4 {
                let Some(push) = push::legal(&self.board, &self.reach, i, from, d) else {
                    continue;
                };
                // parent_h is M even with the room bound: this f bounds the
                // child's M-based f, which bounds its queued one from below.
                if partial
                    && let Some(excess) =
                        push::excess(&self.heuristic, &self.reach, parent_h, &push)
                {
                    let f = g + u64::from(parent_h) + u64::from(excess);
                    if f > limit {
                        deferred = Some(deferred.map_or(f, |least| least.min(f)));
                        continue;
                    }
                }
                let Some(child) = self.admit(
                    &parent,
                    parent_h,
                    (&mut parent_group, &mut room_parent),
                    push,
                    reopen_closed,
                    best,
                ) else {
                    continue;
                };
                // A solved child is always stored, whatever its f.
                let f = u64::from(child.node.g) + u64::from(child.h);
                if partial && child.h > 0 && f > limit {
                    deferred = Some(deferred.map_or(f, |least| least.min(f)));
                    continue;
                }
                match self.insert(&parent, child) {
                    Inserted::Queued => {
                        best = self.incumbent_moves();
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
        if let Some(f) = deferred {
            // In place of the entry just popped, so the queue holds no more
            // entries than before. f > limit, so the queued h rises.
            let h = (f - g).min(u64::from(MAX_QUEUED_H)) as u32;
            let key = push::key(parent.node.g, h, self.policy.weight);
            self.arena.enqueue(key, h, parent.index);
        }
        ControlFlow::Continue(())
    }
    /// Runs one push through [`push::admit`] with the kernel of this
    /// engine's [`Parts`], lent per push because each insert needs the
    /// whole engine, then prices the child with the room bound, if any.
    /// `reopen_closed` and `best` are the values [`Self::expand`] keeps
    /// current across its inserts, and the caches are the parent's, shared
    /// by its children: M's label group, and the room bound's hall part.
    #[inline]
    fn admit(
        &mut self,
        parent: &Parent,
        parent_h: u32,
        (parent_group, room_parent): (&mut ParentGroup, &mut Option<RoomParent>),
        push: Push,
        reopen_closed: bool,
        best: Option<u32>,
    ) -> Option<Child> {
        debug_assert_eq!(
            (reopen_closed, best),
            (self.policy.reopen_closed, self.incumbent_moves())
        );
        let moved = push.moved();
        let mut child = push::admit(
            &mut self.parts().kernel(),
            parent,
            parent_h,
            parent_group,
            push,
            reopen_closed,
            best,
        )?;
        // The child is queued with max(M, S), admissible as both are
        // (docs/room-bound.md). Its record keeps M, so a cheaper duplicate
        // reuses M and is priced again here. The prunes push::admit ran with
        // M hold for any admissible h, and the stand walk must stay on M: it
        // adds moves M leaves uncounted, which S may count.
        if let Some(rooms) = &self.rooms {
            let cache = room_parent.get_or_insert(RoomParent::EMPTY);
            let Some(s) = rooms.child_estimate(&parent.node.state, cache, &child.node.state, moved)
            else {
                self.skipped.pruned_assignment += 1;
                return None;
            };
            if s > child.h {
                let g = u64::from(child.node.g);
                if best.is_some_and(|best| g + u64::from(s) >= u64::from(best)) {
                    self.skipped.pruned_bound += 1;
                    return None;
                }
                child.h = s.min(MAX_QUEUED_H);
            }
        }
        Some(child)
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
                    let id = self.arena.insert(node, slot);
                    self.record(id);
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
            self.record(id);
            // A next phase takes over the rest of this expansion.
            if self.policy.stop_on_goal_push && !self.next_phase() {
                self.end(Status::Solved);
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
    /// Polishes the incumbent in an episode if a job fits the part of the
    /// arena's share it does not hold, less the best episode route; the
    /// search waits, its arena and incumbent kept, until the polish ends.
    /// Clears the episode either way, so the incumbent gets no other. False
    /// when no job fits or the route does not rebuild.
    #[cold]
    #[inline(never)]
    fn start_episode(&mut self) -> bool {
        self.episode_at = u32::MAX;
        // u32::MAX also means none is due, so a count that reached it would
        // land here: only a polishing search may polish.
        let (true, Some(id)) = (self.policy.polish, self.incumbent) else {
            return false;
        };
        let boxes = self.board.labels().len();
        // The arena holds no more than its share, so this is at most the
        // share's free part, and an episode allocates only while the search
        // waits: the budget holds whatever the arena does.
        let allowance = (self.arena.reserved_bytes() as u64)
            .saturating_sub(self.charged_bytes())
            .saturating_sub(self.arena.held_bytes())
            .saturating_sub(self.polished_bytes());
        if !Polish::fits(allowance, self.arena.meta(id).g) {
            return false;
        }
        let Ok(route) = self.route_to(id) else {
            return false;
        };
        // An episode never kicks.
        self.polish = Some(Polish::new(route, Status::Running, allowance, boxes, 0));
        true
    }
    /// The bytes the best episode route holds, which a polisher's allowance
    /// leaves out.
    fn polished_bytes(&self) -> u64 {
        self.polished.as_ref().map_or(0, |route| route.len() as u64)
    }
    /// Runs the polish one unit per pop. Once the end polish ends, gives the
    /// search the status it ended with; once an episode ends, keeps its
    /// route if shorter than the best episode route and the incumbent, frees
    /// the rest, and returns the pops left for the search. Returns 0 when
    /// the pops run out first.
    // Cold and out of line, as advance_staged: it runs only after a Quality
    // search has ended or stalled.
    #[cold]
    #[inline(never)]
    fn advance_polish(&mut self, pops: u32) -> u32 {
        let Some(polisher) = &mut self.polish else {
            return pops;
        };
        let Some(n) = (0..pops)
            .find(|_| polisher.step(&self.board, &self.heuristic, &self.start, &mut self.reach))
        else {
            return 0;
        };
        if polisher.ending() != Status::Running {
            self.status = polisher.ending();
            return 0;
        }
        // An episode ended. With its polisher gone, best_moves is the
        // shorter of the best episode route and the incumbent.
        if let Some(route) = self.polish.take().map(Polish::into_best)
            && self
                .best_moves()
                .is_none_or(|best| (route.len() as u32) < best)
        {
            self.polished = Some(route);
        }
        pops - n - 1
    }
    /// Rebuilds the route to the solved record `id` as direction indices and
    /// replays it from the start: it must reach its first solved state at its
    /// last move, after exactly the record's g moves. No record on a path is
    /// solved, since a solved state is never expanded, so only the last push
    /// solves. Costs O(route) plus one flood per push.
    fn route_to(&mut self, id: u32) -> Result<Vec<u8>, SolutionError> {
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
        let solved_at = polish::replay(&self.board, &self.start, &route)?;
        if solved_at != expected || route.len() != expected {
            return Err(SolutionError::Counters);
        }
        Ok(route)
    }
    /// The best route as letters, replayed from the start: the shortest the
    /// polisher or an episode holds, unless the incumbent is shorter, which
    /// is then rebuilt; a tie keeps the held route. Costs O(route), plus one
    /// flood per push for the incumbent's, so call it once per improved
    /// route.
    pub(crate) fn solution(&mut self) -> Result<Option<String>, SolutionError> {
        let held = [
            self.polish.as_ref().map(Polish::best),
            self.polished.as_deref(),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|route| route.len());
        if let Some(route) = held
            && self
                .incumbent_moves()
                .is_none_or(|g| route.len() <= g as usize)
        {
            if polish::replay(&self.board, &self.start, route)? != route.len() {
                return Err(SolutionError::Counters);
            }
            return Ok(Some(letters(route)));
        }
        let Some(id) = self.incumbent else {
            return Ok(None);
        };
        self.route_to(id).map(|route| Some(letters(&route)))
    }
}

/// A route's direction indices as `UDLR` letters.
fn letters(route: &[u8]) -> String {
    route
        .iter()
        .map(|&direction| ACTIONS[direction as usize] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Engine, Policy, fixed_bytes, same};
    use crate::{
        MAX_STATES, MEMORY_MIB_RANGE, SearchError, SearchStats, Status, StopReason,
        arena::Arena,
        stage,
        testkit::{catalog, huge},
    };
    use sokomind_core::{Board, Game, MAX_BOXES, MAX_CELLS};

    /// Small enough for debug builds.
    const STATES: usize = 1_000;

    /// Runs `policy` to a terminal status one pop at a time.
    fn run(board: &Board, policy: Policy, max_states: usize) -> Engine {
        let mut engine =
            Engine::new(board.clone(), board.initial(), policy, max_states, 16).unwrap();
        drive(&mut engine);
        engine
    }

    /// Advances `engine` to a terminal status one pop at a time.
    fn drive(engine: &mut Engine) {
        while engine.status() == Status::Running {
            engine.advance(1);
        }
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

    /// Checks Quality mode against Fast at one limit: Quality starts as Fast,
    /// so it ends with a route whenever Fast does, never a longer one, and
    /// the route it ends with replays to the length it reports. It never
    /// generates more than its two arenas hold, each with at most the spare
    /// node past the limit, and keeps its counter identities.
    fn check_quality(context: &str, board: &Board, max_states: usize) -> Engine {
        let fast = run(board, Policy::FAST, max_states);
        let mut quality = run(board, Policy::FAST_THEN_QUALITY_RESTART, max_states);
        assert_counters(&quality);
        assert!(
            quality.generated() as usize <= 2 * (max_states + 1),
            "{context}"
        );
        if let Some(moves) = fast.best_moves() {
            assert!(
                quality.best_moves().is_some_and(|best| best <= moves),
                "{context}: {:?} after Fast's {moves}",
                quality.best_moves()
            );
        }
        if let Some(best) = quality.best_moves() {
            let route = quality.solution().unwrap().unwrap();
            let mut game = Game::new(board.clone());
            game.replay(&route).unwrap();
            assert!(game.solved(), "{context}");
            assert_eq!(game.moves(), best, "{context}");
        }
        quality
    }

    /// Quality mode on the whole catalog.
    #[test]
    fn quality_never_ends_longer_than_fast() {
        for (id, board) in catalog() {
            check_quality(&id, &board, STATES);
        }
    }

    /// Sweeps every limit on two small boards, through the one where Fast's
    /// route takes the spare node, past the limits where Quality starts over
    /// without a route, carrying one, or with no room left to start over.
    /// Quality ends the same in slices of one pop as in slices of 2^20.
    #[test]
    fn restart_survives_state_limit_boundaries() {
        let boards = catalog().into_iter();
        for (id, board) in boards.filter(|(id, _)| id == "ultra-tiny" || id == "tiny") {
            let fast = run(&board, Policy::FAST, STATES);
            for max_states in 1..=fast.generated() as usize + 2 {
                let context = format!("{id} at {max_states} states");
                let sliced = check_quality(&context, &board, max_states);
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
            }
        }
    }

    /// An arena that cannot grow ends the search with `MemoryLimit` and the
    /// f of the expansion it cut short, and never restarts, so it discards
    /// nothing, even under a restart policy: a fresh search would need the
    /// same allocation. Growth is refused from the start, so the search
    /// stops once its first chunk is full. The board is the one of
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
                (engine.status(), engine.discarded),
                (Status::MemoryLimit, 0)
            );
            assert!(engine.interrupted_f.is_some() && engine.best_moves().is_none());
        }
    }

    /// The board of `an_empty_goal_behind_a_settled_corral_is_dead` in the
    /// corral tests: A pushed into the corridor before B has passed seals
    /// B's goal off. Every run ends Solved with the 18-move route, which
    /// Exact proves shortest. Fast and Exact without its room bound prune
    /// such a state with the sealed-corral check. With the room bound, Exact
    /// never expands one, so the check that Exact's search counts runs
    /// without it.
    #[test]
    fn corral_check_prunes_a_goal_sealed_too_early() {
        let board = Board::parse("OOOOOOO\nO     O\nO BRA O\nOOOaOOO\nO  b  O\nOOOOOOO").unwrap();
        let roomless = Policy {
            room_bound: false,
            ..Policy::EXACT
        };
        for (mode, policy, corrals) in [
            ("exact", Policy::EXACT, false),
            ("exact without rooms", roomless, true),
            ("fast", Policy::FAST, true),
        ] {
            let engine = run(&board, policy, STATES);
            assert_eq!(
                (engine.status(), engine.best_moves()),
                (Status::Solved, Some(18)),
                "{mode}"
            );
            if corrals {
                assert!(engine.stats().pruned_corrals > 0, "{mode}");
            }
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

    /// Rebuilds `engine`'s route and replays it in a fresh game on huge,
    /// which it must solve in the moves the engine counts. Returns its moves
    /// and pushes.
    fn replay_on_huge(engine: &mut Engine) -> (u32, u32) {
        let route = engine.solution().unwrap().expect("a route");
        let mut game = Game::new(huge());
        game.replay(&route).unwrap();
        assert!(game.solved());
        assert_eq!(Some(game.moves()), engine.best_moves());
        (game.moves(), game.pushes())
    }

    /// Drives `engine` to a terminal status one pop at a time, checking that
    /// it never stages, and that it ends with no route.
    fn drive_unstaged(engine: &mut Engine) {
        while engine.status() == Status::Running {
            engine.advance(1);
            assert!(engine.staging.is_none());
        }
        assert!(engine.best_moves().is_none());
    }

    /// Drives `engine` to a terminal status one pop at a time, checking that
    /// a ladder runs on the way. A ladder ends before its search does, so it
    /// has run to its end.
    fn drive_staged(engine: &mut Engine) {
        let mut staged = false;
        while engine.status() == Status::Running {
            engine.advance(1);
            staged |= engine.staging.is_some();
        }
        assert!(staged, "no ladder ran");
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
        }
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
    /// counter in u32 (docs/stage-ladder.md, Counter width): two arenas of
    /// at most L + 1 records, the ladder's work, and one live copy of a root
    /// per reseed, at most 12,870 of them at this limit's 64 backjumps. The
    /// counter identities hold across the ladder's reseeds.
    #[test]
    fn staged_generated_bound() {
        let mut engine = fresh_on_huge(Policy::FAST, STAGED);
        drive_staged(&mut engine);
        let limit = STAGED as u64;
        let bound = 2 * (limit + 1) + u64::from(stage::work(STAGED, STAGED)) + 12_870;
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
        let mut engine = fresh_on_huge(policy, STAGED);
        drive_staged(&mut engine);
        let one = ended(&mut engine);
        assert_eq!(ended(&mut on_huge(policy, STAGED, 1 << 20)), one);
    }

    /// Fast and Quality mode on huge at 20,000 states, where Fast fills the
    /// arena with no route and ends with the staged route. Quality mode
    /// starts over as plain Quality with it as the bound, at the pop that
    /// ends the ladder, and ends no longer; both routes replay, and the
    /// counter identities hold after the handoff.
    #[test]
    fn fast_and_quality_handoffs() {
        const LIMIT: usize = 20_000;
        let mut fast = fresh_on_huge(Policy::FAST, LIMIT);
        drive_staged(&mut fast);
        assert_eq!(fast.status(), Status::Solved);
        let (staged, _) = replay_on_huge(&mut fast);
        let mut restart = fresh_on_huge(Policy::FAST_THEN_QUALITY_RESTART, LIMIT);
        while restart.staging.is_none() {
            assert_eq!(restart.status(), Status::Running);
            restart.advance(1);
        }
        while restart.staging.is_some() {
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
            assert!(engine.staging.is_none());
            assert_eq!((engine.status(), engine.best_moves()), (status, None));
            let generated = engine.generated();
            engine.advance(1);
            assert_eq!((engine.status(), engine.generated()), (status, generated));
        }
    }

    /// A state reserves a 12-byte record, two bytes per box, an 8-byte queue
    /// entry and 8 to 16 bytes of table, so the first budget that holds a
    /// full `MAX_STATES` depends on the box count and the target. From one
    /// box to `MAX_BOXES` on `MAX_CELLS` cells, with
    /// or without keeper sides' larger tables: every budget below the first
    /// that holds the full limit scales it to what fits, its bytes reserved
    /// within the budget, and that budget and every larger one hold exactly
    /// the limit, so a full search ends `MemoryLimit` below it and
    /// `StateLimit` from it on. Every budget is sized without allocating,
    /// through [`Arena::capacity`]: an arena at the top would reserve chunk
    /// lists for millions of the four-record chunks unit tests use.
    #[test]
    fn memory_binds_a_full_limit_below_the_budget_that_holds_it() {
        for (cells, boxes, sides) in [
            (4, 1, false),
            (MAX_CELLS, 1, false),
            (4, MAX_BOXES, false),
            (MAX_CELLS, MAX_BOXES, false),
            (MAX_CELLS, MAX_BOXES, true),
        ] {
            let fixed = fixed_bytes(cells, boxes, sides);
            let mut held = false;
            for memory_mib in MEMORY_MIB_RANGE {
                let capacity = Arena::capacity(fixed, boxes, MAX_STATES, memory_mib).unwrap();
                assert!(capacity.bytes <= memory_mib << 20);
                held |= capacity.limit == MAX_STATES;
                if held {
                    assert_eq!(
                        capacity.limit, MAX_STATES,
                        "{cells} cells, {boxes} boxes, sides {sides}, {memory_mib} MiB"
                    );
                } else {
                    assert!(
                        capacity.limit < MAX_STATES,
                        "{cells} cells, {boxes} boxes, sides {sides}, {memory_mib} MiB"
                    );
                }
            }
        }
    }

    /// The edge at 64 MiB for the largest board and box count: the limit
    /// [`Arena::capacity`] sizes is kept as asked and fits the budget, and
    /// one more state scales the request back to it.
    #[test]
    fn a_limit_one_past_the_budget_is_scaled_to_it() {
        let fixed = fixed_bytes(MAX_CELLS, MAX_BOXES, false);
        let limit = Arena::capacity(fixed, MAX_BOXES, MAX_STATES, 64)
            .unwrap()
            .limit;
        let fits = Arena::new(fixed, MAX_BOXES, limit, 64).unwrap();
        assert_eq!(fits.node_limit(), limit);
        assert_eq!(fits.limit_status(), Status::StateLimit);
        assert!(fits.reserved_bytes() <= 64 << 20);
        drop(fits);
        let scaled = Arena::new(fixed, MAX_BOXES, limit + 1, 64).unwrap();
        assert_eq!(scaled.node_limit(), limit);
        assert_eq!(scaled.limit_status(), Status::MemoryLimit);
    }

    /// A cell count too large to count in bytes never fits, on wasm32 and
    /// 64-bit alike: the fixed bytes saturate instead of wrapping to a small
    /// total.
    #[test]
    fn a_board_too_large_to_count_never_fits() {
        let fixed = fixed_bytes(usize::MAX, MAX_BOXES, false);
        for memory_mib in [*MEMORY_MIB_RANGE.start(), *MEMORY_MIB_RANGE.end()] {
            assert_eq!(
                Arena::capacity(fixed, MAX_BOXES, MAX_STATES, memory_mib).err(),
                Some(SearchError::BudgetTooSmall)
            );
        }
    }
}
