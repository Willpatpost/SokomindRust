//! Finish phase: 4 boxes (A,B,C,D) to matching goals (a,b,c,d)
use sokomind_core::Board;

fn main() {
    // Create a minimal valid 4-box Sokoban puzzle
    // Each box must have a matching goal of the same letter
    
    let rows = vec![
        "OO A A A A   O",   // row 0: goals a,a,a,a (4 a goals)
        "OO           O",
        "O     O      O",
        "O    O       O",
        "O  O         O",
        "O      R     O",   // row 5: player R
        "O  A A A A   O",   // row 6: boxes A,A,A,A
        "O            O",
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
            println!("Error: {:?}", e);
        }
    }
}
