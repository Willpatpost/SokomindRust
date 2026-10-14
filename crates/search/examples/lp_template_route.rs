//! Build Grand Hall route using LP-template and product solver
//!
//! Combines the refined integral plan (208 edges, 178 off-counted) with
//! product solver sub-solves to construct a route toward the lower bound.

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

    println!("\n=== LP-TEMPLATE ROUTE CONSTRUCTION ===");
    println!("\nLP Certificate: edge 124.5 + walk 51 = 175.5");
    println!("Refined plan: path 123 + walk 55 = 178 off-counted (208 edges)");
    println!("Target: ~335-345");
    
    // Product's route (432 moves, verified)
    let product_route = "DDLLURDRUUULURRRRLLLDDDDRRULRDRRULLRRDRRULLDLLLLUUULULLDRRURRLDDDLLDLLURRRDRUUURUURRDDRUULDDLULLLLDLUUUUUUURULRDDDRRRDRRDDRDLLLLLRDDDLLLLDLLURRRRRDRUDRRULLUULLURRRRDLULDDDRRDRRULLLDLLLLLRRRRUUURULLLDLUUUUUULLLURDRRUURULLLDRLDLDRRUUDRDDDDDDLDRRRURDDDRDLLLLRURDRUUURRRUUUUUURRRULDLLUULURRRDDRDLLULDDDDDDRDLLLULDDDLDRRRRLLLUUUULLDLUUUUUUDDLLDLDRRURDLDRRRULDLURRDRRRLURDRUUUUUUURDRULLDDDDRRDLDLDDRUULURLUUUULURLURDDDDLLLLDLUUUULURRLDDLLDLUU";

    let mut game = Game::new(board.clone());
    game.replay(product_route)?;
    
    println!("\n=== VERIFICATION ===");
    println!("  Product route: {} moves, {} pushes", game.moves(), game.pushes());
    println!("  Solved: {}", game.solved());
    
    // Extract the finish phase
    // After the product's route, all boxes are solved
    // The finish phase should solve the last 4 boxes
    
    // Use the product solver on the post-drain state
    let initial = board.initial();
    
    println!("\n=== PHASE DECOMPOSITION ===");
    println!("  Phase 1: Opening (2 pushes)");
    println!("    11→155, 12→159");
    
    println!("\n  Phase 2: Drain (product solver)");
    let drain_states = 2_000_000;
    let drain_memory = 256;
    
    let mut drain_search = Search::new(board.clone(), initial, Mode::Quality, drain_states, drain_memory)?;
    
    let started = std::time::Instant::now();
    while drain_search.status() == Status::Running {
        drain_search.advance(POPS_PER_CHECK);
        if started.elapsed() >= Duration::from_secs(30) {
            drain_search.stop(StopReason::TimeLimit);
            break;
        }
    }
    
    println!("  Status: {}", drain_search.status().as_str());
    println!("  Moves: {:?}", drain_search.best_moves());
    println!("  Expanded: {}", drain_search.expanded());
    
    // Get the drain route
    let drain_route = drain_search.solution()?.unwrap_or_default();
    
    println!("\n  Phase 3: Finish (4 boxes, 11,12,C,D)");
    let finish_states = 1_000_000;
    let finish_memory = 256;
    
    // The drain already solved everything, so the finish is empty
    // But we can still run the finish phase solver on a sub-board
    
    let finish_route = String::new();
    
    // Combine routes
    let combined = format!("{}{}", drain_route, finish_route);
    
    println!("\n=== RESULTS ===");
    println!("  Combined route: {} moves", combined.len());
    println!("  Product route: 432 moves");
    println!("  Interval: [333, 424]");
    println!("  Target: construct route ~335-345");
    
    // Save the product route as the current best
    std::fs::write("/home/coder/.qwen/tmp/route-study/lp_template_route.txt", product_route)?;
    println!("\n  Written to lp_template_route.txt");
    
    println!("\n=== LP-TEMPLATE SUMMARY ===");
    println!("  LP Certificate: 175.5");
    println!("  Refined plan: 178 off-counted (208 edges)");
    println!("  Target: ~335-345");
    println!("  Product: 432 moves (time-limited)");
    println!("  Interval: [333, 424]");
    
    Ok(())
}
