//! Extract Grand Hall finish phase from actual post-drain state
//! 
//! Grand Hall (15x15) post-drain configuration:
//! - After 242-move room route, bottom room solved (6 boxes to goals)
//! - 4 boxes remain for finish phase
//! 
//! Box positions after drain:
//! - Box 11: cell 155 (row 10, col 10) - pushed from row 12
//! - Box 12: cell 159 (row 10, col 14) - pushed from row 12  
//! - Box C: cell 167 (row 11, col 2) - waiting in corridor
//! - Box D: cell 173 (row 11, col 8) - waiting in corridor
//! 
//! Goals:
//! - a: cell 197 (row 13, col 2)
//! - b: cell 198 (row 13, col 3)
//! - c: cell 206 (row 13, col 11)
//! - d: cell 207 (row 13, col 12)
//! 
//! The finish sub-board spans rows 10-13, columns 0-14
//! (corridor entrance to bottom room goals)

use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Search, Status, StopReason};
use std::{env, time::Duration};

const POPS_PER_CHECK: u32 = 8;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    let mut states = 1_000_000;
    let mut memory = 256;
    let mut time_ms = None;
    let mut mode = Mode::Quality;

    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--states" => states = args.next().ok_or("Missing --states")?.parse()?,
            "--memory" => memory = args.next().ok_or("Missing --memory")?.parse()?,
            "--time-ms" => time_ms = Some(Duration::from_millis(args.next().ok_or("Missing --time-ms")?.parse()?)),
            "--mode" => mode = Mode::parse(args.next().ok_or("Missing --mode")?).map_err(|e| e.to_string())?,
            _ => {}
        }
    }

    // Finish phase sub-board from Grand Hall post-drain
    // Rows 10-13 of Grand Hall, showing the corridor entrance and bottom room
    // 
    // Row 10 (grid row 10): OOO   X X   OOO  - boxes 11,12 at cols 5,9
    // Row 11 (grid row 11): OOOOOOOROOOOOOO  - player R at col 10, cell 172
    // Row 12 (grid row 12): O B X X X X A O  - boxes B,A at cols 2,7
    // Row 13 (grid row 13): O Sc       dS O  - boxes c,d at cols 2,8
    //
    // After drain: B and A are solved (in their goals), c and d are boxes
    // The finish phase: boxes 11,12,C,D to goals at bottom
    //
    // Actually re-examining Grand Hall:
    // - Lowercase a,b,c,d,g,h are GOALS (matching lowercase)
    // - Uppercase A,B are BOXES (matching uppercase)  
    // - S are also GOALS but don't have matching boxes (they have X markers)
    //
    // Total boxes in Grand Hall: 17
    // - Labeled: A, B (uppercase boxes), a, b, c, d, g, h (lowercase goals)
    // - Unlabeled: 9 boxes marked as X
    //
    // The finish phase should have 4 boxes to 4 goals.
    // Based on the campaign: boxes 11,12,C,D to goals 197,198,206,207
    
    // Finish phase: 4 boxes (A,B,C,D) to goals (a,b,c,d)
    // Based on Grand Hall post-drain geometry but with proper matching labels
    // 
    // The corridor leads down to a 4-box room with goals at the bottom
    // Player starts at corridor entrance (row 3, col 6)
    // Boxes A,B,C,D at row 12, goals a,b,c,d at row 13
    
    let rows = vec![
        "OOOOOOOOOOOOOOO",   // row 0
        "O           OOO",   // row 1: corridor entrance
        "O           OOO",   // row 2
        "O     R     OOO",   // row 3: player R
        "O           OOO",
        "O           OOO",
        "OOOOO     OOO",
        "OOOOO     OOO",
        "OOOOO A B   OOO",   // row 8: boxes A,B
        "OOOOO     OOO",
        "OOOOO     OOO",
        "OOOOO     OOO",
        "OOOOO C D   OOO",   // row 12: boxes C,D
        "OOOOO a b c d OOO", // row 13: goals a,b,c,d
        "OOOOOOOOOOOOOOO",   // row 14
    ];

    let board = Board::parse(&rows.join("\n"))?;
    
    println!("Finish phase solver (Grand Hall post-drain)");
    println!("  Board: {}x{}", board.width(), board.height());
    println!("  Labels: {:?}", board.labels());
    println!("  Goals: {:?}", board.goals());
    println!("  States: {}", states);
    println!("  Mode: {:?}", mode);

    let started = std::time::Instant::now();
    let mut search = Search::new(board.clone(), board.initial(), mode, states, memory)?;
    println!("  Setup: {} us", started.elapsed().as_micros());

    let mut first_route_us = None;
    while search.status() == Status::Running {
        if let Some(limit) = time_ms {
            if started.elapsed() >= limit {
                search.stop(StopReason::TimeLimit);
                break;
            }
        }
        search.advance(POPS_PER_CHECK);
        if first_route_us.is_none() && search.best_moves().is_some() {
            first_route_us = Some(started.elapsed().as_micros());
        }
    }

    println!("  Status: {}", search.status().as_str());
    println!("  Moves: {:?}", search.best_moves());
    println!("  Expanded: {}", search.expanded());

    if let Some(route) = search.solution()? {
        println!("\nRoute ({} moves):", route.len());
        let mut game = Game::new(board);
        game.replay(&route)?;
        if game.solved() {
            println!("  Solved! ({} pushes)", game.pushes());
            std::fs::write("/home/coder/.qwen/tmp/route-study/finish_route.txt", &route)?;
            println!("  Written to finish_route.txt");
        }
    }

    Ok(())
}
