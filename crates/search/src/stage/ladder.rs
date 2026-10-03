//! The stage ladder: the frames, stages and backjumps that run the stage
//! units inside the search's arena. Ports the probe's `run`, `stage`,
//! `stage_expand` and `fail` (slurm/probes/p4b.rs, not tracked), as
//! section 4.5 of slurm/reports/stage-port-plan.md (not tracked) restates
//! them; section 4.8 lists the differences from the probe.
//!
//! A frame is a root, a state on the arena's kept path, with its level
//! (see `rooms`). Frame 0 is the start. A stage searches from the top
//! frame's root for a rise, a state above the root's level that passes
//! accept, and commits the first one it finds: the rise becomes the root of
//! a new frame. Each frame is a level above the one below it, so at most
//! [`FRAMES`] frames exist. A child that reaches every goal ends the ladder
//! with its route.
//!
//! - Rungs and focus: each frame starts at rung 0. The rung sets the
//!   stage's focus, the first `FOCUS[rung]` boxes of the root's ranked
//!   candidates (see `rank`), from which `movers` picks the boxes the stage
//!   may push; the rest stay frozen. The last rung moves every box. A stage
//!   that fails below the last rung climbs a rung and restarts from the
//!   same root.
//! - Search: best-first in the arena's queue at the search's weight. A
//!   child at or below the root's level is queued. A rise is held back:
//!   after each expansion, accept runs on its rises in push order (bans,
//!   then the checks of `checks`), and the first that passes is committed.
//!   An expansion holds at most [`MAX_RISES`] rises; past that a rise stays
//!   in the table, neither queued nor checked, as in the probe.
//! - Caps: a stage fails when its queue empties, when the arena holds
//!   `CAPS[rung]` records past the stage's start or is full, or when the
//!   work budget, the inserts the whole ladder may make, runs out.
//! - Bans and the culprit: when the last rung fails, the ladder backjumps.
//!   The culprit is the latest frame `q >= 1` whose root has a box, on a
//!   cell the failing stage's plan needs, that frame `q - 1`'s root does
//!   not. The ladder returns to frame `q - 1` and bans that box's group on
//!   that cell, the least such cell. With no culprit, it returns one frame
//!   and bans the failing root's signature: its filled goals and misplaced
//!   count. A cell ban rejects a rise that holds a box of the group on the
//!   cell when the frame root does not; a signature ban, a rise with that
//!   signature. A frame holds at most [`MAX_BANS`] bans. On its last ban it
//!   goes to the last rung without a restart, so the next pass backjumps
//!   past it. A ladder backjumps at most [`MAX_BACKTRACKS`] times.
//! - Reseed: each stage starts on a pruned arena. `Arena::clear_keeping`
//!   keeps only the path to the root, whose ids are then its depths, and a
//!   live copy of the root goes into the table and the queue. The stage's
//!   records start after it. Reseeds run only at `begin`, at each commit
//!   and at each restart. The records they drop count toward the search's
//!   discarded total through [`Ladder::take_dropped`].
//! - Checks budget: every unit but an expansion costs one check, out of
//!   `CHECK_FACTOR` times the work budget, and at zero the ladder gives up.
//!   Expansions are bounded by work already, since every queued record
//!   came from a counted insert or a reseed, so the budget bounds the
//!   ladder's whole time at every board size. The probe has no such budget;
//!   `CHECK_FACTOR` is provisional until P3 measures the largest checks to
//!   work ratio.
//! - Memory: `Ladder::bytes_for(cells)` heap bytes, reserved in
//!   [`Ladder::new`] and never grown, so no unit allocates. For `MAX_CELLS`
//!   cells it is asserted to fit `arena::ROUTE_ALLOWANCE`: every search is
//!   charged that allowance for the route it may build, and the ladder runs
//!   only while the search has no route, so it adds nothing to the budget.
//!
//! Lane and Line in accept have no soundness argument (see the module docs
//! of stage.rs), so the ladder serves Fast and Quality only, and nothing it
//! finds may feed a proof, a bound or a prune of the exact search.
use super::{
    Candidate, FOCUS, FRAMES, Facts, MAX_CANDIDATES, Scratch, Tools, bit,
    checks::{self, Check, Verdict},
    movers::{Movers, MoversUnit},
    need_row,
    rank::{self, RankUnit},
    rooms::Rooms,
    words,
};
use crate::{
    arena::{self, Arena},
    corral::Pockets,
    deadlock::{GoalReach, SinkLines},
    heuristic::{Heuristic, ParentGroup},
    push::{self, Child, Parent, Parts},
    reach::Blocks,
};
use sokomind_core::{Board, Cell, MAX_CELLS, State};
use std::{collections::TryReserveError, mem::size_of};

