//! Incremental, platform-independent push A*. All hot-path storage is reserved once.
mod arena;
mod bounded;
mod certificate;
mod deadlock;
mod exact;
mod heuristic;
mod reach;

pub use bounded::BoundedSearch;
pub use certificate::Proof;
pub use deadlock::Deadlock;
pub use exact::ExactSearch;
pub use heuristic::Heuristic;
use sokomind_core::{Board, State};

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

/// Mode-dispatching facade over the two engines. `optimal` runs the exact
/// kernel, the only engine allowed to produce a [`Proof`]; the bounded engine
/// always reports unknown optimality.
pub enum Search {
    Exact(ExactSearch),
    Bounded(BoundedSearch),
}
impl Search {
    pub fn new(
        board: Board,
        start: State,
        mode: Mode,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, String> {
        match mode {
            Mode::Optimal => Ok(Self::Exact(ExactSearch::new(
                board,
                start,
                max_states,
                memory_mib,
            )?)),
            Mode::Fast => Ok(Self::Bounded(BoundedSearch::new(
                board,
                start,
                true,
                max_states,
                memory_mib,
            )?)),
            Mode::Quality => Ok(Self::Bounded(BoundedSearch::new(
                board,
                start,
                false,
                max_states,
                memory_mib,
            )?)),
        }
    }
    pub fn status(&self) -> Status {
        match self {
            Self::Exact(search) => search.status(),
            Self::Bounded(search) => search.status(),
        }
    }
    pub fn stop(&mut self, reason: Status) {
        match self {
            Self::Exact(search) => search.stop(reason),
            Self::Bounded(search) => search.stop(reason),
        }
    }
    /// Work is sliced by expansions so a worker can yield, report, or cancel.
    pub fn advance(&mut self, expansions: u32) {
        match self {
            Self::Exact(search) => search.advance(expansions),
            Self::Bounded(search) => search.advance(expansions),
        }
    }
    pub fn best_moves(&self) -> Option<u32> {
        match self {
            Self::Exact(search) => search.best_moves(),
            Self::Bounded(search) => search.best_moves(),
        }
    }
    /// Live certified lower bound on the optimal move count from the start;
    /// the bounded engine claims none.
    pub fn lower_bound(&self) -> Option<u32> {
        match self {
            Self::Exact(search) => search.lower_bound(),
            Self::Bounded(_) => None,
        }
    }
    /// Terminal proof; `None` while running or without a sound certificate.
    pub fn proof(&self) -> Option<Proof> {
        match self {
            Self::Exact(search) => search.proof(),
            Self::Bounded(_) => None,
        }
    }
    pub fn expanded(&self) -> u32 {
        match self {
            Self::Exact(search) => search.expanded(),
            Self::Bounded(search) => search.expanded(),
        }
    }
    pub fn generated(&self) -> u32 {
        match self {
            Self::Exact(search) => search.generated(),
            Self::Bounded(search) => search.generated(),
        }
    }
    pub fn reserved_bytes(&self) -> usize {
        match self {
            Self::Exact(search) => search.reserved_bytes(),
            Self::Bounded(search) => search.reserved_bytes(),
        }
    }
    /// Reconstruct walks only once per reported incumbent, then independently replay.
    pub fn solution(&mut self) -> Result<Option<String>, String> {
        match self {
            Self::Exact(search) => search.solution(),
            Self::Bounded(search) => search.solution(),
        }
    }
}
