//! Pure rules and compact board geometry. No platform or serialization dependencies.
mod board;
mod game;
pub use board::{Board, Cell, MAX_BOXES, NONE, State, WALL};
pub use game::{Game, MAX_ROUTE, decode_direction};

pub const ACTIONS: &[u8; 4] = b"UDLR";
pub const OPPOSITE: [usize; 4] = [1, 0, 3, 2];
