//! The greedy anytime-fallback experiment: can a constructor that never
//! searches route the boards the searches never route?
//!
//! Two catalog boards (gen-v2-340048-b5d312a6, gen-v2-350001-a996dcbc)
//! end every Fast, Quality and Optimal run at a state limit with no
//! route, so a player asking the app for help on them reads "No route
//! returned". A fallback that constructs instead of searches — flood
//! the player, list the legal pushes onto cells a box of its label can
//! still leave, take a push that most reduces a matching distance, and
//! restart with a new seed when nothing improves — needs no arena, no
//! state table and no budget beyond its own push count, which is what
//! "anytime" would have to mean inside the app's worker.
//!
//! The measurement is honest either way: `--puzzle` names the board,
//! `--attempts` bounds the restarts, `--time-ms` the wall, and one JSON
//! record prints whether a route was constructed — replayed strictly
//! to prove it, and written out with `--out` — or the best matching
//! score any attempt reached. Seeds are fixed per attempt, so a record
//! printed once prints again.
use serde_json::json;
use sokomind_core::{Board, Cell, Game, NONE, OPPOSITE, State};
use std::{
    collections::VecDeque,
    env,
    time::{Duration, Instant},
};

/// One fixed-seed shift register. Its only decisions are ties among
/// equal-best pushes, so a run's record is a property of its attempt
/// count, not of the machine or the clock.
struct Rng(u64);

