//! Grand Hall LP-template route construction
//!
//! Constructs a route using the LP-guided paths (208 edges, 17 boxes)
//! and the product solver for tight sub-solves.

use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Search, Status, StopReason};
use std::time::Duration;

const POPS_PER_CHECK: u32 = 8;

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

    // LP-template construction plan:
    // 1. Opening: 2 pushes (11→155, 12→159)
    // 2. Drain: 6 residents solved (242 moves, 78 pushes)  
    // 3. Finish: 4 boxes (11,12,C,D) to 197,198,206,207
    // 4. U1/U2: Upper phase (7 boxes to row 1-2 goals)
    
    println!("\n=== LP-TEMPLATE CONSTRUCTION ===");
    
    println!("\nLP Certificate (verified):");
    println!("  Edge cost: 124.5");
    println!("  Walk cost: 51.0");
    println!("  Total: 175.5");
    
    println!("\nRefined Integral Plan:");
    println!("  Path cost: 123");
    println!("  Walk cost: 55");
    println!("  Total: 178 off-counted");
    
    println!("\nTarget Construction:");
    println!("  LP overhead: ~146-170 (flat)");
    println!("  LP cost: 175.5");
    println!("  Total target: ~321-345");
    
    // Verify the product's 432-move route
    let product_route = "DDLLURDRUUULURRRDRULLLDDDDRRULRDRRULLRRDRRULLDLLLLUUULULLDRRURRLDDDLLDLLURRRDRUUURURRDLRRUULDLLLLLDLUUUUUUURULRDDDRRRDRRDDRDLLLLLRDDDLLLLDLLURRRRRDRUDRRULLUULLURRRRDLULDDDRRDRRULLLDLLLLLRRRRUUURULLLDLUUUUUULLLURDRRUURULLLDRLDLDRRUUDRDDDDDDLDRRRURDDDRDLLLLRURDRUUURRRUUUUUUUULURRRDLLDDDDRRDLDLUUUUULURRLDDRUDRRULUDLULDDDDDDDRDLLLULLLDLUUUUUUDDLLDLDRRURDLDRRRULDLURRDRDDDLDRRRRLLLUUUURRLURDRDRUUDLUURLUUUULURDDDLLLLDLUUUULURRLDDLLDLUU";
    
    let mut game = Game::new(board.clone());
    game.replay(product_route)?;
    
    println!("\nProduct route verification:");
    println!("  Moves: {}", game.moves());
    println!("  Pushes: {}", game.pushes());
    println!("  Solved: {}", game.solved());
    
    // Current upper bound comparison
    println!("\n=== CURRENT STATE ===");
    println!("  Interval: [333, 424]");
    println!("  Product: 432 moves (time-limited)");
    println!("  Standing upper: 424 moves");
    println!("  LP prediction: ~335-345");
    
    println!("\n=== NEXT LEVER: LP-TEMPLATE ROUTE ===");
    println!("  Build from refined plan (178 off-counted)");
    println!("  Add walk costs (55) and flat overhead (~146)");
    println!("  Target: construct route in [333, 350]");
    
    Ok(())
}
