use crate::{
    SearchError, SearchStats, Status, StopReason,
    engine::{Engine, Policy},
    proof::Proof,
};
use sokomind_core::{Board, State};

/// Move-optimal push A* over an admissible heuristic. A cheaper path to a
/// known state is re-inserted as a new node, even if the old one was already
/// expanded, so soundness never depends on consistency; with a consistent
/// heuristic, closed nodes are never re-expanded. The only engine that may
/// produce a [`Proof`]; callers cannot replace its engine.
///
/// ```compile_fail
/// # use sokomind_core::Board;
/// # use sokomind_search::{ExactSearch, Search, Mode};
/// # let board = Board::parse("ORXS").unwrap();
/// # let mut exact = ExactSearch::new(board.clone(), board.initial(), 100, 4).unwrap();
/// # let mut fast = Search::new(board.clone(), board.initial(), Mode::Fast, 100, 4).unwrap();
/// std::mem::swap(&mut *exact, &mut *fast);
/// ```
///
/// ```compile_fail
/// # use sokomind_core::Board;
/// # use sokomind_search::{ExactSearch, Status};
/// # let board = Board::parse("ORXS").unwrap();
/// # let mut exact = ExactSearch::new(board.clone(), board.initial(), 100, 4).unwrap();
/// exact.stop(Status::Exhausted);
/// ```
pub struct ExactSearch(Engine);

impl ExactSearch {
    pub(crate) fn engine(&self) -> &Engine {
        &self.0
    }
    pub(crate) fn engine_mut(&mut self) -> &mut Engine {
        &mut self.0
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

    pub fn new(
        board: Board,
        start: State,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, SearchError> {
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
        self.0
            .arena
            .min_f()
            .unwrap_or(u64::MAX)
            .min(self.0.interrupted_g.map_or(u64::MAX, |g| g as u64 + 1))
    }
}
