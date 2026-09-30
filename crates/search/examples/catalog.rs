//! Reproducible native search corpus. JSON Lines lets the same runner serve
//! fixed-node CI checks, WASM parity, and repeated timed measurements.
use serde_json::{Value, json};
use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Proof, Search, SearchStats, Status, StopReason};
use std::{
    env,
    time::{Duration, Instant},
};

struct Options {
    states: usize,
    memory: usize,
    time: Option<Duration>,
    repeat: usize,
    puzzle: Option<String>,
    mode: Option<Mode>,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut options = Self {
            states: 20_000,
            memory: 64,
            time: None,
            repeat: 1,
            puzzle: None,
            mode: None,
        };
        let mut args = env::args().skip(1);
        while let Some(key) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("Missing value after {key}"))?;
            let number = || {
                value
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid number for {key}: {value}"))
            };
            match key.as_str() {
                "--states" => options.states = number()?,
                "--memory" => options.memory = number()?,
                "--time-ms" => options.time = Some(Duration::from_millis(number()? as u64)),
                "--repeat" => options.repeat = number()?,
                "--puzzle" => options.puzzle = Some(value),
                "--mode" => {
                    options.mode = Some(Mode::parse(&value).map_err(|error| error.to_string())?);
                }
                _ => return Err(format!("Unknown option: {key}")),
            }
        }
        if !(1..=20).contains(&options.repeat) {
            return Err("Use 1..20 repeats".into());
        }
        Ok(options)
    }
}

fn proof_value(proof: Option<Proof>) -> Value {
    let kind = proof.map_or("none", Proof::kind);
    match proof {
        None | Some(Proof::Unsolvable) => json!({ "kind": kind }),
        Some(Proof::Optimal { moves }) => json!({ "kind": kind, "moves": moves }),
        Some(Proof::Bounded {
            lower_bound,
            upper_bound,
        }) => json!({ "kind": kind, "lower": lower_bound, "upper": upper_bound }),
    }
}

/// The counters as one object keyed by [`SearchStats::FIELDS`].
fn stats_value(stats: SearchStats) -> Value {
    SearchStats::FIELDS
        .into_iter()
        .zip(stats.values())
        .map(|(field, value)| (field.to_owned(), Value::from(value)))
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse()?;
    let puzzles: Value = serde_json::from_str(include_str!("../../../data/puzzles.json"))?;
    let mut count = 0;
    for sample in 0..options.repeat {
        for puzzle in puzzles.as_array().ok_or("Catalog is not an array")? {
            let id = puzzle["id"].as_str().ok_or("Puzzle has no id")?;
            if options.puzzle.as_deref().is_some_and(|wanted| wanted != id) {
                continue;
            }
            let rows = puzzle["rows"]
                .as_array()
                .ok_or("Puzzle has no rows")?
                .iter()
                .map(|row| row.as_str().ok_or("Non-string row"))
                .collect::<Result<Vec<_>, _>>()?
                .join("\n");
            let board = Board::parse(&rows)?;
            for mode in Mode::ALL {
                if options.mode.is_some_and(|wanted| wanted != mode) {
                    continue;
                }
                let name = mode.as_str();
                let started = Instant::now();
                let mut search = Search::new(
                    board.clone(),
                    board.initial(),
                    mode,
                    options.states,
                    options.memory,
                )?;
                let setup_us = started.elapsed().as_micros();
                let mut first_route_us = None;
                let mut first_route_expanded = None;
                let mut first_route_generated = None;
                while search.status() == Status::Running {
                    if options.time.is_some_and(|limit| started.elapsed() >= limit) {
                        search.stop(StopReason::TimeLimit);
                    } else {
                        search.advance(8);
                    }
                    if first_route_us.is_none() && search.best_moves().is_some() {
                        first_route_us = Some(started.elapsed().as_micros());
                        first_route_expanded = Some(search.expanded());
                        first_route_generated = Some(search.generated());
                    }
                }
                let search_us = started.elapsed().as_micros();
                let route = search.solution()?;
                let reconstruct_us = started.elapsed().as_micros() - search_us;
                let pushes = if let Some(route) = &route {
                    let mut game = Game::new(board.clone());
                    game.replay(route)?;
                    if !game.solved() || Some(game.moves()) != search.best_moves() {
                        return Err(format!("Invalid route/counters for {id}/{name}").into());
                    }
                    Some(game.pushes())
                } else {
                    None
                };
                let record = json!({
                    "id": id,
                    "fingerprint": board.fingerprint(),
                    "mode": name,
                    "sample": sample,
                    "max_states": options.states,
                    "memory_mib": options.memory,
                    "status": search.status().as_str(),
                    "moves": search.best_moves(),
                    "pushes": pushes,
                    "route": route,
                    "proof": proof_value(search.proof()),
                    "lower_bound": search.lower_bound(),
                    "expanded": search.expanded(),
                    "generated": search.generated(),
                    "reserved_bytes": search.reserved_bytes(),
                    "first_route_expanded": first_route_expanded,
                    "first_route_generated": first_route_generated,
                    "setup_us": setup_us,
                    "first_route_us": first_route_us,
                    "search_us": search_us,
                    "reconstruct_us": reconstruct_us,
                    "stats": stats_value(search.stats())
                });
                // PEA* passes are a measurement only; the key exists only
                // with the switch on, so default records stay byte-identical.
                #[cfg(feature = "pea")]
                let record = {
                    let mut record = record;
                    record["reexpanded"] = json!(search.reexpansions());
                    record
                };
                println!("{record}");
                count += 1;
            }
        }
    }
    if count == 0 {
        return Err("No matching puzzle/mode".into());
    }
    Ok(())
}
