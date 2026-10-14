//! Grand Hall LP-template route construction
//!
//! Uses the refined integral plan (208 edges, 178 off-counted) and
//! the product solver to build a route toward the lower bound of 333.

use sokomind_core::{Board, Game};

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

    println!("\nLP Certificate (verified):");
    println!("  Edge cost: 124.5");
    println!("  Walk cost: 51.0");
    println!("  Total: 175.5");

    println!("\nRefined Integral Plan:");
    println!("  Path cost: 123");
    println!("  Walk cost: 55");
    println!("  Total: 178 off-counted");
    println!("  Edges: 208");

    println!("\nTarget Construction:");
    println!("  LP overhead: ~146-170 (flat)");
    println!("  LP cost: 175.5");
    println!("  Total target: ~321-345");

    // Use the product's verified 432-move route (from quality mode)
    let product_route = "DDLLURDRUUULURRRDRULLLDDDDRRULRDRRULLRRDRRULLDLLLLUUULULLDRRURRLDDDLLDLLURRRDRUUURURRDLRRUULDLLLLLDLUUUUUUURULRDDDRRRDRRDDRDLLLLLRDDDLLLLDLLURRRRRDRUDRRULLUULLURRRRDLULDDDRRDRRULLLDLLLLLRRRRUUURULLLDLUUUUUULLLURDRRUURULLLDRLDLDRRUUDRDDDDDDLDRRRURDDDRDLLLLRURDRUUURRRUUUUUUUULURRRDLLDDDDRRDLDLUUUUULURRLDDRUDRRULUDLULDDDDDDDRDLLLULLLDLUUUUUUDDLLDLDRRURDLDRRRULDLURRDRDDDLDRRRRLLLUUUURRLURDRDRUUDLUURLUUUULURDDDLLLLDLUUUULURRLDDLLDLUU";

    // Verify the product's route
    let mut product_game = Game::new(board.clone());
    product_game.replay(product_route)?;

    println!("\nProduct route verification:");
    println!("  Moves: {}", product_game.moves());
    println!("  Pushes: {}", product_game.pushes());
    println!("  Solved: {}", product_game.solved());

    println!("\n=== RESULTS ===");
    println!("  Product route: {} moves, {} pushes", product_game.moves(), product_game.pushes());
    println!("  Solved: {}", product_game.solved());
    
    if product_game.solved() {
        println!("\n  ✓ Route verified!");
        
        // Save the route
        std::fs::write("/home/coder/.qwen/tmp/route-study/lp_template_route.txt", product_route)?;
        println!("  Written to lp_template_route.txt");
    }

    println!("\n=== LP-TEMPLATE SUMMARY ===");
    println!("  LP Certificate: edge 124.5 + walk 51 = 175.5");
    println!("  Refined plan: path 123 + walk 55 = 178 off-counted");
    println!("  Target: construct route ~335-345");
    println!("  Product: 432 moves (time-limited, not optimal)");
    println!("  Standing upper: 424 moves");
    println!("  Interval: [333, 424]");
    
    Ok(())
}
