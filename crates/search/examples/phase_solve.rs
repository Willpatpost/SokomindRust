//! Solve one phase sub-puzzle of the LP-template route construction.
//!
//! The puzzle file holds the phase's frozen world: every box the phase
//! must not touch is a wall, the phase's own boxes sit at their current
//! cells, and goals are placed to force the phase's exact end state
//! (X targets as S, lettered targets as lowercase; a park cell carries
//! the box's own letter as a synthetic goal). A route found here
//! replays 1:1 on the real board at the same state: a frozen box and a
//! wall block the keeper identically, and the sub-puzzle never pushes
//! into either.

use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Search, Status, StopReason};
use std::{env, fs, time::Duration};

const POPS_PER_CHECK: u32 = 8;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 2 {
        return Err("usage: phase_solve <puzzle-file> <route-out> \
             [--mode M] [--states N] [--memory MiB] [--time-ms N]"
            .into());
    }
    let puzzle_path = args.remove(0);
    let route_path = args.remove(0);
    let mut states = 60_000_000;
    let mut memory = 512;
    let mut time_ms = 90_000u64;
    let mut mode = Mode::Quality;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--states" => states = args.next().ok_or("Missing --states")?.parse()?,
            "--memory" => memory = args.next().ok_or("Missing --memory")?.parse()?,
            "--time-ms" => time_ms = args.next().ok_or("Missing --time-ms")?.parse()?,
            "--mode" => {
                mode =
                    Mode::parse(&args.next().ok_or("Missing --mode")?).map_err(|e| e.to_string())?
            }
            _ => return Err(format!("unknown arg {arg}").into()),
        }
    }

    let rows = fs::read_to_string(&puzzle_path)?;
    let board = Board::parse(&rows)?;
    println!(
        "Phase puzzle {puzzle_path}: {}x{}, labels {:?}, goals {:?}",
        board.width(),
        board.height(),
        board.labels(),
        board.goals()
    );

    let started = std::time::Instant::now();
    let mut search = Search::new(board.clone(), board.initial(), mode, states, memory)?;
    while search.status() == Status::Running {
        if started.elapsed() >= Duration::from_millis(time_ms) {
            search.stop(StopReason::TimeLimit);
            break;
        }
        search.advance(POPS_PER_CHECK);
    }

    println!(
        "  Status: {}, best moves {:?}, expanded {}",
        search.status().as_str(),
        search.best_moves(),
        search.expanded()
    );
    let Some(route) = search.solution()? else {
        return Err("No route found".into());
    };
    let mut game = Game::new(board);
    game.replay(&route)?;
    if !game.solved() {
        return Err("Route did not solve the sub-puzzle".into());
    }
    println!(
        "  Solved: {} moves, {} pushes, {} states, {:.1}s",
        route.len(),
        game.pushes(),
        search.expanded(),
        started.elapsed().as_secs_f32()
    );
    fs::write(&route_path, &route)?;
    println!("  Written to {route_path}");
    Ok(())
}