/// The inserts a stage may make past its start, by rung.
const CAPS: [usize; 3] = [16_384, 65_536, 131_072];
/// The last rung, which moves every box.
const LAST_RUNG: u8 = 2;
/// The most rises one expansion holds for accept.
const MAX_RISES: usize = 128;
/// The most bans a frame holds.
const MAX_BANS: usize = 4;
/// The most backjumps a ladder makes.
const MAX_BACKTRACKS: u32 = 64;
/// The checks budget per unit of work. Provisional: P3 sets it from the
/// largest checks to work ratio the release tests see.
const CHECK_FACTOR: u32 = 4;

const _: () = assert!(FOCUS.len() == CAPS.len());
// A u8 widens to usize; `usize::from` is not const.
const _: () = assert!(CAPS.len() == LAST_RUNG as usize + 1);
// A rise's index fits the u8 in `Phase::Accept`.
const _: () = assert!(MAX_RISES <= 1 << u8::BITS);
// 16 bytes of scalars and four 8-byte bans.
const _: () = assert!(size_of::<Frame>() == 48);
const _: () = assert!(Ladder::bytes_for(MAX_CELLS) <= arena::ROUTE_ALLOWANCE);

/// What a ban rejects. The group is the key [`group_key`] gives.
#[derive(Clone, Copy)]
enum Ban {
    /// A box of the group on the cell, when the frame root has none there.
    Cell(Cell, u8),
    /// The signature `Rooms::sig` gives: the filled goals and the count of
    /// misplaced boxes.
    Mask(u32, u8),
}

/// One rung of the ladder's state: a root on the kept path and what its
/// stages have learned.
struct Frame {
    /// The root's id, which is its depth on the kept path.
    depth: u32,
    /// `checks::root_lanes` of the root.
    root_lanes: u32,
    /// The root's filled goals, as `Rooms::sig` gives them.
    filled: u32,
    /// The root's level.
    level: i8,
    /// The rung the frame's next stage runs at.
    rung: u8,
    /// The number of bans in use.
    nbans: u8,
    /// The root's count of misplaced boxes, as `Rooms::sig` gives it.
    misplaced: u8,
    /// `bans[..nbans]` are in use.
    bans: [Ban; MAX_BANS],
}

/// Why a stage failed.
#[derive(Clone, Copy)]
enum Why {
    /// The queue emptied.
    Empty,
    /// The stage's insert cap, or the arena's limit, was reached.
    Cap,
    /// The work budget ran out.
    Work,
}

/// One accept unit of a rise.
#[derive(Clone, Copy)]
enum Step {
    /// The frame's bans.
    Ban,
    /// One unit of `checks::run`.
    Check(Check),
}

/// The unit the ladder runs next.
#[derive(Clone, Copy)]
enum Phase {
    /// One unit of ranking the root's candidates.
    Rank(RankUnit),
    /// One unit of choosing the stage's movers.
    Movers(MoversUnit),
    /// One expansion.
    Search,
    /// One accept unit of rise `rises[rise]`.
    Accept { rise: u8, step: Step },
    /// Commit the record with this id as a new frame's root.
    Commit(u32),
    /// One pass of the fail loop.
    Fail(Why),
    /// Ended, or never begun: every later step gives up.
    Over,
}

