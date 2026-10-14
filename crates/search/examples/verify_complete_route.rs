//! Verify complete Grand Hall route by combining room + finish phases
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
    
    // Read the room route (drain)
    let room_route = fs::read_to_string("/home/coder/.qwen/tmp/route-study/room-route.txt")?;
    let room_route = room_route.trim();
    println!("Room route: {} moves", room_route.len());
    
    // Read the finish route
    let finish_route = fs::read_to_string("/home/coder/.qwen/tmp/route-study/finish_route.txt")?;
    let finish_route = finish_route.trim();
    println!("Finish route: {} moves", finish_route.len());
    
    // First, verify the room route solves the bottom room
    let mut game = Game::new(board.clone());
    game.replay(room_route)?;
    
    println!("\nAfter room route:");
    println!("  Moves: {}", game.moves());
    println!("  Pushes: {}", game.pushes());
    println!("  Solved: {}", game.solved());
    
    if !game.solved() {
        // Show box positions
        let state = game.state();
        println!("\nBox positions after room route:");
        for (i, &pos) in state.boxes.iter().enumerate() {
            if pos != sokomind_core::NONE {
                println!("  Box {}: cell {}", i, pos);
            }
        }
        println!("  Player: cell {}", state.player);
        
        // Continue with finish route
        println!("\nApplying finish route...");
        game.replay(finish_route)?;
        
        println!("\nAfter finish route:");
        println!("  Total moves: {}", game.moves());
        println!("  Pushes: {}", game.pushes());
        println!("  Solved: {}", game.solved());
        
        if game.solved() {
            println!("\n✓ Complete route solves Grand Hall!");
            println!("  Combined length: {} moves", game.moves());
            
            // Save complete route
            let complete_route = format!("{}{}", room_route, finish_route);
            fs::write("/home/coder/.qwen/tmp/route-study/complete_route.txt", &complete_route)?;
            println!("  Written to complete_route.txt");
        }
    } else {
        println!("  Puzzle already solved after room route!");
    }
    
    Ok(())
}
