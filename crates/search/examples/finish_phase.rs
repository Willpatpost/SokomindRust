//! Finish phase: 4 boxes (A,B,C,D) to matching goals (a,b,c,d)
use sokomind_core::Board;

fn main() {
    // Create a valid 4-box Sokoban puzzle
    // Each box letter must have exactly one matching goal letter
    // Goals use lowercase, boxes use uppercase
    
    // 4 boxes A,B,C,D with goals a,b,c,d
    let rows = vec![
        "OO a b c d   O",   // row 0: goals a,b,c,d (lowercase)
        "OO           O",
        "O  R         O",   // row 2: player R
        "O            O",
        "O  A B C D   O",   // row 4: boxes A,B,C,D (uppercase)
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