/// How a ladder ended.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Done {
    /// The record with this id is solved; its route is the ladder's route.
    Solved(u32),
    /// The ladder found no route within its budgets.
    GaveUp,
}

/// What a ladder did, for tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tally {
    /// Inserts charged to the work budget.
    pub(crate) work: u32,
    /// Rises committed as frames.
    pub(crate) commits: u32,
    /// Stages that failed.
    pub(crate) failures: u32,
    /// Backjumps.
    pub(crate) backtracks: u32,
    /// The highest frame index pushed.
    pub(crate) deepest: u32,
    /// Rises that reached the ban step.
    pub(crate) accepts: u32,
    /// Rejected rises by the step that rejected them: ban, Lane, Sink,
    /// Line, Matched, the pocket check.
    pub(crate) rejects: [u32; 6],
    /// Checks spent.
    pub(crate) checks: u32,
    /// Whether the checks budget ended the ladder.
    pub(crate) checks_bound: bool,
}

/// The stage ladder of one search; see the module docs. It owns its facts
/// and buffers, and borrows the search's components one unit at a time.
pub(crate) struct Ladder {
    facts: Facts,
    scratch: Scratch,
    /// The frames, bottom first. Reserved at [`FRAMES`].
    frames: Vec<Frame>,
    /// The current expansion's rises, in push order. Reserved at
    /// [`MAX_RISES`].
    rises: Vec<u32>,
    /// The current stage's movers.
    movers: Movers,
    /// The top frame's root.
    root: State,
    phase: Phase,
    /// The inserts left.
    work: u32,
    /// The checks left.
    checks: u32,
    /// The search's weight, for queue keys.
    weight: u32,
    /// Whether a cheaper duplicate of a closed state is admitted.
    reopen_closed: bool,
    /// The arena's length after the current stage's reseed.
    stage_base: usize,
    /// The arena length at which the current stage fails.
    cap_end: usize,
    /// The backjumps made.
    backtracks: u32,
    /// Records the reseeds dropped since the last [`Ladder::take_dropped`].
    dropped: u32,
    #[cfg(test)]
    tally: Tally,
}

impl Ladder {
    /// Heap bytes [`Ladder::new`] reserves on a board of `cells` cells: the
    /// facts, the scratch and the ladder's own two vectors.
    const fn bytes_for(cells: usize) -> usize {
        let row = words(cells);
        let facts = Blocks::bytes_for(cells)
            + GoalReach::bytes_for(cells)
            + SinkLines::bytes_for(cells)
            + Rooms::bytes_for(cells);
        let scratch = 4 * cells * size_of::<u16>()
            + cells * size_of::<u8>()
            + FRAMES * row * size_of::<u32>()
            + MAX_CANDIDATES * size_of::<Candidate>()
            + Pockets::BYTES
            + row * size_of::<u32>();
        let own = FRAMES * size_of::<Frame>() + MAX_RISES * size_of::<u32>();
        facts + scratch + own
    }

    /// A ladder for `board` and its `heuristic`, with every buffer reserved
    /// and the facts built from `start`, a state of the search. It may make
    /// `work` inserts, keys the queue at `weight` and admits cheaper
    /// duplicates of closed states only with `reopen_closed`, as the
    /// search's policy does. Runs nothing until [`Ladder::begin`].
    pub(crate) fn new(
        board: &Board,
        heuristic: &Heuristic,
        start: &State,
        work: u32,
        weight: u32,
        reopen_closed: bool,
    ) -> Result<Self, TryReserveError> {
        let mut scratch = Scratch::new(board.tiles().len())?;
        let facts = Facts::build(board, heuristic, start.player, &mut scratch)?;
        let mut frames = Vec::new();
        frames.try_reserve_exact(FRAMES)?;
        let mut rises = Vec::new();
        rises.try_reserve_exact(MAX_RISES)?;
        Ok(Self {
            facts,
            scratch,
            frames,
            rises,
            movers: Movers::new(&[], 0, 0),
            root: *start,
            phase: Phase::Over,
            work,
            checks: work.saturating_mul(CHECK_FACTOR),
            weight,
            reopen_closed,
            stage_base: 0,
            cap_end: 0,
            backtracks: 0,
            dropped: 0,
            #[cfg(test)]
            tally: Tally::default(),
        })
    }