impl Rng {
    /// The next value in the sequence.
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A value below `n`, which the caller keeps above zero.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The cells the player reaches from `state`, with the direction each
/// was entered by: the walk to any reached cell is those directions,
/// read backward. Boxes block the flood; goals do not.
fn flood(board: &Board, state: &State) -> (Vec<bool>, Vec<u8>) {
    let mut visited = vec![false; board.tiles().len()];
    let mut entered = vec![0u8; board.tiles().len()];
    let mut queue = VecDeque::from([state.player]);
    visited[state.player as usize] = true;
    while let Some(cell) = queue.pop_front() {
        for direction in 0..4 {
            let next = board.neighbors()[cell as usize][direction];
            if next != NONE
                && !visited[next as usize]
                && !state.boxes[..board.labels().len()].contains(&next)
            {
                visited[next as usize] = true;
                entered[next as usize] = direction as u8 + 1;
                queue.push_back(next);
            }
        }
    }
    (visited, entered)
}

/// The walk from the flood's start to `target`, as direction indices.
fn walk(board: &Board, entered: &[u8], start: Cell, target: Cell) -> Vec<usize> {
    let mut path = Vec::new();
    let mut cell = target;
    while cell != start {
        let direction = entered[cell as usize] as usize - 1;
        path.push(direction);
        cell = board.neighbors()[cell as usize][OPPOSITE[direction]];
    }
    path.reverse();
    path
}

/// The push distances to one goal: `dist[c]` is the fewest pushes a box
/// at `c` needs to reach `goal`, other boxes aside, or `u32::MAX` when
/// no push chain does. Each edge of the breadth-first flood is one
/// reverse push — a cell the box could have come from, with the cell
/// the pusher would have stood on floor — so the same table names the
/// cells from which a box can still reach this goal.
fn pull_distances(board: &Board, goal: Cell) -> Vec<u32> {
    let mut dist = vec![u32::MAX; board.tiles().len()];
    let mut queue = VecDeque::from([goal]);
    dist[goal as usize] = 0;
    while let Some(cell) = queue.pop_front() {
        let next = dist[cell as usize] + 1;
        // A box at `from`, one step in `opposite` from `cell`, pushes onto
        // `cell` when the next cell in `opposite` is floor for the pusher
        // to stand on.
        for &opposite in &OPPOSITE {
            let from = board.neighbors()[cell as usize][opposite];
            if from == NONE || board.neighbors()[from as usize][opposite] == NONE {
                continue;
            }
            if dist[from as usize] == u32::MAX {
                dist[from as usize] = next;
                queue.push_back(from);
            }
        }
    }
    dist
}

/// The matching score of `state`: each box, in slot order, claims the
/// nearest unclaimed goal of its own label, and the score is the sum of
/// their push distances — zero exactly when every box sits placed. Not
/// admissible and not minimal, only downhill, which is all a hill
/// climb needs.
fn score(board: &Board, distances: &[Vec<u32>], state: &State) -> u64 {
    let mut claimed = vec![false; board.goals().len()];
    let mut total = 0u64;
    for (index, &cell) in state.boxes[..board.labels().len()].iter().enumerate() {
        let label = board.labels()[index];
        let mut best = u32::MAX;
        let mut taken = usize::MAX;
        for (goal, &(_, goal_label)) in board.goals().iter().enumerate() {
            if goal_label != label || claimed[goal] {
                continue;
            }
            let distance = distances[goal][cell as usize];
            if distance < best {
                best = distance;
                taken = goal;
            }
        }
        if best == u32::MAX {
            return u64::MAX;
        }
        claimed[taken] = true;
        total = total.saturating_add(best as u64);
    }
    total
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut puzzle = None;
    let mut attempts = 2_000usize;
    let mut time_ms = 60_000u64;
    let mut out = None;
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("Missing value after {key}"))?;
        match key.as_str() {
            "--puzzle" => puzzle = Some(value),
            "--attempts" => {
                attempts = value
                    .parse()
                    .map_err(|_| format!("Invalid --attempts: {value}"))?
            }
            "--time-ms" => {
                time_ms = value
                    .parse()
                    .map_err(|_| format!("Invalid --time-ms: {value}"))?
            }
            "--out" => out = Some(value),
            _ => return Err(format!("Unknown option: {key}").into()),
        }
    }
    let id = puzzle.ok_or("Name a puzzle with --puzzle")?;
    let catalog: serde_json::Value =
        serde_json::from_str(include_str!("../../../data/puzzles.json"))?;
    let entry = catalog
        .as_array()
        .ok_or("Catalog is not an array")?
        .iter()
        .find(|puzzle| puzzle["id"].as_str() == Some(id.as_str()))
        .ok_or_else(|| format!("No catalog puzzle {id}"))?;
    let rows = entry["rows"]
        .as_array()
        .ok_or("Puzzle has no rows")?
        .iter()
        .map(|row| row.as_str().ok_or("Non-string row"))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    let board = Board::parse(&rows)?;
    let boxes = board.labels().len();

    // One distance table per goal, then each label's live cells: the
    // targets a push may land a box on without stranding it.
    let distances: Vec<Vec<u32>> = board
        .goals()
        .iter()
        .map(|&(goal, _)| pull_distances(&board, goal))
        .collect();
    let mut live = vec![vec![false; board.tiles().len()]; 26];
    for (goal, &(_, label)) in board.goals().iter().enumerate() {
        for cell in 0..board.tiles().len() {
            if distances[goal][cell] != u32::MAX {
                live[(label - b'A') as usize][cell] = true;
            }
        }
    }

    let started = Instant::now();
    let deadline = started + Duration::from_millis(time_ms);
    let initial = score(&board, &distances, &board.initial());
    // A hill climb may shuffle across equal scores, so the cap is the
    // guard against circling, not a promise about route length.
    let cap = initial.saturating_mul(4).saturating_add(64).min(10_000);
    let mut push_decisions = 0u64;
    let mut best_seen = u64::MAX;
    let mut made = 0usize;
    let mut routed: Option<(String, u32, u32, usize)> = None;
    for attempt in 0..attempts {
        if routed.is_some() || Instant::now() >= deadline {
            break;
        }
        made = attempt + 1;
        let mut rng =
            Rng(0x9E37_79B9_7F4A_7C15 ^ (attempt as u64).wrapping_mul(0x2545_F491_4F6C_DD1D));
        let mut game = Game::new(board.clone());
        let mut pushes = 0u32;
        while !game.solved() && u64::from(pushes) < cap && Instant::now() < deadline {
            let state = game.state();
            let (visited, entered) = flood(&board, &state);
            let current = score(&board, &distances, &state);
            // Every legal push: the target free and live for the box's
            // label, the pusher's cell reached by the player.
            let mut best: Vec<(usize, usize)> = Vec::new();
            let mut best_score = u64::MAX;
            for index in 0..boxes {
                let cell = state.boxes[index];
                let label = (board.labels()[index] - b'A') as usize;
                for (direction, &opposite) in OPPOSITE.iter().enumerate() {
                    let target = board.neighbors()[cell as usize][direction];
                    let pusher = board.neighbors()[cell as usize][opposite];
                    if target == NONE
                        || pusher == NONE
                        || !visited[pusher as usize]
                        || state.boxes[..boxes].contains(&target)
                        || !live[label][target as usize]
                    {
                        continue;
                    }
                    let mut next = state;
                    next.boxes[index] = target;
                    let scored = score(&board, &distances, &next);
                    if scored == u64::MAX {
                        continue;
                    }
                    if scored < best_score {
                        best_score = scored;
                        best.clear();
                    }
                    if scored == best_score {
                        best.push((index, direction));
                    }
                }
            }
            if best.is_empty() || best_score > current {
                break;
            }
            best_seen = best_seen.min(best_score);
            push_decisions += 1;
            let (index, direction) = best[rng.below(best.len())];
            let behind = board.neighbors()[state.boxes[index] as usize][OPPOSITE[direction]];
            let path = walk(&board, &entered, state.player, behind);
            if game.check_extension(path.len() as u32 + 1).is_err() {
                break;
            }
            for step in path {
                if !game.step(step) {
                    return Err("The flood walked into a refused step".into());
                }
            }
            if !game.step(direction) {
                return Err("The chosen push was refused".into());
            }
            pushes += 1;
        }
        best_seen = best_seen.min(score(&board, &distances, &game.state()));
        if game.solved() {
            let route = game.actions().to_owned();
            let mut replay = Game::new(board.clone());
            replay.replay(&route)?;
            if !replay.solved() {
                return Err("The constructed route does not replay to a solve".into());
            }
            routed = Some((route, game.moves(), game.pushes(), attempt + 1));
        }
    }

    let ms = started.elapsed().as_millis() as u64;
    let record = if let Some((route, moves, pushes, at)) = routed {
        if let Some(path) = &out {
            std::fs::write(path, &route)?;
        }
        json!({
            "id": id,
            "fingerprint": board.fingerprint(),
            "boxes": boxes,
            "routed": true,
            "attempt": at,
            "moves": moves,
            "pushes": pushes,
            "push_decisions": push_decisions,
            "initial_score": initial,
            "ms": ms,
            "route_file": out,
        })
    } else {
        json!({
            "id": id,
            "fingerprint": board.fingerprint(),
            "boxes": boxes,
            "routed": false,
            "attempts": made,
            "push_decisions": push_decisions,
            "initial_score": initial,
            "best_score": if best_seen == u64::MAX { None } else { Some(best_seen) },
            "ms": ms,
        })
    };
    println!("{record}");
    Ok(())
}
