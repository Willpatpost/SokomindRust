use crate::{
    Status,
    engine::{Engine, Policy},
    proof::Proof,
};
use sokomind_core::{Board, State};
use std::ops::{Deref, DerefMut};

/// Move-optimal push A* over an admissible heuristic. A cheaper path to a
/// known state is re-inserted as a new node, even if the old one was already
/// expanded, so soundness never depends on consistency; with a consistent
/// heuristic, closed nodes are never re-expanded. The only engine that may
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
    /// Live certified lower bound on the optimal move count from the start:
    /// the frontier, capped by the incumbent.
    pub fn lower_bound(&self) -> Option<u32> {
        let frontier = self.frontier();
        match self.best_moves() {
            Some(best) if frontier >= best as u64 => Some(best),
            _ => (frontier < u32::MAX as u64).then_some(frontier as u32),
        }
    }
    /// Terminal proof; `None` while running or without an incumbent. A limit
    /// or cancellation keeps every bound computed so far.
    pub fn proof(&self) -> Option<Proof> {
        match self.status() {
            Status::Running => None,
            Status::Exhausted => Some(Proof::Unsolvable),
            _ => {
                let upper = self.best_moves()?;
                let lower = self.lower_bound()?;
                Some(if lower >= upper {
                    Proof::Optimal { moves: upper }
                } else {
                    Proof::Bounded {
                        lower_bound: lower,
                        upper_bound: upper,
                    }
                })
            }
        }
    }
    /// Minimum f over everything not yet expanded, including the unpushed
    /// successors of an interrupted expansion.
    fn frontier(&self) -> u64 {
        self.arena
            .min_f()
            .unwrap_or(u64::MAX)
            .min(self.interrupted_g.map_or(u64::MAX, |g| g as u64 + 1))
    }
}
