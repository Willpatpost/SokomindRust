//! Pure rules and compact board geometry. No platform or serialization dependencies.
//!
//! [`Board::parse`] turns puzzle text into an immutable [`Board`], and a
//! [`State`] is one position on it. [`Board::step`] is the primitive move:
//! [`Game`] plays it with undo and strict route replay for the server and
//! the web app, and the search replays every route it returns through it.
//! Directions are indices into [`ACTIONS`] everywhere: a route spells each
//! one as its letter there (see [`decode_direction`]), and
//! [`Board::neighbors`] and the WASM ABI index by them.
mod board;
mod game;
pub use board::{
    Board, Cell, MAX_BOXES, MAX_CELLS, NONE, ParseError, State, StateError, Step, WALL,
};
pub use game::{Game, MAX_ROUTE, ReplayError, decode_direction};

/// The route letter of each direction: up, down, left and right are 0..4.
pub const ACTIONS: &[u8; 4] = b"UDLR";
/// The reverse of each direction: `OPPOSITE[d]` points the other way from
/// `d`, as down does from up.
pub const OPPOSITE: [usize; 4] = [1, 0, 3, 2];
