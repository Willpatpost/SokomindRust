//! Incremental, platform-independent push A* for Sokomind boards. A search
//! reserves its arena, queue, state table and flood buffers once, within the
//! caller's state limit and memory budget, and never grows them.
//!
//! A [`Search`] runs one of three [`Mode`]s over the same engine. It expands
//! pushes rather than single steps, its costs count every move, the walks
//! between pushes included, and every mode prunes dead cells and frozen
//! boxes:
//!
//! - [`Mode::Fast`]: weighted A* that stops at its first route.
//! - [`Mode::Quality`]: Fast, then a lighter weight that keeps shortening
//!   that route until nothing queued can beat it or a limit hits.
//! - [`Mode::Optimal`]: admissible A*, and the only mode that yields a
//!   [`Proof`]: that the route is shortest, that the optimum lies between
//!   two bounds, or that no route exists.
//!
//! A caller drives a search in slices. [`Search::advance`] runs a bounded
//! number of queue pops; between slices the caller reads progress and may
//! [`Search::stop`] the search, until [`Search::status`] leaves
//! [`Status::Running`]. [`Search::solution`] hands over the best route as
//! `UDLR` letters, already replayed from the start.
//!
//! ```
//! use sokomind_core::{Board, Game};
//! use sokomind_search::{Mode, Proof, Search, Status};
//!
//! // The robot walks around the box, then pushes it right onto its goal.
//! let board = Board::parse("OOOOO\nO XSO\nO   O\nO R O\nOOOOO")?;
//! let mut search = Search::new(board.clone(), board.initial(), Mode::Optimal, 10_000, 16)?;
//! while search.status() == Status::Running {
//!     // A worker reports progress or checks its clock between slices.
//!     search.advance(64);
//! }
//! assert_eq!(search.status(), Status::Solved);
//! assert_eq!(search.proof(), Some(Proof::Optimal { moves: 4 }));
//! let route = search.solution()?.expect("a solved search has a route");
//! let mut game = Game::new(board);
//! game.replay(&route)?;
//! assert!(game.solved());
//! assert_eq!(game.moves(), 4);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
// The PEA* slack is one const (5.3 S2): exactly one C, never the bare switch.
#[cfg(any(
    all(feature = "pea0", feature = "pea1"),
    all(feature = "pea0", feature = "pea2"),
    all(feature = "pea1", feature = "pea2"),
))]
compile_error!("features pea0, pea1 and pea2 are mutually exclusive");
#[cfg(all(
    feature = "pea",
    not(any(feature = "pea0", feature = "pea1", feature = "pea2")),
))]
compile_error!("feature pea needs a slack: enable pea0, pea1 or pea2");
mod arena;
mod deadlock;
mod engine;
mod exact;
mod heuristic;
#[cfg(feature = "o6")]
mod keeper;
mod proof;
mod reach;
#[cfg(feature = "o2")]
mod sides;
#[cfg(test)]
mod testkit;

use engine::{Engine, Policy};
use exact::ExactSearch;
pub use proof::Proof;
use sokomind_core::{Board, MAX_ROUTE, State, StateError};
use std::ops::RangeInclusive;

/// Largest per-search state limit accepted anywhere. A state costs a 12-byte
/// record, two bytes per box, an 8-byte queue entry and 8 to 16 bytes of
/// index table, so at 64 MiB the memory budget binds first from about 20
/// boxes: 19 fit a full limit even at `MAX_CELLS` cells, and 20 do not even
/// on a tiny board (the arena test `memory_binds_from_twenty_boxes_at_64_mib`).
pub const MAX_STATES: usize = 1_000_000;
/// The `max_states` values [`Search::new`] accepts. Callers that validate
/// limits themselves check against this range, never a copy of it.
pub const MAX_STATES_RANGE: RangeInclusive<usize> = 1..=MAX_STATES;
/// The `memory_mib` budgets [`Search::new`] accepts. A caller may cap
/// requests lower but never below this range's start.
pub const MEMORY_MIB_RANGE: RangeInclusive<usize> = 4..=256;

