//! Re-solve the Grand Hall drain sub-board with continuation-safe exits.
//!
//! The v1 drain (room-route-v1.txt, 242 moves / 78 pushes) parks A and B
//! on row 8's segment-end cells 121/133 — dead pockets: A's only vertical
//! pusher cell (136, row 9 col 1) and horizontal pusher cell (120, row 8
//! col 0) are walls, and B's (148, 134) mirror them. Neither box can ever
//! move again, so U2 (the residents' climb to rows 1-2) is impossible.
//!
//! v2 moves the labeled exits one cell inward (a: 121 -> 122, b: 133 ->
//! 132); every exit cell now has at least one usable pusher position.
//! Walls, door, residents and keeper start are unchanged, so the route
//! still replays 1:1 on the full board after the opening parks the
//! flankers at 155/159.

use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Search, Status, StopReason};
use std::{env, time::Duration};

const POPS_PER_CHECK: u32 = 8;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let mut states = 20_000_000;
    let mut memory = 256;
    let mut time_ms = None;
    let mut mode = Mode::Fast;

    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--states" => states = args.next().ok_or("Missing --states")?.parse()?,
            "--memory" => memory = args.next().ok_or("Missing --memory")?.parse()?,
            "--time-ms" => {
                time_ms = Some(Duration::from_millis(
                    args.next().ok_or("Missing --time-ms")?.parse()?,
                ))
            }
            "--mode" => {
                mode = Mode::parse(args.next().ok_or("Missing --mode")?).map_err(|e| e.to_string())?
            }
            _ => {}
        }
    }

    // Real rows 7-14: sealed top, exit row, staging, flanker walls
    // (155/159 = the post-opening parking cells), the door, the six
    // residents, empty row 13, floor.
    let rows = vec![
        "OOOOOOOOOOOOOOO",
        "O aS S O S Sb O",
        "OOO    R    OOO",
        "OOO  O   O  OOO",
        "OOOOOOO OOOOOOO",
        "O B X X X X A O",
        "O             O",
        "OOOOOOOOOOOOOOO",
    ];
    let board = Board::parse(&rows.join("\n"))?;
    println!("Drain sub-board v2 (continuation-safe exits)");
    println!("  Board: {}x{}", board.width(), board.height());
    println!("  Labels: {:?}", board.labels());
    println!("  Goals: {:?}", board.goals());
    println!("  Mode: {:?}, states {}, memory {} MiB", mode, states, memory);

    let started = std::time::Instant::now();
    let mut search = Search::new(board.clone(), board.initial(), mode, states, memory)?;
    println!("  Setup: {} us", started.elapsed().as_micros());

    while search.status() == Status::Running {
        if let Some(limit) = time_ms {
            if started.elapsed() >= limit {
                search.stop(StopReason::TimeLimit);
                break;
            }
        }
        search.advance(POPS_PER_CHECK);
    }

    println!("  Status: {}", search.status().as_str());
    println!("  Moves: {:?}", search.best_moves());
    println!("  Expanded: {}", search.expanded());

    if let Some(route) = search.solution()? {
        let mut game = Game::new(board.clone());
        game.replay(&route)?;
        if game.solved() {
            println!(
                "  Solved: {} moves, {} pushes, {} states, {:.1}s, fingerprint {}",
                route.len(),
                game.pushes(),
                search.expanded(),
                started.elapsed().as_secs_f32(),
                board.fingerprint()
            );
            std::fs::write("/home/coder/.qwen/tmp/route-study/room-route.txt", &route)?;
            println!("  Written to room-route.txt (v2)");
        } else {
            return Err("Route did not solve the sub-board".into());
        }
    } else {
        return Err("No route found".into());
    }

    Ok(())
}
