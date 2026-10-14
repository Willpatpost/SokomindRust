//! Analyze the Grand Hall route to find the finish phase state
//! 
//! The route is 436 moves with 222 pushes
//! We need to find where the finish phase starts (after room route)

use sokomind_core::{Board, Game};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rows = vec![
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
        "OOOOOOOOOOOOOOO"
    ];
    
    let board = Board::parse(&rows.join("\n"))?;
    println!("Grand Hall: {}x{}", board.width(), board.height());
    
    // Load the route from the catalog output
    let route_str = "DDLLURDRUUULURRRRLLLDDDDRRULRDRRULLRRDRRULLDLLLLUUULULLDRRURRLDDDLLDLLURRRDRUUURUURRDDRUULDDLULLLLDLUUUUUUURULRDDDRRRDRRDDRDLLLLLRDDDLLLLDLLURRRRRDRUDRRULLUULLURRRRDLULDDDRRDRRULLLDLLLLLRRRRUUURULLLDLUUUUUULLLURDRRUURULLLDRLDLDRRUUDRDDDDDDLDRRRURDDDRDLLLLRURDRUUURRRUUUUUURRRULDLLUULURRRDDRDLLULDDDDDDRDLLLULDDDLDRRRRLLLUUUULLDLUUUUUUDDLLDLDRRURDLDRRRULDLURRDRRRLURDRUUUUUUURDRULLDDDDRRDLDLDDRUULURLUUUULURLURDDDDLLLLDLUUUULURRLDDLLDLUU";
    
    let route: Vec<char> = route_str.chars().collect();
    println!("Route: {} moves", route.len());
    
    // Simulate the route to find the state after the room phase
    // The room phase solves the bottom room (6 boxes to goals)
    // After that, we have 4 boxes left (11,12,C,D) to finish
    
    let mut game = Game::new(board.clone());
    game.replay(&route)?;
    
    if game.solved() {
        println!("Route solves the puzzle!");
        println!("Moves: {}, Pushes: {}", game.moves(), game.pushes());
        
        // Show final state
        println!("\nFinal board state:");
        for y in 0..board.height() {
            let mut row_str = String::new();
            for x in 0..board.width() {
                let cell = board.cell(x, y);
                let pos = y * board.width() + x;
                
                let has_box = game.boxes().iter().position(|p| *p == pos).map(|i| {
                    board.boxes()[i]
                });
                let has_goal = board.goals().iter().find(|g| g.0 == pos).map(|g| g.1);
                
                match (has_box, has_goal) {
                    (Some(b), Some(g)) if b == g => {
                        if pos == game.player() { print!("X"); } else { print!("x"); }
                    }
                    (Some(b), _) => {
                        if pos == game.player() { print!("O"); } else { print!("{}", b as char); }
                    }
                    (_, Some(g)) => {
                        if pos == game.player() { print!("+"); } else { print!("{}", g as char); }
                    }
                    _ => {
                        if pos == game.player() { print!("R"); } else { print!("{}", cell as char); }
                    }
                }
            }
            println!();
        }
    } else {
        println!("Route does NOT solve the puzzle");
    }
    
    Ok(())
}
