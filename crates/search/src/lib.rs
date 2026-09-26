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
pub use exact::ExactSearch;
pub use proof::Proof;
use sokomind_core::{Board, State, StateError};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    InvalidState(StateError),
    Configuration(String),
}
impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidState(error) => error.fmt(f),
            Self::Configuration(error) => f.write_str(error),
        }
    }
}
impl std::error::Error for SearchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidState(error) => Some(error),
            Self::Configuration(_) => None,
        }
    }
}

/// Counters from the current search. Generated records include immutable
/// improved versions of existing states; unique states count table entries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Distinct canonical box positions AND exact player positions retained.
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
    /// Legal pushes rejected by 2x2 or frozen-component deadlock rules.
    pub pruned_deadlocks: u64,
    /// Children rejected because an equal/cheaper version is known or closed.
    pub pruned_duplicates: u64,
    /// Children rejected because no label-compatible goal assignment exists.
    pub pruned_assignment: u64,
    /// Popped nodes or children rejected by an incumbent cost bound.
    pub pruned_bound: u64,
}

/// Mode dispatch over one engine. `optimal` runs [`ExactSearch`], the only
/// type that may produce a [`Proof`]; fast and quality run the same engine
/// with a weighted policy and always report unknown optimality. The shared
/// methods are forwarded without exposing the engine.
pub struct Search(Kind);
enum Kind {
    Exact(ExactSearch),
    Bounded(Engine),
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
            Mode::Fast => Kind::Bounded(Engine::new(
                board,
                start,
                Policy::FAST,
                max_states,
                memory_mib,
            )?),
            Mode::Quality => Kind::Bounded(Engine::new(
                board,
                start,
                Policy::QUALITY,
                max_states,
                memory_mib,
            )?),
        };
        Ok(Self(kind))
    }
    /// Live certified lower bound on the optimal move count from the start;
    /// the bounded modes claim none.
    pub fn lower_bound(&self) -> Option<u32> {
        match &self.0 {
            Kind::Exact(search) => search.lower_bound(),
            Kind::Bounded(_) => None,
        }
    }
    /// Terminal proof; `None` while running or without a sound certificate.
    pub fn proof(&self) -> Option<Proof> {
        match &self.0 {
            Kind::Exact(search) => search.proof(),
            Kind::Bounded(_) => None,
        }
    }
}

impl Search {
    fn engine(&self) -> &Engine {
        match &self.0 {
            Kind::Exact(search) => search.engine(),
            Kind::Bounded(search) => search,
        }
    }
    fn engine_mut(&mut self) -> &mut Engine {
        match &mut self.0 {
            Kind::Exact(search) => search.engine_mut(),
            Kind::Bounded(search) => search,
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
    pub fn solution(&mut self) -> Result<Option<String>, String> {
        self.engine_mut().solution()
    }
}