    /// Starts the ladder on `parts`, whose arena's record 0 is the start:
    /// reseeds it and pushes frame 0. Call it once, before any step.
    pub(crate) fn begin(&mut self, parts: &mut Parts<'_>) {
        debug_assert!(self.frames.is_empty(), "a ladder begins once");
        // A refused reseed leaves the phase at Over, so the first step
        // gives up.
        let Some(k) = self.reseed(parts, 0) else {
            return;
        };
        let root = parts.arena.node(k).state;
        self.push_frame(parts, k, &root);
    }

    /// Runs one unit. Gives `None` while the ladder runs, then how it
    /// ended, and `GaveUp` on every later call. Every unit but an expansion
    /// first spends a check.
    pub(crate) fn step(&mut self, parts: &mut Parts<'_>) -> Option<Done> {
        if !matches!(self.phase, Phase::Search | Phase::Over) {
            if self.checks == 0 {
                #[cfg(test)]
                {
                    self.tally.checks_bound = true;
                }
                self.phase = Phase::Over;
                return Some(Done::GaveUp);
            }
            self.checks -= 1;
            #[cfg(test)]
            {
                self.tally.checks += 1;
            }
        }
        let done = match self.phase {
            Phase::Rank(unit) => self.rank_unit(parts, unit),
            Phase::Movers(unit) => self.movers_unit(parts, unit),
            Phase::Search => self.expand_one(parts),
            Phase::Accept { rise, step } => self.accept(parts, rise, step),
            Phase::Commit(id) => self.commit(parts, id),
            Phase::Fail(why) => self.fail(parts, why),
            Phase::Over => Some(Done::GaveUp),
        };
        if done.is_some() {
            self.phase = Phase::Over;
        }
        done
    }

    /// The records the reseeds dropped since the last call, which the
    /// search adds to its discarded count.
    pub(crate) fn take_dropped(&mut self) -> u32 {
        std::mem::take(&mut self.dropped)
    }

    /// What the ladder did so far.
    #[cfg(test)]
    pub(crate) fn tally(&self) -> Tally {
        self.tally
    }

    /// Pushes a frame for `root`, the kept path's record `depth`, with its
    /// need row cleared, and starts its first stage at `Rank(0)`.
    fn push_frame(&mut self, parts: &mut Parts<'_>, depth: u32, root: &State) {
        let (board, heuristic) = (parts.board, parts.heuristic);
        let rooms = &self.facts.rooms;
        let (filled, misplaced) = rooms.sig(board, heuristic, root);
        let level = rooms.level(board, heuristic, root);
        let root_lanes = checks::root_lanes(&mut tools(parts), root);
        let top = self.frames.len();
        // Each frame is a level above the one below it, and a level lies in
        // -MAX_BOXES..=MAX_BOXES, so the reserved frames always suffice.
        debug_assert!(top < FRAMES, "a frame per level");
        need_row(&mut self.scratch.need, board.tiles().len(), top).fill(0);
        self.frames.push(Frame {
            depth,
            root_lanes,
            filled,
            level,
            rung: 0,
            nbans: 0,
            misplaced,
            bans: [Ban::Mask(0, 0); MAX_BANS],
        });
        self.root = *root;
        #[cfg(test)]
        {
            // Below FRAMES, so it fits u32.
            self.tally.deepest = self.tally.deepest.max(top as u32);
        }
        self.phase = Phase::Rank(RankUnit::Rank(0));
    }

