//! Finish phase: 4 boxes (A,B,C,D) to matching goals (a,b,c,d)
use sokomind_core::Board;

fn main() {
    // Create a valid 4-box Sokoban puzzle
    // Each box must have a matching goal of the same letter
    // Player must be on a valid floor cell
    
    // Simple 4-box layout with matching labels
    let rows = vec![
        "OO A B C D   O",   // row 0: goals a,b,c,d at cols 3,5,7,9
        "OO           O",
        "O  R         O",   // row 2: player R at col 3
        "O            O",
        "O  A B C D   O",   // row 4: boxes A,B,C,D at cols 3,5,7,9
        "OOOOOOOOOOOOOO",
    ];

    match Board::parse(&rows.join("\n")) {
        Ok(board) => {
            println!("Puzzle created successfully");
            println!("  Width: {}", board.width());
            println!("  Height: {}", board.height());
            println!("  Labels: {:?}", board.labels());
            println!("  Goals: {:?}", board.goals());
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }
}