/// Which search [`Search::new`] runs. Only `Optimal` can produce a [`Proof`].
/// [`Mode::as_str`] gives its wire name: the server's `mode` field, the WASM
/// search's `mode` argument and the benchmark corpus's `mode` key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Weighted A* (weight 5) that ends at its first route, however long.
    Fast,
    /// Fast until its first route, then weight 3 in the same arena, shortening
    /// the route until the queue empties or a limit hits. When Fast fills the
    /// arena without a route, the search starts over at weight 3.
    Quality,
    /// Admissible A* over moves: a shortest route, proved once the search
    /// finishes.
    Optimal,
}
impl Mode {
    /// Every mode in wire order: fast, quality, optimal.
    pub const ALL: [Self; 3] = [Self::Fast, Self::Quality, Self::Optimal];
    /// The wire name [`Mode::parse`] accepts: `fast`, `quality` or `optimal`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Quality => "quality",
            Self::Optimal => "optimal",
        }
    }
    /// The mode whose wire name is exactly `value`; case and surrounding
    /// whitespace count. Anything else is a [`ParseModeError`].
    pub fn parse(value: &str) -> Result<Self, ParseModeError> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .ok_or(ParseModeError)
    }
}
impl std::str::FromStr for Mode {
    type Err = ParseModeError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// [`Mode::parse`] got something other than a mode's wire name. The server
/// and the WASM search show its text as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseModeError;
impl std::fmt::Display for ParseModeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Mode must be fast, quality, or optimal")
    }
}
impl std::error::Error for ParseModeError {}

/// Where a search stands. It stays `Running` until [`Search::advance`] or
/// [`Search::stop`] ends the search; every other status is final. A final
/// status keeps whatever the search found: [`Search::best_moves`],
/// [`Search::solution`] and, for `Optimal`, [`Search::proof`] and
/// [`Search::lower_bound`] stay readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// More pops may find or improve a route.
    Running,
    /// Finished with a route: Fast at its first, Quality once its queue
    /// empties, Optimal once it pops a solved state. Only an `Optimal`
    /// search proves the route shortest; see [`Search::proof`].
    Solved,
    /// The queue emptied without a route, or the start has no
    /// label-compatible goal assignment. Only an `Optimal` search proves
    /// from this that none exists.
    Exhausted,
    /// The arena filled at the caller's `max_states`.
    StateLimit,
    /// The arena filled at the smaller state limit that `memory_mib` allows;
    /// see [`Search::new`].
    MemoryLimit,
    /// Stopped by [`Search::stop`] with [`StopReason::TimeLimit`].
    TimeLimit,
    /// Stopped by [`Search::stop`] with [`StopReason::Cancelled`].
    Cancelled,
}
impl Status {
    /// The snake_case wire name, such as `state_limit`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Solved => "solved",
            Self::Exhausted => "exhausted",
            Self::StateLimit => "state_limit",
            Self::MemoryLimit => "memory_limit",
            Self::TimeLimit => "time_limit",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The only reasons a caller may interrupt a search. Capacity limits and
/// terminal verdicts are determined internally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The caller gave up on the search.
    Cancelled,
    /// The caller's time budget ran out.
    TimeLimit,
}

/// Search construction failed before any caller-supplied state was indexed.
/// `InvalidState`, `Limits` and `BudgetTooSmall` are caller errors;
/// `Allocation` is a resource failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    /// The start state does not fit the board.
    InvalidState(StateError),
    /// `max_states` is outside [`MAX_STATES_RANGE`] or `memory_mib` is
    /// outside [`MEMORY_MIB_RANGE`].
    Limits,
    /// The memory budget cannot hold the board's fixed buffers and one
    /// state.
    BudgetTooSmall,
    /// The allocator refused a buffer the budget allows; names the buffer.
    Allocation(&'static str),
}
impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidState(error) => error.fmt(f),
            Self::Limits => write!(
                f,
                "Use {}..{} states and {}..{} MiB",
                MAX_STATES_RANGE.start(),
                MAX_STATES_RANGE.end(),
                MEMORY_MIB_RANGE.start(),
                MEMORY_MIB_RANGE.end()
            ),
            Self::BudgetTooSmall => f.write_str("Memory budget is too small"),
            Self::Allocation(buffer) => write!(f, "Cannot reserve {buffer}"),
        }
    }
}
impl std::error::Error for SearchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidState(error) => Some(error),
            Self::Limits | Self::BudgetTooSmall | Self::Allocation(_) => None,
        }
    }
}

/// Why [`Search::solution`] could not hand over the incumbent's route. Only
/// `TooLong` can follow from a caller's input; the others are internal
/// invariant failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolutionError {
    /// The route is longer than [`MAX_ROUTE`] moves, so no game replays it.
    TooLong,
    /// The rebuilt route did not replay from the start.
    Replay,
    /// The rebuilt route did not end solved at the incumbent's length.
    Counters,
}
impl std::fmt::Display for SolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong => write!(f, "Solution exceeds the {MAX_ROUTE}-move replay limit"),
            Self::Replay => f.write_str("Internal route replay failed"),
            Self::Counters => f.write_str("Internal solution counters failed"),
        }
    }
}
impl std::error::Error for SolutionError {}