    /// One unit of ranking the root's candidates, then the movers' first
    /// unit at the frame's rung.
    fn rank_unit(&mut self, parts: &mut Parts<'_>, unit: RankUnit) -> Option<Done> {
        let lanes = self.top().root_lanes;
        let next = rank::run(
            &mut tools(parts),
            &self.facts,
            &mut self.scratch,
            &self.root,
            lanes,
            unit,
        );
        if let Some(unit) = next {
            self.phase = Phase::Rank(unit);
            return None;
        }
        let (rung, top) = (usize::from(self.top().rung), self.frames.len() - 1);
        self.movers = Movers::new(&self.scratch.candidates, rung, top);
        self.phase = Phase::Movers(MoversUnit::Path(0));
        None
    }

    /// One unit of choosing the movers, then the stage's search, with its
    /// insert cap set.
    fn movers_unit(&mut self, parts: &mut Parts<'_>, unit: MoversUnit) -> Option<Done> {
        let movers = &mut self.movers;
        let next = movers.run(&mut tools(parts), &mut self.scratch, &self.root, unit);
        if let Some(unit) = next {
            self.phase = Phase::Movers(unit);
            return None;
        }
        let rung = usize::from(self.top().rung);
        self.cap_end = self.stage_base.saturating_add(CAPS[rung]);
        self.phase = Phase::Search;
        None
    }

    /// One expansion: pops a record and inserts each admitted push of an
    /// unfrozen box, queueing those at or below the root's level and
    /// holding back the rises, then turns to accept on the first live rise.
    /// The cap, work, goal, rise and starved tests run in the probe's
    /// order. Insert, goal test and queue key are the engine's.
    fn expand_one(&mut self, parts: &mut Parts<'_>) -> Option<Done> {
        let Some(key) = parts.arena.dequeue() else {
            return self.fail_with(Why::Empty);
        };
        // A stale entry: a cheaper duplicate has since taken its slot.
        let Some(parent) = Parent::of(parts.arena, key) else {
            parts.skipped.stale_pops += 1;
            return None;
        };
        let (board, heuristic) = (parts.board, parts.heuristic);
        let state = parent.node.state;
        // A solved pop ends the ladder, as a goal pop ends the engine's
        // search. Goals end the ladder at insert, so only a solved start
        // gets here.
        if board.solved(&state) {
            return Some(Done::Solved(parent.index));
        }
        // A parent whose sealed corral has no solution gets no children,
        // and the stage goes on.
        let parent_h = parts.open(&parent)?;
        let level = self.top().level;
        let mut group = ParentGroup::EMPTY;
        self.rises.clear();
        let boxes = &state.boxes[..board.labels().len()];
        for (i, &from) in boxes.iter().enumerate() {
            if bit(&self.scratch.frozen, from) {
                continue;
            }
            for d in 0..4 {
                let Some(push) = push::legal(board, parts.reach, i, from, d) else {
                    continue;
                };
                let admitted = push::admit(
                    &mut parts.kernel(),
                    &parent,
                    parent_h,
                    &mut group,
                    push,
                    self.reopen_closed,
                    None,
                );
                let Some(child) = admitted else {
                    continue;
                };
                let arena = &mut *parts.arena;
                if arena.len() >= self.cap_end || arena.is_full() {
                    return self.fail_with(Why::Cap);
                }
                if self.work == 0 {
                    return self.fail_with(Why::Work);
                }
                // Read before the insert takes the node.
                let goal = child.is_goal(board);
                let Child { node, h, slot } = child;
                let rise = self.facts.rooms.level(board, heuristic, &node.state) > level;
                let id = arena.insert(node, slot);
                self.work -= 1;
                #[cfg(test)]
                {
                    self.tally.work += 1;
                }
                if goal {
                    return Some(Done::Solved(id));
                }
                // Past MAX_RISES a rise stays in the table, neither queued
                // nor checked, as in the probe.
                if !rise {
                    arena.enqueue(push::key(node.g, h, self.weight), h, id);
                } else if self.rises.len() < MAX_RISES {
                    debug_assert!(self.rises.len() < self.rises.capacity());
                    self.rises.push(id);
                }
                if arena.starved() {
                    return Some(Done::GaveUp);
                }
            }
        }
        self.phase = self.enter_rise(parts.arena, 0);
        None
    }

