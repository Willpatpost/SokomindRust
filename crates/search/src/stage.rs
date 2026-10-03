//! The Stage Ladder's parts, ported from the P4b stage probe
//! (slurm/probes/p4b.rs, not tracked): the rooms and levels that define a
//! rise, the accept checks a rise must pass, the ranked candidate moves of
//! a frame root, and the boxes a stage may move. This is step 7 of
//! slurm/reports/stage-port-plan.md (not tracked). The ladder that drives
//! these parts comes in step 8. Until step 9 wires it into the engine, only
//! tests reach this module.
//!
//! Accept is a heuristic filter, not a necessary condition. Sink, Matched
//! and the pocket check only reject states with no solution, as the module
//! docs of deadlock/goals.rs and corral.rs argue. Lane and Line have no
//! such argument. Lane compares the state with the frame root: when a
//! route closes every lane into an unfilled goal and later reopens one,
//! Lane rejects the states in between against a root from before the
//! closing. Line rejects pushes 78 to 80 of huge's 503-move route, which
//! `checks::tests::route_replay` pins. So the ladder serves Fast and
//! Quality only, and nothing here may feed a proof, a bound or a prune of
//! the exact search.
//!
//! The work comes in units of `O(cells)` each (a push search, a flood or a
//! cell scan, a constant number of times), so a caller can stop between any
//! two of them. Each unit function takes the unit to run and returns the
//! next one, or `None` when its sequence ends. Each submodule owns one part:
//!
//! - `rooms`: the rooms of the start position, which boxes are misplaced,
//!   a state's level and its signature.
//! - `checks`: the accept checks, one unit each.
//! - `rank`: a frame root's candidate moves, ranked and filtered.
//! - `movers`: the boxes a stage may move and the cells its plan needs.
//!
//! Three structs carry what the units share:
//! - [`Tools`] borrows the search components the units run on.
//! - [`Facts`] holds what is built once per board from its start position.
//! - [`Scratch`] holds every working buffer. Each is reserved once at its
//!   final size, so no unit allocates.
//!
//! Settled boxes, those on a goal of their own label, are never stored.
//! They are read from Deadlock's occupancy, which each unit first refreshes
//! to the state it checks.
mod checks;
mod movers;
mod rank;
#[cfg(test)]
mod reference;
mod rooms;

use crate::{
    corral::{Corral, Pockets},
    deadlock::{Deadlock, GoalReach, SinkLines},
    heuristic::Heuristic,
    reach::{Blocks, Reach},
};
use sokomind_core::{Board, Cell, MAX_BOXES, MAX_CELLS};
use std::{collections::TryReserveError, mem::size_of};

/// The number of boxes a stage's focus takes from the ranked candidates, by
/// rung. The last rung takes every box.
const FOCUS: [usize; 3] = [3, 6, usize::MAX];
/// The most boxes a stage may move below the last rung. The last rung moves
/// every box.
const MAX_MOVE: usize = 8;
/// The most frames a ladder holds, one per level. A level runs from
/// `-MAX_BOXES` to `MAX_BOXES`.
const FRAMES: usize = 2 * MAX_BOXES + 1;
/// The most candidates a frame root has: per box, up to two goal targets
/// and one room exit.
const MAX_CANDIDATES: usize = 3 * MAX_BOXES;
// A room's sort key packs its gate cell into 12 bits (see `rooms`).
const _: () = assert!(MAX_CELLS <= 1 << 12);

/// A candidate move of a frame root: push the box in `slot` along one
/// shortest path to `target`. Ranking sorts by (cost, kind, the box's cell,
/// target). That key is unique because a box has at most one candidate per
/// target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Candidate {
    /// The box's slot in the root.
    slot: u8,
    /// 0 for an unfilled goal of the box's group. 1 for the nearest floor
    /// cell outside every room, which only a misplaced box gets.
    kind: u8,
    /// The cell where the box ends.
    target: Cell,
    /// The keeper's stand for the first push.
    first_stand: Cell,
    /// The keeper's cell after the last push: the path's second-to-last
    /// cell, or the box's own cell for a one-push path.
    hyp_player: Cell,
    /// The path's length plus 2 for each other box on its cells or stands.
    /// Once the candidates are ordered, it also includes the keeper's walk
    /// to the first stand.
    cost: u32,
}
const _: () = assert!(size_of::<Candidate>() == 12);

/// The search components a unit runs on. The keeper flood, Deadlock's
/// occupancy and Corral's stamps are working state that a unit may
/// overwrite, so a unit may not rely on what an earlier one left in them,
/// with one exception: the pocket check keeps the keeper flood and
/// Corral's stamps from PocketsInit until its last PocketPop, so nothing
/// else may use them in between (see `checks::run`).
struct Tools<'a> {
    board: &'a Board,
    heuristic: &'a Heuristic,
    reach: &'a mut Reach,
    deadlock: &'a mut Deadlock,
    corral: &'a mut Corral,
}

/// What is known from the board and its start position, built once:
/// - the floor topology with one cell removed;
/// - the goals each box reaches alone;
/// - the sink lines;
/// - the rooms.
struct Facts {
    blocks: Blocks,
    goal_reach: GoalReach,
    sink_lines: SinkLines,
    rooms: rooms::Rooms,
}

