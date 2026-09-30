//! `POST /api/solve`: one native search, bounded in time, states and memory.
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

/// A native solve's time budget. Its upper end heads the solve timeout
/// chain, whose links change together and must keep 30 s (this range's top)
/// < 35 s (compose grace, and the page's 30 s + 5 s wait) < 40 s (nginx):
/// - web/src/solver-client.ts: the page gives up on a native solve at
///   `time_ms` + 5 s, 35 s at most, which leaves the server time to finish
///   and send its answer after the deadline.
/// - compose.yaml: `stop_grace_period: 35s` lets a solve that is running at
///   shutdown finish, since graceful shutdown waits for in-flight requests.
/// - deploy/nginx.conf: `proxy_read_timeout 40s` outlasts the page's wait,
///   so nginx never cuts off an answer the page still expects.
///
/// Health follows the same rule: HEALTH_TIMEOUT, 900 ms in
/// crates/server/src/api.rs, stays under the page's 1.5 s HEALTH_TIMEOUT_MS
/// in web/src/progress.ts.
const TIME_MS: RangeInclusive<u64> = 10..=30_000;
/// Pops the search makes between cancel and deadline checks: a stop waits
/// for at most this many pops, and one atomic load and clock read per batch
/// cost little beside them.
const POPS_PER_CHECK: u32 = 8;
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

/// A solve request's JSON body. Unknown fields are rejected, so a typo
/// cannot silently fall back to a default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// The puzzle's rows, in the catalog's text format.
    rows: Vec<String>,
    /// Moves already played from `rows`; the search starts after them.
    #[serde(default)]
    actions: String,
    /// The search mode, as [`Mode::parse`] reads it.
    mode: String,
    /// The time budget in milliseconds, within [`TIME_MS`]; 5000 by default.
    #[serde(default = "default_ms")]
    time_ms: u64,
    /// The state cap, within [`MAX_STATES_RANGE`]; its top by default.
    #[serde(default = "default_states")]
    max_states: usize,
    /// The memory budget in MiB, within [`MEMORY_MIB`]; its top by default.
    #[serde(default = "default_memory")]
    memory_mib: usize,
}
fn default_ms() -> u64 {
    5000
}
/// The search crate's [`MAX_STATES`](sokomind_search::MAX_STATES), which no
/// [`MEMORY_MIB`] budget reaches, so by default `memory_mib` alone sizes a
/// solve. The arena reserves `min(max_states, what fits in memory_mib)`
/// eagerly, when the search starts, so a default solve reserves as many
/// states as its memory budget holds, even on a small board. That is by
/// design: `memory_mib` is the declared bound, and the server's search
/// memory stays within SOLVE_CONCURRENCY times [`MEMORY_MIB`]'s top.
fn default_states() -> usize {
    sokomind_search::MAX_STATES
}
fn default_memory() -> usize {
    *MEMORY_MIB.end()
}
/// What the search proved about the shortest route, in moves.
#[derive(Serialize)]
struct ProofBody {
    /// [`Proof::kind`]: optimal, bounded or unsolvable.
    kind: &'static str,
    /// No route is shorter than this; omitted for an unsolvable puzzle.
    #[serde(skip_serializing_if = "Option::is_none")]
    lower_bound: Option<u32>,
    /// The route's move count; omitted for an unsolvable puzzle.
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
/// A finished search's JSON answer, whether or not it found a route.
#[derive(Serialize)]
pub struct ResultBody {
    /// Why the search ended, from [`Status::as_str`].
    status: &'static str,
    /// The moves found after `actions`, or `null`.
    route: Option<String>,
    /// The route's moves, or `null` without a route.
    moves: Option<u32>,
    /// The route's pushes, or `null` without a route.
    pushes: Option<u32>,
    /// Records the search expanded, each counted once.
    expanded: u32,
    /// Records the search inserted, improved versions of known states
    /// included.
    generated: u32,
    /// Bytes charged against `memory_mib`, all reserved up front.
    reserved_bytes: usize,
    /// Wall-clock time the search took, in milliseconds.
    elapsed_ms: u64,
    /// What the search proved, or `null` when it proved nothing.
    proof: Option<ProofBody>,
    /// Every [`SearchStats`] counter by name, as integers.
    stats: serde_json::Value,
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// `POST /api/solve`: runs one native search on a blocking thread within
/// the request's limits and answers with the [`ResultBody`]. A dropped
/// request cancels its search.
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
    let mode = Mode::parse(&request.mode).map_err(|error| Error::bad(error.to_string()))?;
    let permit = app.solve_permit()?;
    // The busy slot only stops concurrent solves; this stops one client
    // from taking every slot the moment it frees. Charged after the slot is
    // taken, so a busy answer does not spend the budget. The page shows the
    // refusal as sent (nativeErrorText in web/src/transport.ts), so, like
    // the busy text, it names the browser solver.
    app.charge(
        &app.solves,
        &headers,
        peer,
        "Too many solve requests; try the browser solver or try again shortly",
    )?;
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
                search.advance(POPS_PER_CHECK);
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
            assert_eq!(search_error(error.clone()).status, status, "{error:?}");
        }
        // Out-of-range limits get the same message as the request check.
        assert_eq!(search_error(SearchError::Limits).message, limits().message);
    }

    #[test]
    fn request_limits_sit_inside_the_search_ranges() {
        assert_eq!(default_states(), *MAX_STATES_RANGE.end());
        assert!(MEMORY_MIB_RANGE.contains(MEMORY_MIB.start()));
        assert!(MEMORY_MIB_RANGE.contains(&default_memory()));
    }
}
