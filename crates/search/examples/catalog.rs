//! Reproducible native search corpus. JSON Lines lets the same runner serve
//! fixed-node CI checks, WASM parity, and repeated timed measurements.
//! `--parallel N` runs N items at once, one engine each, the records
//! emitted in the sequential order either way; each record's timings are
//! its own item's wall, so only they differ between worker counts.
use serde_json::{Value, json};
use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Proof, Search, SearchStats, Status, StopReason};
use std::{
    env,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Queue pops per `advance` between time checks, as `POPS_PER_CHECK` in
/// crates/server/src/solve.rs. It sets how finely the timings and the
/// `first_route_*` fields resolve, so changing it moves recorded numbers.
const POPS_PER_CHECK: u32 = 8;

struct Options {
    states: usize,
    memory: usize,
    time: Option<Duration>,
    repeat: usize,
    puzzle: Option<String>,
    mode: Option<Mode>,
    parallel: usize,
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
            parallel: 1,
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
                "--parallel" => options.parallel = number()?,
                "--mode" => {
                    options.mode = Some(Mode::parse(&value).map_err(|error| error.to_string())?);
                }
                _ => return Err(format!("Unknown option: {key}")),
            }
        }
        if !(1..=20).contains(&options.repeat) {
            return Err("Use 1..20 repeats".into());
        }
        if options.parallel == 0 {
            return Err("Use at least 1 parallel worker".into());
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

/// One corpus item: the sample it belongs to, the puzzle's id and board,
/// and the mode to run it in. The list is built in the order the records
/// print in, whatever ran them.
struct Item {
    sample: usize,
    id: String,
    board: Board,
    mode: Mode,
}

/// Runs one item and builds its record. The timings are this item's own
/// wall, setup included; every other field comes from the search itself,
/// so two runs of the same item agree wherever the search is
/// deterministic.
fn run_one(item: &Item, options: &Options) -> Result<Value, String> {
    let Item {
        sample,
        id,
        board,
        mode,
    } = item;
    let name = mode.as_str();
    // One clock from before Search::new times setup_us,
    // first_route_us, search_us and the --time-ms limit, so the
    // last three include setup (the arena's first chunks and
    // table, and the heuristic tables);
    // reconstruct_us times solution() alone.
    let started = Instant::now();
    let mut search = Search::new(
        board.clone(),
        board.initial(),
        *mode,
        options.states,
        options.memory,
    )
    .map_err(|error| error.to_string())?;
    let setup_us = started.elapsed().as_micros();
    let mut first_route_us = None;
    let mut first_route_expanded = None;
    let mut first_route_generated = None;
    while search.status() == Status::Running {
        if options.time.is_some_and(|limit| started.elapsed() >= limit) {
            search.stop(StopReason::TimeLimit);
        } else {
            search.advance(POPS_PER_CHECK);
        }
        if first_route_us.is_none() && search.best_moves().is_some() {
            first_route_us = Some(started.elapsed().as_micros());
            first_route_expanded = Some(search.expanded());
            first_route_generated = Some(search.generated());
        }
    }
    let search_us = started.elapsed().as_micros();
    let route = search.solution().map_err(|error| error.to_string())?;
    let reconstruct_us = started.elapsed().as_micros() - search_us;
    let pushes = if let Some(route) = &route {
        let mut game = Game::new(board.clone());
        game.replay(route).map_err(|error| error.to_string())?;
        if !game.solved() || Some(game.moves()) != search.best_moves() {
            return Err(format!("Invalid route/counters for {id}/{name}"));
        }
        Some(game.pushes())
    } else {
        None
    };
    Ok(json!({
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
    }))
}

/// Runs `work` across `options.parallel` threads, each taking the next
/// unstarted item until none is left, and returns the results in work
/// order. One engine per item: nothing is shared but the work list and
/// the options.
fn run_pool(work: &[Item], options: &Options) -> Vec<Result<Value, String>> {
    let next = AtomicUsize::new(0);
    let (send, recv) = mpsc::channel();
    thread::scope(|scope| {
        for _ in 0..options.parallel {
            let (next, send) = (&next, &send);
            scope.spawn(move || {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    if at >= work.len() {
                        return;
                    }
                    let _ = send.send((at, run_one(&work[at], options)));
                }
            });
        }
    });
    drop(send);
    let mut results: Vec<Option<Result<Value, String>>> = (0..work.len()).map(|_| None).collect();
    for (at, result) in recv {
        results[at] = Some(result);
    }
    results
        .into_iter()
        .map(|result| result.expect("every item ran"))
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse()?;
    let puzzles: Value = serde_json::from_str(include_str!("../../../data/puzzles.json"))?;
    // The work list, in the order the sequential loop always visited it:
    // every (sample, puzzle, mode) the filters keep.
    let mut work = Vec::new();
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
                work.push(Item {
                    sample,
                    id: id.to_owned(),
                    board: board.clone(),
                    mode,
                });
            }
        }
    }
    if work.is_empty() {
        return Err("No matching puzzle/mode".into());
    }
    // The records print in work order whether one thread ran them all or
    // a pool did; only the per-record timings, each its own item's wall,
    // depend on the worker count.
    let results: Vec<Result<Value, String>> = if options.parallel == 1 {
        work.iter().map(|item| run_one(item, &options)).collect()
    } else {
        run_pool(&work, &options)
    };
    for result in results {
        println!("{}", result?);
    }
    Ok(())
}
