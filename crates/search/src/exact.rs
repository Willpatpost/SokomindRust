use crate::{
    SearchError, Status,
    arena::G_SAT,
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
    /// Terminal proof; `None` while running or without a sound certificate
    /// (see [`certify`]), and an exhausted search with no route proves
    /// `Unsolvable`. A limit or cancellation keeps every bound computed so
    /// far.
    pub(crate) fn proof(&self) -> Option<Proof> {
        certify(self.0.status(), self.0.best_moves(), self.lower_bound())
    }
}

/// The proof a search ending in `status` certifies from its incumbent's move
/// count `best` and its lower bound `lower`.
fn certify(status: Status, best: Option<u32>, lower: Option<u32>) -> Option<Proof> {
    match status {
        Status::Running => None,
        // The engine ends Exhausted only while it has no incumbent. A route
        // would refute Unsolvable, so one falls through to the bounds below.
        Status::Exhausted if best.is_none() => Some(Proof::Unsolvable),
        _ => {
            // A saturated g stands for any count at or above it, so it
            // bounds nothing from above.
            let upper = best.filter(|&best| best < G_SAT)?;
            let lower = lower?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certify_proves_only_from_an_exact_incumbent() {
        let ends = [
            Status::Solved,
            Status::StateLimit,
            Status::MemoryLimit,
            Status::Cancelled,
            Status::TimeLimit,
        ];
        for status in ends {
            for lower in [None, Some(1), Some(G_SAT - 1), Some(G_SAT)] {
                assert_eq!(certify(status, Some(G_SAT), lower), None, "{status:?}");
            }
            assert_eq!(certify(status, None, Some(1)), None, "{status:?}");
            // One below saturation is an exact count.
            let last = G_SAT - 1;
            assert_eq!(
                certify(status, Some(last), Some(last)),
                Some(Proof::Optimal { moves: last })
            );
            assert_eq!(
                certify(status, Some(last), Some(1)),
                Some(Proof::Bounded {
                    lower_bound: 1,
                    upper_bound: last,
                })
            );
            assert_eq!(certify(status, Some(last), None), None, "{status:?}");
        }
        assert_eq!(certify(Status::Running, Some(1), Some(1)), None);
        assert_eq!(
            certify(Status::Exhausted, None, None),
            Some(Proof::Unsolvable)
        );
        // An exhaustion that kept a route never proves Unsolvable.
        assert_eq!(
            certify(Status::Exhausted, Some(3), Some(3)),
            Some(Proof::Optimal { moves: 3 })
        );
        assert_eq!(certify(Status::Exhausted, Some(G_SAT), Some(1)), None);
    }
}