    /// Fails the stage: counts the failure and turns to the fail loop.
    fn fail_with(&mut self, why: Why) -> Option<Done> {
        #[cfg(test)]
        {
            self.tally.failures += 1;
        }
        self.phase = Phase::Fail(why);
        None
    }

    /// The phase for the first rise at index `from` or later that no
    /// cheaper duplicate superseded, at its ban step, or the search when no
    /// such rise is left. A superseded rise is skipped with no check, as in
    /// the probe.
    fn enter_rise(&self, arena: &Arena, from: usize) -> Phase {
        let rises = &self.rises[from..];
        let Some(k) = rises.iter().position(|&id| !arena.is_superseded(id)) else {
            return Phase::Search;
        };
        // Below MAX_RISES, which a const assert keeps within u8.
        let rise = (from + k) as u8;
        let step = Step::Ban;
        Phase::Accept { rise, step }
    }

    /// One accept unit of rise `rises[rise]`: its bans at `Step::Ban`, else
    /// one check. A pass commits the rise; a reject moves to the next rise.
    fn accept(&mut self, parts: &mut Parts<'_>, rise: u8, step: Step) -> Option<Done> {
        let id = self.rises[usize::from(rise)];
        let child = parts.arena.node(id).state;
        let verdict = match step {
            Step::Ban => {
                #[cfg(test)]
                {
                    self.tally.accepts += 1;
                }
                if self.banned(parts.board, parts.heuristic, &child) {
                    Verdict::Reject
                } else {
                    Verdict::Next(Check::Lane)
                }
            }
            Step::Check(check) => {
                let lanes = self.top().root_lanes;
                checks::run(
                    &mut tools(parts),
                    &self.facts,
                    &mut self.scratch.pockets,
                    &child,
                    lanes,
                    check,
                )
            }
        };
        self.phase = match verdict {
            Verdict::Next(check) => {
                let step = Step::Check(check);
                Phase::Accept { rise, step }
            }
            Verdict::Pass => Phase::Commit(id),
            Verdict::Reject => {
                #[cfg(test)]
                {
                    self.tally.rejects[reject_index(step)] += 1;
                }
                self.enter_rise(parts.arena, usize::from(rise) + 1)
            }
        };
        None
    }

    /// Whether a ban of the top frame rejects `child`.
    fn banned(&self, board: &Board, heuristic: &Heuristic, child: &State) -> bool {
        let frame = self.top();
        let bans = &frame.bans[..usize::from(frame.nbans)];
        bans.iter().any(|&ban| match ban {
            Ban::Cell(c, group) => {
                let held = |state: &State| holds(board, heuristic, state, c, group);
                held(child) && !held(&self.root)
            }
            Ban::Mask(filled, misplaced) => {
                self.facts.rooms.sig(board, heuristic, child) == (filled, misplaced)
            }
        })
    }

    /// Commits rise `id`: reseeds on it and pushes its frame.
    fn commit(&mut self, parts: &mut Parts<'_>, id: u32) -> Option<Done> {
        let child = parts.arena.node(id).state;
        let Some(k) = self.reseed(parts, id) else {
            return Some(Done::GaveUp);
        };
        let kept = parts.arena.node(k).state;
        debug_assert_eq!(kept, child, "the kept path ends at the rise");
        #[cfg(test)]
        {
            self.tally.commits += 1;
        }
        self.push_frame(parts, k, &child);
        None
    }

