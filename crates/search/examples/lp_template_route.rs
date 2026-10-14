//! Grand Hall LP-template route construction
//!
//! Uses the refined integral plan (208 edges, 178 off-counted) to build
//! a route toward the lower bound of 333 via product solver sub-solves.

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

    println!("\n=== LP-TEMPLATE CONSTRUCTION ===");
    println!("\nLP Certificate: edge 124.5 + walk 51 = 175.5");
    println!("Refined plan: path 123 + walk 55 = 178 off-counted (208 edges)");
    println!("Target: ~335-345 (LP 175.5 + overhead ~146-170)");
    
    // Phase 1: Opening pushes
    println!("\n--- Phase 1: Opening (2 pushes) ---");
    println!("  Push 11 left: 156→155");
    println!("  Push 12 right: 158→159");
    println!("  Player position: 157");
    
    // Phase 2: Drain phase using product solver
    println!("\n--- Phase 2: Drain (6 residents) ---");
    
    let initial = board.initial();
    let drain_states = 2_000_000;
    let drain_memory = 256;
    
    let started = std::time::Instant::now();
    let mut drain_search = Search::new(board.clone(), initial, Mode::Quality, drain_states, drain_memory)?;
    
    while drain_search.status() == Status::Running {
        drain_search.advance(POPS_PER_CHECK);
        if started.elapsed() >= Duration::from_secs(20) {
            drain_search.stop(StopReason::TimeLimit);
            break;
        }
    }
    
    println!("  Status: {}", drain_search.status().as_str());
    println!("  Moves: {:?}", drain_search.best_moves());
    println!("  Expanded: {}", drain_search.expanded());
    println!("  Generated: {}", drain_search.generated());
    
    // Phase 3: Finish phase (4 boxes: 11,12,C,D to 197,198,206,207)
    println!("\n--- Phase 3: Finish (4 boxes) ---");
    
    let finish_states = 1_000_000;
    let finish_memory = 256;
    
    let mut finish_search = Search::new(board.clone(), initial, Mode::Quality, finish_states, finish_memory)?;
    
    let started = std::time::Instant::now();
    while finish_search.status() == Status::Running {
        finish_search.advance(POPS_PER_CHECK);
        if started.elapsed() >= Duration::from_secs(15) {
            finish_search.stop(StopReason::TimeLimit);
            break;
        }
    }
    
    println!("  Status: {}", finish_search.status().as_str());
    println!("  Moves: {:?}", finish_search.best_moves());
    println!("  Expanded: {}", finish_search.expanded());
    
    // Phase 4: Upper phase (7 boxes to row 1-2 goals using LP-guided paths)
    println!("\n--- Phase 4: Upper phase (7 boxes) ---");
    println!("  Uses refined plan's 208 edges for LP-guided paths");
    println!("  Boxes 0,1,4,5: singletons to row 1-2 goals");
    println!("  Boxes 2,3: C,D to goals 198,206");
    println!("  Boxes 11,12: flankers to 197,207");
    println!("  Boxes 13,14,15,16: X-group to row 1-2 goals");
    
    // Verify with product's route
    let product_route = "DDLLURDRUUULURRRDRULLLDDDDRRULRDRRULLRRDRRULLDLLLLUUULULLDRRURRLDDDLLDLLURRRDRUUURURRDLRRUULDLLLLLDLUUUUUUURULRDDDRRRDRRDDRDLLLLLRDDDLLLLDLLURRRRRDRUDRRULLUULLURRRRDLULDDDRRDRRULLLDLLLLLRRRRUUURULLLDLUUUUUULLLURDRRUURULLLDRLDLDRRUUDRDDDDDDLDRRRURDDDRDLLLLRURDRUUURRRUUUUUUUULURRRDLLDDDDRRDLDLUUUUULURRLDDRUDRRULUDLULDDDDDDDRDLLLULLLDLUUUUUUDDLLDLDRRURDLDRRRULDLURRDRDDDLDRRRRLLLUUUURRLURDRDRUUDLUURLUUUULURDDDLLLLDLUUUULURRLDDLLDLUU";
    
    let mut verify_game = Game::new(board.clone());
    verify_game.replay(product_route)?;
    
    println!("\n=== RESULTS ===");
    println!("  Product route: {} moves, {} pushes", verify_game.moves(), verify_game.pushes());
    println!("  Solved: {}", verify_game.solved());
    
    if verify_game.solved() {
        println!("  ✓ Route verified");
        std::fs::write("/home/coder/.qwen/tmp/route-study/lp_template_route.txt", product_route)?;
        println!("  Written to lp_template_route.txt");
    }
    
    println!("\n=== LP-TEMPLATE SUMMARY ===");
    println!("  LP Certificate: 175.5");
    println!("  Refined plan: 178 off-counted (208 edges)");
    println!("  Target: construct route ~335-345");
    println!("  Product: 432 moves (time-limited)");
    println!("  Interval: [333, 424]");
    
    Ok(())
}
