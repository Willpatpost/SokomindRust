use crate::api::{ApiJson, App, Error};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sokomind_core::Game;
use sokomind_search::{
    MAX_STATES_RANGE, MEMORY_MIB_RANGE, Mode, Proof, Search, SearchError, SearchStats, Status,
    StopReason,
};
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
/// Native solves take the search crate's state range as is and cap its
/// memory range lower, so every accepted request is a valid search limit.
const MEMORY_MIB: RangeInclusive<usize> = *MEMORY_MIB_RANGE.start()..=64;
const _: () = assert!(*MEMORY_MIB.end() <= *MEMORY_MIB_RANGE.end());

fn limits() -> Error {
    Error::bad(format!(
        "Server limits: {}..{} ms, {}..{} states, {}..{} MiB",
        TIME_MS.start(),
        TIME_MS.end(),
        MAX_STATES_RANGE.start(),
        MAX_STATES_RANGE.end(),
        MEMORY_MIB.start(),
        MEMORY_MIB.end()
    ))
}
/// Limits the caller chose are its error (400); an allocation the host
/// could not grant is 503. Requests pass [`limits`] and a replay first, so
/// `Limits` cannot occur here and `InvalidState` would be a server bug.
fn search_error(error: SearchError) -> Error {
    match error {
        SearchError::Limits => limits(),
        SearchError::BudgetTooSmall => Error::bad(error.to_string()),
        SearchError::Allocation(_) => {
            eprintln!("search allocation failed: {error}");
            Error::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Search allocation failed; lower the memory budget",
            )
        }
        SearchError::InvalidState(_) => Error::internal(error),
    }
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
    *MEMORY_MIB.end()
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
        || !MAX_STATES_RANGE.contains(&request.max_states)
        || !MEMORY_MIB.contains(&request.memory_mib)
    {
        return Err(limits());
    }
    let mode = Mode::parse(&request.mode).map_err(Error::bad)?;
    let permit = app
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::too_many("Solver busy; try the browser solver or retry later"))?;
    // The busy slot only stops concurrent solves; this stops one client
    // from taking every slot the moment it frees. Charged after the slot is
    // taken, so a busy answer does not spend the budget.
    if !app.solves.allow(app.proxies.client(&headers, peer.ip())) {
        return Err(Error::too_many(
            "Too many solve requests; try again shortly",
        ));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(cancel.clone());
    // CPU search never occupies an async Tokio worker; no unbounded job queue.
    let result = tokio::task::spawn_blocking(move || -> Result<ResultBody, Error> {
        let _permit = permit;
        let started = Instant::now();
        let deadline = Duration::from_millis(request.time_ms);
        let mut game = Game::at(&request.rows.join("\n"), &request.actions)
            .map_err(|error| Error::bad(error.to_string()))?;
        let initial_moves = game.moves();
        let initial_pushes = game.pushes();
        let mut search = Search::new(
            game.board().clone(),
            game.state(),
            mode,
            request.max_states,
            request.memory_mib,
        )
        .map_err(search_error)?;
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
            game.check_extension(moves)
                .map_err(|error| Error::bad(error.to_string()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use sokomind_core::StateError;

    #[test]
    fn bad_limits_are_400_and_failed_allocations_503() {
        let cases = [
            (SearchError::Limits, StatusCode::BAD_REQUEST),
            (SearchError::BudgetTooSmall, StatusCode::BAD_REQUEST),
            (
                SearchError::Allocation("node arena"),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                SearchError::InvalidState(StateError::PlayerOnWall { cell: 0 }),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (error, status) in cases {
            assert_eq!(search_error(error.clone()).0, status, "{error:?}");
        }
        // Out-of-range limits get the same message as the request check.
        assert_eq!(search_error(SearchError::Limits).1, limits().1);
    }

    #[test]
    fn request_limits_sit_inside_the_search_ranges() {
        assert_eq!(default_states(), *MAX_STATES_RANGE.end());
        assert!(MEMORY_MIB_RANGE.contains(MEMORY_MIB.start()));
        assert!(MEMORY_MIB_RANGE.contains(&default_memory()));
    }
}
