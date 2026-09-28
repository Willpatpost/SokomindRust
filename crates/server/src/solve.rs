use crate::api::{ApiJson, App, Error};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sokomind_core::Game;
use sokomind_search::{Mode, Proof, Search, SearchStats, Status, StopReason};
use std::{
    net::SocketAddr,
    ops::RangeInclusive,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const TIME_MS: RangeInclusive<u64> = 10..=30_000;
const MAX_STATES: RangeInclusive<usize> = 1..=sokomind_search::MAX_STATES;
const MEMORY_MIB: RangeInclusive<usize> = 4..=64;

fn limits() -> Error {
    Error::bad(format!(
        "Server limits: {}..{} ms, {}..{} states, {}..{} MiB",
        TIME_MS.start(),
        TIME_MS.end(),
        MAX_STATES.start(),
        MAX_STATES.end(),
        MEMORY_MIB.start(),
        MEMORY_MIB.end()
    ))
}

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
    sokomind_search::MAX_STATES
}
fn default_memory() -> usize {
    64
}
#[derive(Serialize)]
struct ProofBody {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    lower_bound: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upper_bound: Option<u32>,
}
fn proof_body(proof: Option<Proof>) -> Option<ProofBody> {
    let proof = proof?;
    let (lower_bound, upper_bound) = match proof {
        Proof::Bounded {
            lower_bound,
            upper_bound,
        } => (Some(lower_bound), Some(upper_bound)),
        Proof::Optimal { moves } => (Some(moves), Some(moves)),
        Proof::Unsolvable => (None, None),
    };
    Some(ProofBody {
        kind: proof.kind(),
        lower_bound,
        upper_bound,
    })
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
    proof: Option<ProofBody>,
    stats: serde_json::Value,
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub async fn solve(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<Request>,
) -> Result<Json<ResultBody>, Error> {
    if !TIME_MS.contains(&request.time_ms)
        || !MAX_STATES.contains(&request.max_states)
        || !MEMORY_MIB.contains(&request.memory_mib)
    {
        return Err(limits());
    }
    let mode = Mode::parse(&request.mode).map_err(Error::bad)?;
    // The busy slot only stops concurrent solves; this stops one client
    // from taking every slot the moment it frees.
    if !app.solves.allow(app.proxies.client(&headers, peer.ip())) {
        return Err(Error::too_many(
            "Too many solve requests; try again shortly",
        ));
    }
    let permit = app
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::too_many("Solver busy; try the browser solver or retry later"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(cancel.clone());
    // CPU search never occupies an async Tokio worker; no unbounded job queue.
    let result = tokio::task::spawn_blocking(move || -> Result<ResultBody, Error> {
        let _permit = permit;
        let started = Instant::now();
        let deadline = Duration::from_millis(request.time_ms);
        let mut game = Game::at(&request.rows.join("\n"), &request.actions).map_err(Error::bad)?;
        let initial_moves = game.moves();
        let initial_pushes = game.pushes();
        let mut search = Search::new(
            game.board().clone(),
            game.state(),
            mode,
            request.max_states,
            request.memory_mib,
        )
        .map_err(|_| {
            Error::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Search allocation failed; lower the memory budget",
            )
        })?;
        while search.status() == Status::Running {
            if cancel.load(Ordering::Relaxed) {
                search.stop(StopReason::Cancelled);
            } else if started.elapsed() >= deadline {
                search.stop(StopReason::TimeLimit);
            } else {
                search.advance(8);
            }
        }
        // Checked on the incumbent's length before reconstruction, which
        // would otherwise fail a route alone over the limit as a 500.
        if let Some(moves) = search.best_moves() {
            game.check_extension(moves).map_err(Error::bad)?;
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
                Some(game.pushes() - initial_pushes),
            )
        } else {
            (None, None)
        };
        let stats = search.stats();
        Ok(ResultBody {
            status: search.status().as_str(),
            route,
            moves,
            pushes,
            expanded: search.expanded(),
            generated: search.generated(),
            reserved_bytes: search.reserved_bytes(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            proof: proof_body(search.proof()),
            stats: SearchStats::FIELDS
                .into_iter()
                .zip(stats.values())
                .map(|(field, value)| (field.to_owned(), serde_json::Value::from(value)))
                .collect::<serde_json::Map<String, serde_json::Value>>()
                .into(),
        })
    })
    .await
    .map_err(Error::internal)??;
    Ok(Json(result))
}
