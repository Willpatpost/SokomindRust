//! Incremental, platform-independent push A*. The arena, queue, table and
//! flood buffers are reserved once.
mod arena;
mod deadlock;
mod engine;
mod exact;
mod heuristic;
mod proof;
mod reach;

use engine::{Engine, Policy};
use exact::ExactSearch;
pub use proof::Proof;
use sokomind_core::{Board, MAX_ROUTE, State, StateError};
use std::ops::RangeInclusive;

/// Largest per-search state limit accepted anywhere. At 64 MiB the memory
/// budget binds first on boards with 14 or more boxes.
pub const MAX_STATES: usize = 1_000_000;
/// The `max_states` values [`Search::new`] accepts. Callers that validate
/// limits themselves check against this range, never a copy of it.
pub const MAX_STATES_RANGE: RangeInclusive<usize> = 1..=MAX_STATES;
/// The `memory_mib` budgets [`Search::new`] accepts. A caller may cap
/// requests lower but never below this range's start.
pub const MEMORY_MIB_RANGE: RangeInclusive<usize> = 4..=256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Fast,
    Quality,
    Optimal,
}
impl Mode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "fast" => Ok(Self::Fast),
            "quality" => Ok(Self::Quality),
            "optimal" => Ok(Self::Optimal),
            _ => Err("Mode must be fast, quality, or optimal".into()),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Solved,
    Exhausted,
    StateLimit,
    MemoryLimit,
    TimeLimit,
    Cancelled,
}
impl Status {
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
    Cancelled,
    TimeLimit,
}

/// Search construction failed before any caller-supplied state was indexed.
/// `InvalidState`, `Limits` and `BudgetTooSmall` are caller errors;
/// `Allocation` is a resource failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
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
    /// Live certified lower bound on the optimal move count from the start;
    /// the weighted modes claim none.
    pub fn lower_bound(&self) -> Option<u32> {
        match &self.0 {
            Kind::Exact(search) => search.lower_bound(),
            Kind::Weighted(_) => None,
        }
    }
    /// Terminal proof; `None` while running or without a sound certificate.
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

    pub fn status(&self) -> Status {
        self.engine().status()
    }
    pub fn best_moves(&self) -> Option<u32> {
        self.engine().best_moves()
    }
    pub fn expanded(&self) -> u32 {
        self.engine().expanded()
    }
    pub fn generated(&self) -> u32 {
        self.engine().generated()
    }
    pub fn reserved_bytes(&self) -> usize {
        self.engine().reserved_bytes()
    }
    pub fn stats(&self) -> SearchStats {
        self.engine().stats()
    }
    pub fn stop(&mut self, reason: StopReason) {
        self.engine_mut().stop(reason);
    }
    pub fn advance(&mut self, pops: u32) {
        self.engine_mut().advance(pops);
    }
    pub fn solution(&mut self) -> Result<Option<String>, SolutionError> {
        self.engine_mut().solution()
    }
}
