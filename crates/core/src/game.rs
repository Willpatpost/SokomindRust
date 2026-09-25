use crate::{ACTIONS, Board, Cell, MAX_BOXES, State};
pub const MAX_ROUTE: usize = 100_000;

/// The player's previous cell and the pushed box index, or MAX_BOXES for a
/// walk. A pushed box came from the cell the player now stands on.
struct Undo {
    player: Cell,
    box_index: usize,
}

pub struct Game {
    pub board: Board,
    /// Private: undo records box indices, so only `step`, `undo`, and
    /// `reset` may move boxes or reorder the live state.
    state: State,
    pub pushes: u32,
    history: Vec<Undo>,
    actions: String,
}

pub fn decode_direction(action: u8) -> Result<usize, String> {
    ACTIONS
        .iter()
        .position(|&a| a == action)
        .ok_or_else(|| "Routes must use only U/D/L/R".into())
}

impl Game {
    pub fn new(board: Board) -> Self {
        Self {
            state: board.initial,
            board,
            pushes: 0,
            history: Vec::new(),
            actions: String::new(),
        }
    }
    /// A copy of the live position, in the game's own box order.
    pub fn state(&self) -> State {
        self.state
    }
    pub fn moves(&self) -> u32 {
        self.history.len() as u32
    }
    pub fn actions(&self) -> &str {
        &self.actions
    }
    pub fn solved(&self) -> bool {
        self.board.solved(&self.state)
    }
    pub fn step(&mut self, direction: usize) -> bool {
        if self.solved() || self.history.len() >= MAX_ROUTE {
            return false;
        }
        let player = self.state.player;
        let Some(i) = self.board.step(&mut self.state, direction) else {
            return false;
        };
        self.history.push(Undo {
            player,
            box_index: i,
        });
        self.pushes += u32::from(i < MAX_BOXES);
        self.actions.push(ACTIONS[direction] as char);
        true
    }
    pub fn undo(&mut self) -> bool {
        let Some(undo) = self.history.pop() else {
            return false;
        };
        if undo.box_index < MAX_BOXES {
            self.state.boxes[undo.box_index] = self.state.player;
            self.pushes -= 1;
        }
        self.state.player = undo.player;
        self.actions.pop();
        true
    }
    pub fn reset(&mut self) {
        self.state = self.board.initial;
        self.history.clear();
        self.actions.clear();
        self.pushes = 0;
    }
    /// Atomic strict replay: malformed or blocked routes leave the game unchanged.
    pub fn replay(&mut self, route: &str) -> Result<(), String> {
        if route.len() > MAX_ROUTE {
            return Err(format!("Route is too long: the limit is {MAX_ROUTE} moves"));
        }
        let mut next = Game::new(self.board.clone());
        for (i, action) in route.bytes().enumerate() {
            if !next.step(decode_direction(action)?) {
                return Err(format!("Blocked action at index {i}"));
            }
        }
        *self = next;
        Ok(())
    }
}
