//! Finish phase: 4 boxes (A,B,C,D) to matching goals (a,b,c,d)
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

    // 4 boxes A,B,C,D with goals a,b,c,d (matching labels)
    let rows = vec![
        "OO a b c d   O",   // row 0: goals a,b,c,d (lowercase)
        "OO           O",
        "O  R         O",   // row 2: player R
        "O            O",
        "O  A B C D   O",   // row 4: boxes A,B,C,D (uppercase)
        "OOOOOOOOOOOOOO",
    ];

    let board = Board::parse(&rows.join("\n"))?;
    
    println!("Finish phase solver");
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
