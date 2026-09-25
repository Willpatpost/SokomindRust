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
pub use heuristic::Heuristic;
pub use proof::Proof;
use sokomind_core::{Board, State};
use std::ops::{Deref, DerefMut};

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

/// Mode dispatch over one engine. `optimal` runs [`ExactSearch`], the only
/// type that may produce a [`Proof`]; fast and quality run the same engine
/// with a weighted policy and always report unknown optimality. The shared
/// search methods come through `Deref`.
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
    ) -> Result<Self, String> {
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
impl Deref for Search {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        match &self.0 {
            Kind::Exact(search) => search,
            Kind::Bounded(search) => search,
        }
    }
}
impl DerefMut for Search {
    fn deref_mut(&mut self) -> &mut Engine {
        match &mut self.0 {
            Kind::Exact(search) => search,
            Kind::Bounded(search) => search,
        }
    }
}