/// Counters from the current search. Generated records include immutable
/// improved versions of existing states; unique states count table entries.
/// A Quality search that restarts counts both runs, so its generated records
/// may exceed the state limit by up to one full arena.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Distinct canonical box positions AND exact player positions indexed,
    /// including those of an arena a Quality restart discarded.
    pub unique_states: u32,
    /// Accepted cheaper versions of an already known state.
    pub duplicate_improvements: u32,
    /// Accepted cheaper versions whose previous record was expanded.
    pub reopened_states: u32,
    /// Queue pops superseded by a cheaper record.
    pub stale_pops: u32,
    /// Largest queue length, including stale entries.
    pub peak_queue: u32,
    /// Geometrically legal pushes onto a label-specific dead cell.
    pub pruned_dead_cells: u64,
    /// Legal pushes rejected by the frozen-component (greatest-fixpoint)
    /// deadlock rule.
    pub pruned_deadlocks: u64,
    /// Children rejected because an equal/cheaper version is known or closed.
    pub pruned_duplicates: u64,
    /// Children rejected because no label-compatible goal assignment exists.
    pub pruned_assignment: u64,
    /// Popped nodes or children rejected by an incumbent cost bound.
    pub pruned_bound: u64,
}
/// Length shared by [`SearchStats::FIELDS`] and [`SearchStats::values`], so
/// neither list can grow without the other.
const STAT_COUNT: usize = 10;
impl SearchStats {
    /// The counters' wire names in declaration order. The server's `stats`
    /// object, the WASM diagnostics ABI and the benchmark corpus all pair
    /// these with [`Self::values`].
    pub const FIELDS: [&'static str; STAT_COUNT] = [
        "unique_states",
        "duplicate_improvements",
        "reopened_states",
        "stale_pops",
        "peak_queue",
        "pruned_dead_cells",
        "pruned_deadlocks",
        "pruned_duplicates",
        "pruned_assignment",
        "pruned_bound",
    ];
    /// The counters in [`Self::FIELDS`] order, widened to u64. The
    /// destructure names every field, so a new counter fails to compile
    /// until it is listed here and in `FIELDS`.
    pub fn values(&self) -> [u64; STAT_COUNT] {
        let Self {
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
        } = *self;
        [
            u64::from(unique_states),
            u64::from(duplicate_improvements),
            u64::from(reopened_states),
            u64::from(stale_pops),
            u64::from(peak_queue),
            pruned_dead_cells,
            pruned_deadlocks,
            pruned_duplicates,
            pruned_assignment,
            pruned_bound,
        ]
    }
}

/// Mode dispatch over one engine. `optimal` runs the crate's exact search,
/// the only code that may produce a [`Proof`]; fast and quality run the same
/// engine with a weighted policy and always report unknown optimality. The
/// shared methods are forwarded without exposing the engine, so callers
/// cannot reach or replace an optimal search's engine:
///
/// ```compile_fail
/// # use sokomind_core::Board;
/// # use sokomind_search::{Mode, Search};
/// # let board = Board::parse("ORXS").unwrap();
/// # let mut exact = Search::new(board.clone(), board.initial(), Mode::Optimal, 100, 4).unwrap();
/// # let mut fast = Search::new(board.clone(), board.initial(), Mode::Fast, 100, 4).unwrap();
/// std::mem::swap(&mut *exact, &mut *fast);
/// ```
///
/// A caller interrupts a search only with a [`StopReason`], never with a
/// verdict:
///
/// ```compile_fail
/// # use sokomind_core::Board;
/// # use sokomind_search::{Mode, Search, Status};
/// # let board = Board::parse("ORXS").unwrap();
/// # let mut exact = Search::new(board.clone(), board.initial(), Mode::Optimal, 100, 4).unwrap();
/// exact.stop(Status::Exhausted);
/// ```
pub struct Search(Kind);
enum Kind {
    Exact(ExactSearch),
    Weighted(Engine),
}
impl Search {
    /// Checks `start` against `board` and the limits against
    /// [`MAX_STATES_RANGE`] and [`MEMORY_MIB_RANGE`], then builds the
    /// distance tables and reserves every buffer the search will use: room
    /// for `max_states` records, or for as many as `memory_mib` MiB holds
    /// when that is fewer, in which case filling it ends the search with
    /// [`Status::MemoryLimit`] instead of [`Status::StateLimit`].
    ///
    /// `start` may be any valid position on `board`, such as a live game's;
    /// equal-label boxes may come in any order. No state is expanded until
    /// [`Search::advance`], and a start with no label-compatible goal
    /// assignment gives a search that is already [`Status::Exhausted`].
    ///
    /// # Errors
    ///
    /// - [`SearchError::InvalidState`] when `start` breaks `board`'s geometry
    ///   or occupancy.
    /// - [`SearchError::Limits`] when either limit is outside its range.
    /// - [`SearchError::BudgetTooSmall`] when `memory_mib` cannot hold the
    ///   board's fixed buffers and one state.
    /// - [`SearchError::Allocation`] when the allocator refuses a buffer the
    ///   budget allows.
    pub fn new(
        board: Board,
        start: State,
        mode: Mode,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, SearchError> {
        let kind = match mode {
            Mode::Optimal => Kind::Exact(ExactSearch::new(board, start, max_states, memory_mib)?),
            Mode::Fast => Kind::Weighted(Engine::new(
                board,
                start,
                Policy::FAST,
                max_states,
                memory_mib,
            )?),
            Mode::Quality => Kind::Weighted(Engine::new(
                board,
                start,
                Policy::FAST_THEN_QUALITY_RESTART,
                max_states,
                memory_mib,
            )?),
        };
        Ok(Self(kind))
    }
    /// Live certified lower bound on the optimal move count from the start:
    /// the least f of any record not yet expanded, or whose expansion a limit
    /// cut short, capped by the best route.
    /// `None` for the weighted modes, which claim none, and while no finite
    /// bound exists, as after an exhausted search.
    pub fn lower_bound(&self) -> Option<u32> {
        match &self.0 {
            Kind::Exact(search) => search.lower_bound(),
            Kind::Weighted(_) => None,
        }
    }
    /// Terminal proof; `None` while running or without a sound certificate.
    /// A limit or stop still yields one once a route exists: bounds, or
    /// optimality when they meet. See [`Proof::kind`] for how each wire
    /// format spells it.
    pub fn proof(&self) -> Option<Proof> {
        match &self.0 {
            Kind::Exact(search) => search.proof(),
            Kind::Weighted(_) => None,
        }
    }
}

impl Search {
    fn engine(&self) -> &Engine {
        match &self.0 {
            Kind::Exact(search) => search.engine(),
            Kind::Weighted(search) => search,
        }
    }
    fn engine_mut(&mut self) -> &mut Engine {
        match &mut self.0 {
            Kind::Exact(search) => search.engine_mut(),
            Kind::Weighted(search) => search,
        }
    }

