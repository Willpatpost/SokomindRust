use crate::{
    SearchError, Status,
    engine::{Engine, Policy},
    proof::Proof,
};
use sokomind_core::{Board, State};

/// Move-optimal push A* over an admissible heuristic. A cheaper path to a
/// known state is re-inserted as a new node, even if the old one was already
/// expanded, so soundness never depends on consistency; with a consistent
/// heuristic, closed nodes are never re-expanded. The only engine that may
/// produce a [`Proof`]. Callers reach it only as [`crate::Search`] in
/// [`crate::Mode::Optimal`], which forwards the shared methods to its engine
/// and never hands the engine out, so callers cannot replace it.
pub(crate) struct ExactSearch(Engine);

impl ExactSearch {
    pub(crate) fn new(
        board: Board,
        start: State,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, SearchError> {
        Engine::new(board, start, Policy::EXACT, max_states, memory_mib).map(Self)
    }
    pub(crate) fn engine(&self) -> &Engine {
        &self.0
    }
    pub(crate) fn engine_mut(&mut self) -> &mut Engine {
        &mut self.0
    }
    /// Live certified lower bound on the optimal move count from the start:
    /// the frontier, capped by the incumbent.
    pub(crate) fn lower_bound(&self) -> Option<u32> {
        let frontier = self.0.frontier();
        match self.0.best_moves() {
            Some(best) if frontier >= best as u64 => Some(best),
            _ => (frontier < u32::MAX as u64).then_some(frontier as u32),
        }
    }
    /// Terminal proof; `None` while running or without an incumbent. A limit
    /// or cancellation keeps every bound computed so far.
    pub(crate) fn proof(&self) -> Option<Proof> {
        match self.0.status() {
            Status::Running => None,
            Status::Exhausted => Some(Proof::Unsolvable),
            _ => {
                let upper = self.0.best_moves()?;
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
}