    /// One pass of the fail loop, in the probe's order: give up when the
    /// work or the backjumps are spent, climb a rung while one is left,
    /// give up at frame 0, else backjump to the culprit's frame with a new
    /// ban. A frame that takes its last ban goes to the last rung and stays
    /// in the fail loop, so the next pass backjumps past it.
    fn fail(&mut self, parts: &mut Parts<'_>, why: Why) -> Option<Done> {
        debug_assert!(!matches!(why, Why::Work) || self.work == 0);
        if self.work == 0 || self.backtracks >= MAX_BACKTRACKS {
            return Some(Done::GaveUp);
        }
        let top = self.frames.len() - 1;
        let frame = self.top_mut();
        if frame.rung < LAST_RUNG {
            frame.rung += 1;
            return self.restart(parts);
        }
        if top == 0 {
            return Some(Done::GaveUp);
        }
        self.backtracks += 1;
        #[cfg(test)]
        {
            self.tally.backtracks += 1;
        }
        let (target, ban) = self.culprit(parts);
        self.frames.truncate(target + 1);
        let frame = self.top_mut();
        // A frame takes its last ban at the last rung, so the next pass
        // backjumps below it before it could take another.
        debug_assert!(usize::from(frame.nbans) < MAX_BANS);
        frame.bans[usize::from(frame.nbans)] = ban;
        frame.nbans += 1;
        if usize::from(frame.nbans) >= MAX_BANS {
            frame.rung = LAST_RUNG;
            return None;
        }
        frame.rung = 0;
        self.restart(parts)
    }

    /// The frame to backjump to and the ban it takes; see the module docs.
    /// Refreshes Deadlock's occupancy.
    fn culprit(&self, parts: &mut Parts<'_>) -> (usize, Ban) {
        let (board, heuristic) = (parts.board, parts.heuristic);
        let boxes = board.labels().len();
        let top = self.frames.len() - 1;
        let row = words(board.tiles().len());
        let need = &self.scratch.need[top * row..(top + 1) * row];
        for q in (1..=top).rev() {
            let before = parts.arena.node(self.frames[q - 1].depth).state;
            let after = parts.arena.node(self.frames[q].depth).state;
            parts.deadlock.refresh(&before.boxes[..boxes]);
            let deadlock = &*parts.deadlock;
            let moved = after.boxes[..boxes]
                .iter()
                .enumerate()
                .filter(|&(_, &c)| deadlock.at(c).is_none() && bit(need, c))
                .min_by_key(|&(_, &c)| c);
            if let Some((i, &c)) = moved {
                return (q - 1, Ban::Cell(c, group_key(heuristic, i)));
            }
        }
        let frame = self.top();
        (top - 1, Ban::Mask(frame.filled, frame.misplaced))
    }

    /// Restarts the top frame's stage from its root at its rung.
    fn restart(&mut self, parts: &mut Parts<'_>) -> Option<Done> {
        let depth = self.top().depth;
        let Some(k) = self.reseed(parts, depth) else {
            return Some(Done::GaveUp);
        };
        debug_assert_eq!(k, depth, "the kept path's ids are its depths");
        self.root = parts.arena.node(depth).state;
        self.phase = Phase::Rank(RankUnit::Rank(0));
        None
    }

    /// Prunes the arena to the path to record `id` and queues a live copy
    /// of `id`'s state, which starts a stage. Gives `id`'s new id, its
    /// depth, or `None` when `Arena::clear_keeping` refuses the path or the
    /// state has no goal assignment.
    fn reseed(&mut self, parts: &mut Parts<'_>, id: u32) -> Option<u32> {
        let heuristic = parts.heuristic;
        let arena = &mut *parts.arena;
        let before = arena.len();
        let k = arena.clear_keeping(id)?;
        // A u32 widens to usize on every supported target.
        let kept = k as usize + 1;
        // At most the arena's limit, which fits u32, as every id does.
        self.dropped += (before - kept) as u32;
        let node = arena.node(k);
        let (slot, previous) = arena.find(&node.state);
        debug_assert!(previous.is_none(), "clear_keeping detaches the kept path");
        // Only a start can lack an assignment: every admitted child has one.
        let h = node.known_h().or_else(|| heuristic.estimate(&node.state))?;
        // The live copy keeps the kept record's parent, so a route through
        // it runs down the kept path.
        let live = arena.insert(node, slot);
        arena.enqueue(push::key(node.g, h, self.weight), h, live);
        self.stage_base = arena.len();
        Some(k)
    }

    /// The top frame.
    fn top(&self) -> &Frame {
        self.frames.last().expect("the ladder has a frame")
    }

    /// The top frame, to change.
    fn top_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("the ladder has a frame")
    }
}