    /// Where the search stands; see [`Status`].
    pub fn status(&self) -> Status {
        self.engine().status()
    }
    /// Move count of the best route found so far, if any. It never grows: a
    /// later route replaces it only when shorter.
    pub fn best_moves(&self) -> Option<u32> {
        self.engine().best_moves()
    }
    /// Records expanded so far, each counted once, including those a Quality
    /// restart discarded. A cheaper route to a known state adds a new record,
    /// so a reopened state counts again.
    pub fn expanded(&self) -> u32 {
        self.engine().expanded()
    }
    /// Pops that re-expanded a partially expanded record (5.3 S2). Always
    /// 0 for Fast and Quality; `expanded` keeps counting distinct records.
    #[cfg(feature = "pea")]
    pub fn reexpansions(&self) -> u32 {
        self.engine().reexpansions()
    }
    /// Records inserted so far, improved versions of known states and any a
    /// Quality restart discarded included.
    pub fn generated(&self) -> u32 {
        self.engine().generated()
    }
    /// Bytes charged against the memory budget: the fixed buffers and the
    /// whole arena, reserved when the search was built.
    pub fn reserved_bytes(&self) -> usize {
        self.engine().reserved_bytes()
    }
    /// Counters from the current search; see [`SearchStats`].
    pub fn stats(&self) -> SearchStats {
        self.engine().stats()
    }
    /// Ends a running search with the status `reason` names; a search that
    /// has already ended keeps its status.
    pub fn stop(&mut self, reason: StopReason) {
        self.engine_mut().stop(reason);
    }
    /// Runs up to `pops` queue pops, returning early when the search ends
    /// and at once when it has already ended. A pop that expands a record
    /// floods the robot's reach once and tries every legal push from there;
    /// stale, solved and dominated pops count without expanding anything.
    /// Slicing the work this way lets a worker yield, report or stop between
    /// calls.
    pub fn advance(&mut self, pops: u32) {
        self.engine_mut().advance(pops);
    }
    /// The best route found so far as `UDLR` letters, rebuilt and replayed
    /// from the start, or `None` without one. It costs O(route) plus one
    /// flood per push, so call it once per improved route. It may be called
    /// between slices of a running search: it borrows mutably only for flood
    /// scratch and leaves the search as it was.
    ///
    /// # Errors
    ///
    /// [`SolutionError::TooLong`] when the route is longer than
    /// [`MAX_ROUTE`] moves. The other variants mean an internal invariant
    /// broke.
    pub fn solution(&mut self) -> Result<Option<String>, SolutionError> {
        self.engine_mut().solution()
    }
}
