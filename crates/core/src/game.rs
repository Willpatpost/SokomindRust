use crate::{ACTIONS, Board, Cell, State, Step};
pub const MAX_ROUTE: usize = 100_000;

/// Why a route was not replayed. Each variant displays the message the
/// server and the web app show for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// The rows did not parse; carries [`Board::parse`]'s message. Only
    /// [`Game::at`] returns it.
    InvalidBoard(String),
    /// The route alone is longer than [`MAX_ROUTE`] moves.
    TooLong,
    /// The byte at `index` is not one of U/D/L/R.
    InvalidAction { index: usize },
    /// The move at `index` was refused: a wall or an immovable box is in
    /// the way, or the puzzle was already solved before it.
    Blocked { index: usize },
    /// The position's moves plus the moves asked for would pass
    /// [`MAX_ROUTE`]; see [`Game::check_extension`].
    PastLimit,
}
impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBoard(error) => f.write_str(error),
            Self::TooLong => write!(f, "Route is too long: the limit is {MAX_ROUTE} moves"),
            Self::InvalidAction { .. } => f.write_str("Routes must use only U/D/L/R"),
            Self::Blocked { index } => write!(f, "Blocked action at index {index}"),
            Self::PastLimit => write!(
                f,
                "Position and route together exceed the {MAX_ROUTE}-move replay limit"
            ),
        }
    }
}
impl std::error::Error for ReplayError {}

/// The player's previous cell and what the step did. A pushed box came from
/// the cell the player now stands on.
struct Undo {
    player: Cell,
    step: Step,
}

/// The fields are private: undo records box indices, so only `step`,
/// `undo`, and `reset` may move boxes, reorder the live state, or change
/// the counters.
pub struct Game {
    board: Board,
    state: State,
    pushes: u32,
    history: Vec<Undo>,
    actions: String,
}

/// The direction index of a U/D/L/R action byte; `None` for any other byte.
pub fn decode_direction(action: u8) -> Option<usize> {
    ACTIONS.iter().position(|&a| a == action)
}

impl Game {
    pub fn new(board: Board) -> Self {
        Self {
            state: board.initial(),
            board,
            pushes: 0,
            history: Vec::new(),
            actions: String::new(),
        }
    }
    pub fn board(&self) -> &Board {
        &self.board
    }
    /// The board and a copy of the live position, for handing a game's
    /// position to a search.
    pub fn into_parts(self) -> (Board, State) {
        (self.board, self.state)
    }
    /// A copy of the live position, in the game's own box order.
    pub fn state(&self) -> State {
        self.state
    }
    pub fn moves(&self) -> u32 {
        self.history.len() as u32
    }
    pub fn pushes(&self) -> u32 {
        self.pushes
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
        let Some(step) = self.board.step(&mut self.state, direction) else {
            return false;
        };
        self.history.push(Undo { player, step });
        self.pushes += u32::from(matches!(step, Step::Push(_)));
        self.actions.push(ACTIONS[direction] as char);
        true
    }
    pub fn undo(&mut self) -> bool {
        let Some(undo) = self.history.pop() else {
            return false;
        };
        if let Step::Push(index) = undo.step {
            self.state.boxes[index] = self.state.player;
            self.pushes -= 1;
        }
        self.state.player = undo.player;
        self.actions.pop();
        true
    }
    pub fn reset(&mut self) {
        self.state = self.board.initial();
        self.history.clear();
        self.actions.clear();
        self.pushes = 0;
    }
    /// The position after strictly replaying `actions` on the board parsed
    /// from `rows`: the one entry point for callers that search from a
    /// played prefix. A full-length unsolved position can only be extended
    /// past [`MAX_ROUTE`], so it is refused here, before a caller spends a
    /// search budget on it.
    pub fn at(rows: &str, actions: &str) -> Result<Self, ReplayError> {
        let mut game = Self::new(Board::parse(rows).map_err(ReplayError::InvalidBoard)?);
        game.replay(actions)?;
        if !game.solved() {
            game.check_extension(1)?;
        }
        Ok(game)
    }
    /// Errs with [`ReplayError::PastLimit`] when `moves` more moves would
    /// take this game's route past [`MAX_ROUTE`].
    pub fn check_extension(&self, moves: u32) -> Result<(), ReplayError> {
        // `step` never lets the history outgrow the limit, so this cannot underflow.
        if moves as usize > MAX_ROUTE - self.history.len() {
            return Err(ReplayError::PastLimit);
        }
        Ok(())
    }
    /// Atomic strict replay: malformed or blocked routes leave the game unchanged.
    pub fn replay(&mut self, route: &str) -> Result<(), ReplayError> {
        if route.len() > MAX_ROUTE {
            return Err(ReplayError::TooLong);
        }
        let mut next = Game::new(self.board.clone());
        for (index, action) in route.bytes().enumerate() {
            let direction = decode_direction(action).ok_or(ReplayError::InvalidAction { index })?;
            if !next.step(direction) {
                return Err(ReplayError::Blocked { index });
            }
        }
        *self = next;
        Ok(())
    }
}