impl Facts {
    /// Builds every fact from `board.initial()`. The builds borrow their
    /// scratch from `scratch`, whose regions the units use only after the
    /// builds end:
    /// - Blocks: its DFS stack and low-links from the pool, and its
    ///   direction cursors from `via`.
    /// - GoalReach: its queue from the pool.
    /// - Rooms: its sort keys from `need`, which holds at least `2 * cells`
    ///   words at every size, and its DFS order and both prefix sums from
    ///   the pool.
    fn build(
        board: &Board,
        heuristic: &Heuristic,
        scratch: &mut Scratch,
    ) -> Result<Self, TryReserveError> {
        let cells = board.tiles().len();
        let player = board.initial().player;
        let (stack, low) = scratch.pool.split_at_mut(cells);
        let blocks = Blocks::build(board, player, stack, low, &mut scratch.via)?;
        let queue = &mut scratch.pool[..2 * cells];
        let goal_reach = GoalReach::build(board, heuristic, &blocks, queue)?;
        let sink_lines = SinkLines::build(board)?;
        // The taken prefix gets the pool's last 2C - 1 entries, at least the
        // C + 1 it needs, since a board has at least a box and the robot.
        let (order, rest) = scratch.pool.split_at_mut(cells);
        let (prefix, taken) = rest.split_at_mut(cells + 1);
        let keys = &mut scratch.need[..2 * cells];
        let rooms = rooms::Rooms::build(board, heuristic, &blocks, keys, order, prefix, taken)?;
        Ok(Self {
            blocks,
            goal_reach,
            sink_lines,
            rooms,
        })
    }
}

/// Every working buffer the units share. Each one is reserved once at its
/// final size, and `new` sets the slices to their full length, so no unit
/// allocates and every push stays within reserved capacity.
struct Scratch {
    /// Four `cells`-long `u16` regions, which [`split`] separates: dist,
    /// queue, aux and path.
    pool: Vec<u16>,
    /// The direction of the last push into each cell in a push search.
    /// One candidate's movers `Leg` units read it and the pool's path
    /// region, which the first of them rebuilds, so nothing else may
    /// overwrite either until the last of them ends.
    via: Vec<u8>,
    /// One row of `words(cells)` words per frame: the cells each frame's
    /// stage plan needs, one bit each.
    need: Vec<u32>,
    /// A frame root's candidates, at most [`MAX_CANDIDATES`]. Reserved
    /// only, never resized.
    candidates: Vec<Candidate>,
    /// The pocket check's own state.
    pockets: Pockets,
    /// One bit per cell: the cells of the root's boxes that a stage may not
    /// move.
    frozen: Vec<u32>,
}

impl Scratch {
    /// Every buffer for a board of `cells` cells, at its final size.
    fn new(cells: usize) -> Result<Self, TryReserveError> {
        let row = words(cells);
        let mut candidates = Vec::new();
        candidates.try_reserve_exact(MAX_CANDIDATES)?;
        Ok(Self {
            pool: filled(4 * cells, 0)?,
            via: filled(cells, 0)?,
            need: filled(FRAMES * row, 0)?,
            candidates,
            pockets: Pockets::new()?,
            frozen: filled(row, 0)?,
        })
    }
}

/// A vector of `len` copies of `value`, reserved at exactly that length.
fn filled<T: Clone>(len: usize, value: T) -> Result<Vec<T>, TryReserveError> {
    let mut vec = Vec::new();
    vec.try_reserve_exact(len)?;
    vec.resize(len, value);
    Ok(vec)
}

/// The number of `u32` words a bitset over `cells` cells takes.
fn words(cells: usize) -> usize {
    cells.div_ceil(32)
}

/// Sets bit `c` of a bitset.
fn set_bit(bits: &mut [u32], c: Cell) {
    bits[usize::from(c) / 32] |= 1 << (c % 32);
}

/// Whether bit `c` of a bitset is set.
fn bit(bits: &[u32], c: Cell) -> bool {
    (bits[usize::from(c) / 32] >> (c % 32)) & 1 != 0
}

/// Frame `f`'s row of `need`, a bitset over `cells` cells.
fn need_row(need: &mut [u32], cells: usize, f: usize) -> &mut [u32] {
    let row = words(cells);
    &mut need[f * row..(f + 1) * row]
}

/// Splits the pool into its four `cells`-long regions:
/// - dist and queue: a push search or a flood;
/// - aux: a second distance array, such as a walk-blocker leg's flood back
///   from its end;
/// - path: a rebuilt box path, which outlives the unit that wrote it.
fn split(pool: &mut [u16], cells: usize) -> (&mut [u16], &mut [u16], &mut [u16], &mut [u16]) {
    debug_assert_eq!(pool.len(), 4 * cells);
    let (dist, rest) = pool.split_at_mut(cells);
    let (queue, rest) = rest.split_at_mut(cells);
    let (aux, path) = rest.split_at_mut(cells);
    (dist, queue, aux, path)
}

/// Whether cell `c` holds a box on a goal of its own label, as of the last
/// refresh of `deadlock`.
fn settled(board: &Board, deadlock: &Deadlock, c: Cell) -> bool {
    deadlock.at(c).is_some_and(|i| board.on_goal(i, c))
}
