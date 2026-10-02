//! Browser bindings for the web app. `scripts/build-wasm.mjs` compiles this
//! crate and runs wasm-bindgen into `web/wasm/`.
//!
//! [`WasmGame`] plays one board on the main thread, and [`WasmSearch`] runs
//! one search in a worker, a slice of queue pops at a time. Only strings,
//! numbers and flat number arrays cross the boundary. `web/src/transport.ts`
//! decodes and checks the snapshot and metrics arrays, `web/src/main.ts`
//! hands the tiles and labels to `web/src/board.ts`, and
//! `scripts/parity.mjs` compares the diagnostics with the native stats.
//! These layouts, documented on each method, are a wire format: change
//! every reader together.
use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Proof, Search, Status, StopReason};
use wasm_bindgen::prelude::*;

/// A live game for the page, wrapping [`Game`] so every rule stays in Rust.
#[wasm_bindgen]
pub struct WasmGame {
    game: Game,
}
#[wasm_bindgen]
impl WasmGame {
    /// Parses `rows` as [`Board::parse`] does and starts a game at the
    /// board's start position. Throws the parse error's message when the
    /// text is not a board.
    #[wasm_bindgen(constructor)]
    pub fn new(rows: &str) -> Result<WasmGame, JsError> {
        Ok(Self {
            game: Game::new(Board::parse(rows)?),
        })
    }
    /// Columns: the longest row's length in bytes.
    pub fn width(&self) -> u32 {
        self.game.board().width() as u32
    }
    /// Rows.
    pub fn height(&self) -> u32 {
        self.game.board().height() as u32
    }
    /// Tile ABI: one byte per cell in row-major order, so a cell is
    /// `row * width + column`. 0 is floor, 255 is a wall or row padding,
    /// and any other value is the ASCII code of a goal's label (88, `X`,
    /// for an `S` goal). The robot and the boxes are not tiles; they come
    /// from `snapshot`. Fixed for the game's lifetime.
    pub fn tiles(&self) -> Vec<u8> {
        self.game.board().tiles().to_vec()
    }
    /// Each box's label as an ASCII code, in the order `snapshot` lists box
    /// cells. Fixed for the game's lifetime.
    pub fn labels(&self) -> Vec<u8> {
        self.game.board().labels().to_vec()
    }
    /// Snapshot ABI: robot cell, moves, pushes, solved (0 or 1), then each
    /// box's cell in `labels` order, so `4 + labels().length` values.
    /// Labels are static, so they are not repeated here.
    pub fn snapshot(&self) -> Vec<u32> {
        let game = &self.game;
        let state = game.state();
        let boxes = &state.boxes[..game.board().labels().len()];
        let mut out = Vec::with_capacity(4 + boxes.len());
        out.extend([
            u32::from(state.player),
            game.moves(),
            game.pushes(),
            u32::from(game.solved()),
        ]);
        out.extend(boxes.iter().map(|&cell| u32::from(cell)));
        out
    }
    /// Plays one move, `direction` 0..4 meaning U, D, L, R, as
    /// [`Game::step`] does. False, with nothing changed, when `direction` is
    /// out of range, the move is blocked, the puzzle is already solved or
    /// the route is at its move limit.
    pub fn step(&mut self, direction: u32) -> bool {
        self.game.step(direction as usize)
    }
    /// Takes back the last move; false when no move is left to undo.
    pub fn undo(&mut self) -> bool {
        self.game.undo()
    }
    /// Returns to the start position and forgets every move.
    pub fn reset(&mut self) {
        self.game.reset();
    }
    /// The route played so far, one U/D/L/R letter per move.
    pub fn actions(&self) -> String {
        self.game.actions().to_owned()
    }
    /// Moves played so far.
    pub fn moves(&self) -> u32 {
        self.game.moves()
    }
    /// Replaces the game with a strict replay of `route` from the start
    /// position, as [`Game::replay`] does. Throws that error's message and
    /// leaves the game unchanged when the route is refused.
    pub fn replay(&mut self, route: &str) -> Result<(), JsError> {
        Ok(self.game.replay(route)?)
    }
    /// Whether box `index` sits on its matching goal. The renderer styles
    /// solved boxes from this, so no game rule lives in JavaScript. False
    /// for an index past the last box.
    pub fn on_goal(&self, index: usize) -> bool {
        let board = self.game.board();
        index < board.labels().len() && board.on_goal(index, self.game.state().boxes[index])
    }
}

