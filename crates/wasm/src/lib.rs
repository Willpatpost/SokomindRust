use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Proof, Search, Status};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WasmGame {
    game: Game,
}
#[wasm_bindgen]
impl WasmGame {
    #[wasm_bindgen(constructor)]
    pub fn new(rows: &str) -> Result<WasmGame, JsError> {
        Ok(Self {
            game: Game::new(Board::parse(rows).map_err(|e| JsError::new(&e))?),
        })
    }
    pub fn width(&self) -> u32 {
        self.game.board.width as u32
    }
    pub fn height(&self) -> u32 {
        self.game.board.height as u32
    }
    pub fn tiles(&self) -> Vec<u8> {
        self.game.board.tiles.clone()
    }
    pub fn labels(&self) -> Vec<u8> {
        self.game.board.labels.clone()
    }
    pub fn snapshot(&self) -> Vec<u32> {
        self.game.snapshot()
    }
    pub fn step(&mut self, direction: u32) -> bool {
        self.game.step(direction as usize)
    }
    pub fn undo(&mut self) -> bool {
        self.game.undo()
    }
    pub fn reset(&mut self) {
        self.game.reset();
    }
    pub fn actions(&self) -> String {
        self.game.actions().to_owned()
    }
    pub fn moves(&self) -> u32 {
        self.game.moves()
    }
    pub fn replay(&mut self, route: &str) -> Result<(), JsError> {
        self.game.replay(route).map_err(|e| JsError::new(&e))
    }
    /// Whether box `index` sits on its matching goal. The renderer styles
    /// solved boxes from this, so no game rule lives in JavaScript.
    pub fn on_goal(&self, index: usize) -> bool {
        if index >= self.game.board.labels.len() {
            return false;
        }
        let cell = self.game.state().boxes[index] as usize;
        self.game.board.tiles[cell] == self.game.board.labels[index]
    }
}

#[wasm_bindgen]
pub struct WasmSearch {
    search: Search,
}
#[wasm_bindgen]
impl WasmSearch {
    #[wasm_bindgen(constructor)]
    pub fn new(
        rows: &str,
        actions: &str,
        mode: &str,
        max_states: u32,
        memory_mib: u32,
    ) -> Result<WasmSearch, JsError> {
        let board = Board::parse(rows).map_err(|e| JsError::new(&e))?;
        let mut game = Game::new(board);
        game.replay(actions).map_err(|e| JsError::new(&e))?;
        let mode = Mode::parse(mode).map_err(|e| JsError::new(&e))?;
        let start = game.state();
        let search = Search::new(
            game.board,
            start,
            mode,
            max_states as usize,
            memory_mib as usize,
        )
        .map_err(|e| JsError::new(&e))?;
        Ok(Self { search })
    }
    pub fn advance(&mut self, expansions: u32) {
        self.search.advance(expansions.min(256));
    }
    pub fn stop(&mut self, timeout: bool) {
        self.search.stop(if timeout {
            Status::TimeLimit
        } else {
            Status::Cancelled
        });
    }
    pub fn status(&self) -> String {
        self.search.status().as_str().into()
    }
    /// expanded, generated, accounted reserved bytes, best moves (u32::MAX if
    /// none), proof kind (0 none, 1 bounded, 2 optimal, 3 unsolvable), and the
    /// certified lower bound (u32::MAX if none).
    pub fn metrics(&self) -> Vec<u32> {
        let proof = self.search.proof();
        let kind = match proof {
            Some(Proof::Optimal { .. }) => 2,
            Some(Proof::Bounded { .. }) => 1,
            Some(Proof::Unsolvable) => 3,
            None => 0,
        };
        let lower = match proof {
            Some(Proof::Bounded { lower_bound, .. }) => lower_bound,
            Some(Proof::Optimal { moves }) => moves,
            _ => self.search.lower_bound().unwrap_or(u32::MAX),
        };
        vec![
            self.search.expanded(),
            self.search.generated(),
            self.search.reserved_bytes() as u32,
            self.search.best_moves().unwrap_or(u32::MAX),
            kind,
            lower,
        ]
    }
    pub fn solution(&mut self) -> Result<Option<String>, JsError> {
        self.search.solution().map_err(|e| JsError::new(&e))
    }
}
