use crate::{
    Status,
    certificate::Proof,
    engine::{Engine, Policy},
};
use sokomind_core::{Board, State};
use std::ops::{Deref, DerefMut};

/// Admissible move-optimal A* with reopenings. The only engine that may
/// produce a [`Proof`]; the shared search methods come through `Deref`.
pub struct ExactSearch(Engine);

impl Deref for ExactSearch {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        &self.0
    }
}
impl DerefMut for ExactSearch {
    fn deref_mut(&mut self) -> &mut Engine {
        &mut self.0
    }
}

impl ExactSearch {
    pub fn new(
        board: Board,
        start: State,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, String> {
        Engine::new(board, start, Policy::EXACT, max_states, memory_mib).map(Self)
    }
    /// Live certified lower bound on the optimal move count from the start.
    pub fn lower_bound(&self) -> Option<u32> {
        if let Some(proof) = self.proof() {
            return match proof {
                Proof::Bounded { lower_bound, .. } | Proof::Optimal { moves: lower_bound } => {
                    Some(lower_bound)
                }
                Proof::Unsolvable => None,
            };
        }
        let frontier = self.frontier();
        match self.best_moves() {
            Some(best) if frontier >= best as u64 => Some(best),
            _ => (frontier < u32::MAX as u64).then_some(frontier as u32),
        }
    }
    /// Terminal proof; `None` while running or without a sound certificate.
    pub fn proof(&self) -> Option<Proof> {
        match self.status() {
            Status::Running => None,
            Status::Solved => self.best_moves().map(|moves| Proof::Optimal { moves }),
            Status::Exhausted => Some(Proof::Unsolvable),
            _ => self.bounded_proof(),
        }
    }
    /// A limit or cancellation keeps every bound computed so far.
    fn bounded_proof(&self) -> Option<Proof> {
        let upper_bound = self.best_moves()?;
        let frontier = self.frontier();
        if frontier >= upper_bound as u64 {
            return Some(Proof::Optimal { moves: upper_bound });
        }
        Some(Proof::Bounded {
            lower_bound: frontier as u32,
            upper_bound,
        })
    }
    /// Minimum f over everything not yet expanded, including the unpushed
    /// successors of an interrupted expansion.
    fn frontier(&self) -> u64 {
        self.arena
            .peek_priority()
            .unwrap_or(u64::MAX)
            .min(self.interrupted_g.map_or(u64::MAX, |g| g as u64 + 1))
    }
}