/// One search for the worker, wrapping [`Search`]. The worker advances it in
/// short slices, reads `metrics` between them, and stops it itself on
/// cancellation or at its time budget.
#[wasm_bindgen]
pub struct WasmSearch {
    search: Search,
}
#[wasm_bindgen]
impl WasmSearch {
    /// Searches from the position reached by strictly replaying `actions`
    /// on the board parsed from `rows`, as [`Game::at`] does. `mode` is
    /// `fast`, `quality` or `optimal`; `max_states` and `memory_mib` must
    /// lie in [`MAX_STATES_RANGE`](sokomind_search::MAX_STATES_RANGE) and
    /// [`MEMORY_MIB_RANGE`](sokomind_search::MEMORY_MIB_RANGE). Throws the
    /// board, replay, mode or search error's message.
    #[wasm_bindgen(constructor)]
    pub fn new(
        rows: &str,
        actions: &str,
        mode: &str,
        max_states: u32,
        memory_mib: u32,
    ) -> Result<WasmSearch, JsError> {
        let game = Game::at(rows, actions)?;
        let mode = Mode::parse(mode)?;
        let (board, start) = game.into_parts();
        let search = Search::new(board, start, mode, max_states as usize, memory_mib as usize)?;
        Ok(Self { search })
    }
    /// Runs up to `pops` queue pops; true while the search is still running.
    pub fn advance(&mut self, pops: u32) -> bool {
        self.search.advance(pops);
        self.search.status() == Status::Running
    }
    /// Ends a running search because its time budget ran out; `status`
    /// then reads `time_limit`. No effect once the search has ended, and
    /// the best route found stays readable.
    pub fn stop_time_limit(&mut self) {
        self.search.stop(StopReason::TimeLimit);
    }
    /// Ends a running search because the user cancelled it; `status` then
    /// reads `cancelled`. No effect once the search has ended, and the best
    /// route found stays readable.
    pub fn cancel(&mut self) {
        self.search.stop(StopReason::Cancelled);
    }
    /// The search's state by its wire name, as [`Status::as_str`] spells
    /// it: `running` until it ends, then the reason it ended.
    pub fn status(&self) -> String {
        self.search.status().as_str().into()
    }
    /// Metrics ABI, six values:
    ///
    /// - `[0]` states expanded.
    /// - `[1]` states generated.
    /// - `[2]` the memory ceiling, [`Search::reserved_bytes`]: the most the
    ///   search's buffers can reach, not the bytes allocated so far.
    /// - `[3]` moves in the best route found, or `u32::MAX` before the first.
    ///   The count saturates at `u32::MAX - 1` (see [`Search::best_moves`]),
    ///   so `u32::MAX` always means no route.
    /// - `[4]` proof kind: 0 none, 1 bounded, 2 optimal, 3 unsolvable, as the
    ///   table on [`Proof::kind`] lists.
    /// - `[5]` the live certified lower bound on the optimal moves, or
    ///   `u32::MAX` while there is none.
    ///
    /// A bounded proof spans `[5]` to `[3]`, and an optimal one proves
    /// `[3]` optimal. `decodeMetricTuple` in `web/src/transport.ts` reads
    /// the first six values and ignores any appended later.
    pub fn metrics(&self) -> Vec<u32> {
        let kind = match self.search.proof() {
            None => 0,
            Some(Proof::Bounded { .. }) => 1,
            Some(Proof::Optimal { .. }) => 2,
            Some(Proof::Unsolvable) => 3,
        };
        vec![
            self.search.expanded(),
            self.search.generated(),
            self.search.reserved_bytes() as u32,
            self.search.best_moves().unwrap_or(u32::MAX),
            kind,
            self.search.lower_bound().unwrap_or(u32::MAX),
        ]
    }
    /// The best route found, starting from the position after the
    /// constructor's `actions`, or undefined before the first. Rust rebuilds
    /// the route and replays it on the board before returning it, and
    /// throws when the route is longer than
    /// [`MAX_ROUTE`](sokomind_core::MAX_ROUTE) moves or the replay check
    /// fails. The web worker calls it only for a best that fits in
    /// `MAX_ROUTE` after the constructor's `actions`, so the worker never
    /// meets the first error; for a finished search whose best does not fit,
    /// the worker reports `ReplayError::PastLimit`'s text instead.
    pub fn solution(&mut self) -> Result<Option<String>, JsError> {
        Ok(self.search.solution()?)
    }

    /// Diagnostic ABI: the counters in
    /// [`SearchStats::FIELDS`](sokomind_search::SearchStats::FIELDS) order,
    /// as `scripts/parity.mjs` reads them (unique states, duplicate
    /// improvements, reopenings, stale pops, peak queue, then
    /// dead-cell/deadlock/duplicate/assignment/bound prunes, then
    /// sealed-corral prunes). f64 exactly represents all counters under the
    /// node cap. Kept separate from the small six-value progress ABI.
    pub fn diagnostics(&self) -> Vec<f64> {
        self.search
            .stats()
            .values()
            .into_iter()
            .map(|value| value as f64)
            .collect()
    }
}
