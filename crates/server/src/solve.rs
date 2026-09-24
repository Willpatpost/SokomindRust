use crate::api::{App, Error};
use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use sokomind_core::{Board, Game};
use sokomind_search::{Mode, Search, Status};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    rows: Vec<String>,
    #[serde(default)]
    actions: String,
    mode: String,
    #[serde(default = "default_ms")]
    time_ms: u64,
    #[serde(default = "default_states")]
    max_states: usize,
    #[serde(default = "default_memory")]
    memory_mib: usize,
}
fn default_ms() -> u64 {
    5000
}
fn default_states() -> usize {
    200_000
}
fn default_memory() -> usize {
    64
}
#[derive(Serialize)]
pub struct ResultBody {
    status: &'static str,
    route: Option<String>,
    moves: Option<u32>,
    pushes: Option<u32>,
    expanded: u32,
    generated: u32,
    reserved_bytes: usize,
    elapsed_ms: u64,
    proven: bool,
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub async fn solve(
    State(app): State<App>,
    Json(request): Json<Request>,
) -> Result<Json<ResultBody>, Error> {
    if !(10..=30_000).contains(&request.time_ms)
        || !(1..=500_000).contains(&request.max_states)
        || !(4..=64).contains(&request.memory_mib)
    {
        return Err(Error::bad(
            "Server limits: 10..30000 ms, 1..500000 states, 4..64 MiB",
        ));
    }
    let mode = Mode::parse(&request.mode).map_err(Error::bad)?;
    let permit = app.slots.clone().try_acquire_owned().map_err(|_| {
        Error(
            StatusCode::TOO_MANY_REQUESTS,
            "Solver busy; try the browser solver or retry later".into(),
        )
    })?;
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(cancel.clone());
    // CPU search never occupies an async Tokio worker; no unbounded job queue.
    let result = tokio::task::spawn_blocking(move || -> Result<ResultBody, Error> {
        let _permit = permit;
        let started = Instant::now();
        let deadline = Duration::from_millis(request.time_ms);
        let mut game = Game::new(Board::parse(&request.rows.join("\n")).map_err(Error::bad)?);
        game.replay(&request.actions).map_err(Error::bad)?;
        let initial_moves = game.moves();
        let initial_pushes = game.pushes;
        let mut search = Search::new(
            game.board.clone(),
            game.state,
            mode,
            request.max_states,
            request.memory_mib,
        )
        .map_err(Error::bad)?;
        while search.status == Status::Running {
            if cancel.load(Ordering::Relaxed) {
                search.stop(Status::Cancelled);
            } else if started.elapsed() >= deadline {
                search.stop(Status::TimeLimit);
            } else {
                search.advance(8);
            }
        }
        let route = search.solution().map_err(Error::internal)?;
        let (moves, pushes) = if let Some(route) = &route {
            game.replay(&(request.actions + route))
                .map_err(Error::internal)?;
            if !game.solved() {
                return Err(Error::internal("Search returned an unsolved route"));
            }
            (
                Some(game.moves() - initial_moves),
                Some(game.pushes - initial_pushes),
            )
        } else {
            (None, None)
        };
        Ok(ResultBody {
            status: search.status.as_str(),
            route,
            moves,
            pushes,
            expanded: search.expanded,
            generated: search.generated,
            reserved_bytes: search.reserved_bytes,
            elapsed_ms: started.elapsed().as_millis() as u64,
            proven: search.proven,
        })
    })
    .await
    .map_err(Error::internal)??;
    Ok(Json(result))
}