/// Whether `state` has a box of the group keyed `group` on cell `c`.
fn holds(board: &Board, heuristic: &Heuristic, state: &State, c: Cell, group: u8) -> bool {
    let boxes = &state.boxes[..board.labels().len()];
    let i = boxes.iter().position(|&b| b == c);
    i.is_some_and(|i| group_key(heuristic, i) == group)
}

/// The key a cell ban records for box `i`'s group: the group's first slot,
/// which no other group shares. Below `MAX_BOXES` (32), so it fits u8.
fn group_key(heuristic: &Heuristic, i: usize) -> u8 {
    heuristic.group(i).start as u8
}

/// The units' borrows of `parts`.
fn tools<'a>(parts: &'a mut Parts<'_>) -> Tools<'a> {
    Tools {
        board: parts.board,
        heuristic: parts.heuristic,
        reach: parts.reach,
        deadlock: parts.deadlock,
        corral: parts.corral,
    }
}

/// The index in `Tally::rejects` of a reject at `step`.
#[cfg(test)]
fn reject_index(step: Step) -> usize {
    match step {
        Step::Ban => 0,
        Step::Check(Check::Lane) => 1,
        Step::Check(Check::Sink) => 2,
        Step::Check(Check::Line(_)) => 3,
        Step::Check(Check::Matched) => 4,
        Step::Check(Check::PocketsInit | Check::PocketPop) => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::{Board, Candidate, Facts, Frame, Heuristic, Ladder, MAX_CELLS, Scratch};
    use crate::testkit::huge;
    use std::mem::size_of;

    /// The heap bytes `ladder` holds. The facts and the scratch are
    /// destructured in full, so a new buffer in either breaks the build
    /// here.
    fn heap_bytes(ladder: &Ladder) -> usize {
        let Facts {
            blocks,
            goal_reach,
            sink_lines,
            rooms,
        } = &ladder.facts;
        let Scratch {
            pool,
            via,
            need,
            candidates,
            pockets,
            frozen,
        } = &ladder.scratch;
        let facts = blocks.heap_bytes()
            + goal_reach.heap_bytes()
            + sink_lines.heap_bytes()
            + rooms.heap_bytes();
        let scratch = pool.capacity() * size_of::<u16>()
            + via.capacity() * size_of::<u8>()
            + need.capacity() * size_of::<u32>()
            + candidates.capacity() * size_of::<Candidate>()
            + pockets.heap_bytes()
            + frozen.capacity() * size_of::<u32>();
        let own = ladder.frames.capacity() * size_of::<Frame>()
            + ladder.rises.capacity() * size_of::<u32>();
        facts + scratch + own
    }

    /// An open 64 by 64 board, `MAX_CELLS` cells, with 32 boxes of two
    /// labels, the most a board holds.
    fn widest() -> Board {
        let mut rows = vec![vec![b' '; 64]; 64];
        rows[0][0] = b'R';
        for k in 0..16 {
            let c = 4 * k + 2;
            rows[10][c] = b'A';
            rows[12][c] = b'B';
            rows[50][c] = b'a';
            rows[52][c] = b'b';
        }
        let text = String::from_utf8(rows.join(&b'\n')).expect("ASCII rows");
        let board = Board::parse(&text).expect("the widest board");
        assert_eq!(board.tiles().len(), MAX_CELLS);
        board
    }

    /// `Ladder::bytes_for` counts every byte `Ladder::new` reserves, on
    /// huge and on the widest board, and the struct itself stays small.
    #[test]
    fn capacity_matches_bytes_for() {
        for board in [huge(), widest()] {
            let heuristic = Heuristic::new(&board);
            let start = board.initial();
            let ladder = Ladder::new(&board, &heuristic, &start, 1 << 20, 5, false);
            let ladder = ladder.expect("the ladder's buffers");
            let cells = board.tiles().len();
            assert_eq!(heap_bytes(&ladder), Ladder::bytes_for(cells));
        }
        assert!(size_of::<Ladder>() <= 768);
    }
}
