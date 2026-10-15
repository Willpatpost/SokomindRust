//! Throwaway verification for the LP-template route campaign: replay a
//! route file on 'huge' through the product's own rules (`Game::replay`).
//! Not durable machinery; removed after the campaign block.

use sokomind_core::{Board, Game};
use std::fs;

const ROWS: &[&str] = &[
    "OOOOOOOOOOOOOOO",
    "OaSS   S   SSbO",
    "OSCS  OOO  SDSO",
    "OX X  OOO  X XO",
    "O     OOO     O",
    "OOOO   X   OOOO",
    "O      O      O",
    "O G hOOOOOH g O",
    "O      O      O",
    "OOO         OOO",
    "OOO   X X   OOO",
    "OOOOOOOROOOOOOO",
    "O B X X X X A O",
    "O Sc       dS O",
    "OOOOOOOOOOOOOOO",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: replay_route <route-file>")?;
    let route = fs::read_to_string(&path)?;
    let route = route.trim();
    let board = Board::parse(&ROWS.join("\n"))?;
    let mut game = Game::new(board);
    println!("route file {path}: {} moves", route.len());
    game.replay(route)
        .map_err(|e| format!("REPLAY FAILED: {e}"))?;
    println!(
        "replay OK under the product rules: {} moves, {} pushes, solved = {}",
        game.moves(),
        game.pushes(),
        game.solved()
    );
    assert!(game.solved(), "route did not solve the board");
    assert_eq!(game.moves() as usize, route.len());
    Ok(())
}
