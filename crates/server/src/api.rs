//! State the handlers share, the `{error}` type every failure answers with,
//! the JSON and path extractors that keep that shape, and /api/health.
use crate::client::TrustedProxies;
use crate::config::Config;
use crate::database::{self, Interruption};
use crate::limit::RateLimiter;
use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Request, State},
    http::{HeaderMap, StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, de::DeserializeOwned};
use sokomind_core::Board;
use sqlx::PgPool;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Under the page's 1.5 s health timeout (web/src/progress.ts) even when the
/// database hangs. Part of the solve timeout chain; see TIME_MS in
/// crates/server/src/solve.rs.
const HEALTH_TIMEOUT: Duration = Duration::from_millis(900);
/// How long one database probe answers /api/health, so the database sees at
/// most one probe a second however often health is polled.
const HEALTH_TTL: Duration = Duration::from_secs(1);
/// Bounds slow request bodies on direct runs: bodies are at most 128 KiB, so
/// a client still sending after this long is holding a connection and a
/// handler open, not uploading. Behind nginx it never fires, because nginx
/// reads each whole body before passing the request on.
pub const BODY_TIMEOUT: Duration = Duration::from_secs(10);
/// Saves each client address may make per `RATE_WINDOW`.
pub const SAVES_PER_MINUTE: u32 = 60;
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// What every handler shares. Each request gets a clone, which shares the
/// slots, budgets and caches behind it rather than copying them.
#[derive(Clone)]
pub struct App {
    /// The bounded request pool; `None` when persistence is off.
    pub db: Option<PgPool>,
    /// The embedded catalog, keyed by puzzle id.
    pub catalog: Arc<HashMap<String, Board>>,
    /// One permit per native solve that may run at once.
    pub solve_slots: Arc<Semaphore>,
    /// One permit per progress read or save that may run at once.
    pub progress_slots: Arc<Semaphore>,
    /// The peers trusted to name, through X-Forwarded-For, the client a
    /// budget charges.
    pub proxies: Arc<TrustedProxies>,
    /// Each client's save budget, [`SAVES_PER_MINUTE`].
    pub saves: Arc<RateLimiter>,
    /// Each client's native solve budget, `SOLVE_RATE_PER_MINUTE`.
    pub solves: Arc<RateLimiter>,
    /// The cached database probe behind /api/health.
    pub health: Arc<Health>,
}
impl App {
    /// The state every handler shares: the budgets `config` sets, the parsed
    /// catalog, and the request pool when persistence is on.
    pub fn new(config: Config, catalog: HashMap<String, Board>, db: Option<PgPool>) -> Self {
        Self {
            db,
            catalog: Arc::new(catalog),
            solve_slots: Arc::new(Semaphore::new(config.solve_concurrency as usize)),
            progress_slots: Arc::new(Semaphore::new(config.progress_concurrency as usize)),
            proxies: Arc::new(config.proxies),
            saves: Arc::new(RateLimiter::new(SAVES_PER_MINUTE, RATE_WINDOW)),
            solves: Arc::new(RateLimiter::new(config.solve_rate, RATE_WINDOW)),
            health: Arc::default(),
        }
    }
    /// A solve slot, held until the permit drops; see [`permit`].
    pub fn solve_permit(&self) -> Result<OwnedSemaphorePermit, Error> {
        permit(
            &self.solve_slots,
            "Solver busy; try the browser solver or retry later",
        )
    }
    /// A progress slot, held until the permit drops; see [`permit`].
    pub fn progress_permit(&self) -> Result<OwnedSemaphorePermit, Error> {
        permit(&self.progress_slots, "Progress busy; retry later")
    }
    /// Spends one request of `budget` for the client that `headers` and
    /// `peer` name, or answers 429 with `refused` when that client has none
    /// left. Handlers charge last, so a request refused for any other reason
    /// spends nothing.
    pub fn charge(
        &self,
        budget: &RateLimiter,
        headers: &HeaderMap,
        peer: SocketAddr,
        refused: &str,
    ) -> Result<(), Error> {
        if budget.allow(self.proxies.client(headers, peer.ip())) {
            Ok(())
        } else {
            Err(Error::too_many(refused))
        }
    }
    /// The request pool, or 503 when persistence is off.
    pub fn db(&self) -> Result<&PgPool, Error> {
        self.db.as_ref().ok_or_else(Error::unavailable)
    }
    /// The catalog's board for `id`, or 404.
    pub fn puzzle(&self, id: &str) -> Result<&Board, Error> {
        self.catalog
            .get(id)
            .ok_or_else(|| Error::new(StatusCode::NOT_FOUND, "Unknown catalog puzzle"))
    }
}
/// A free slot of `slots`, or 429 with `busy` at once when none is free:
/// nothing queues, so a busy server answers without holding the request.
fn permit(slots: &Arc<Semaphore>, busy: &str) -> Result<OwnedSemaphorePermit, Error> {
    Arc::clone(slots)
        .try_acquire_owned()
        .map_err(|_| Error::too_many(busy))
}
/// The fields of a `data/puzzles.json` entry the server needs; serde
/// ignores the rest.
#[derive(Deserialize)]
struct Puzzle {
    id: String,
    rows: Vec<String>,
}
/// Parses the embedded catalog once, keyed by puzzle id.
pub fn load_catalog() -> Result<HashMap<String, Board>, String> {
    let puzzles: Vec<Puzzle> = serde_json::from_str(include_str!("../../../data/puzzles.json"))
        .map_err(|error| format!("puzzle catalog: {error}"))?;
    let mut catalog = HashMap::with_capacity(puzzles.len());
    for Puzzle { id, rows } in puzzles {
        let board =
            Board::parse(&rows.join("\n")).map_err(|error| format!("puzzle {id}: {error}"))?;
        if catalog.insert(id.clone(), board).is_some() {
            return Err(format!("puzzle {id} appears twice in the catalog"));
        }
    }
    Ok(catalog)
}
/// JSON body extractor that keeps the API's `{error}` response shape for
/// malformed payloads instead of axum's plain-text rejections.
pub struct ApiJson<T>(pub T);
impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Error;
    async fn from_request(request: Request, state: &S) -> Result<Self, Error> {
        match tokio::time::timeout(BODY_TIMEOUT, Json::<T>::from_request(request, state)).await {
            Ok(Ok(Json(value))) => Ok(ApiJson(value)),
            Ok(Err(rejection)) => Err(Error::new(rejection.status(), rejection.body_text())),
            Err(_) => Err(Error::new(
                StatusCode::REQUEST_TIMEOUT,
                "Request body timed out",
            )),
        }
    }
}
/// Path extractor with the same `{error}` rejections.
pub struct ApiPath<T>(pub T);
impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Error> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(ApiPath(value)),
            Err(rejection) => Err(Error::new(rejection.status(), rejection.body_text())),
        }
    }
}
/// A failed request: its status and the message its `{error}` body carries.
#[derive(Debug)]
pub struct Error {
    /// The response status.
    pub status: StatusCode,
    /// The body's `error`, written for the person using the page.
    pub message: String,
}
impl Error {
    /// `status` with `message` as its `error`.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    /// 400: the request itself is invalid.
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    /// 429: every slot is busy, or the client's budget is spent.
    pub fn too_many(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message)
    }
    /// 404 for a path no route matches.
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "Unknown API endpoint")
    }
    /// 503: persistence is off, or the database cannot be reached.
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "PostgreSQL persistence is unavailable",
        )
    }
    /// 503: a database operation was stopped and may succeed on a retry.
    pub fn interrupted(interruption: Interruption) -> Self {
        let message = match interruption {
            Interruption::TimedOut => "PostgreSQL operation timed out; retry later",
            Interruption::Conflict => {
                "PostgreSQL operation conflicted with a concurrent one; retry later"
            }
        };
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }
    /// Interrupted operations and an unreachable database are retryable
    /// 503s, while malformed SQL and constraint errors remain internal
    /// failures.
    pub fn database(error: sqlx::Error) -> Self {
        if let Some(interruption) = database::interrupted(&error) {
            eprintln!("database operation interrupted: {error}");
            Self::interrupted(interruption)
        } else if database::unreachable(&error) {
            eprintln!("database unavailable: {error}");
            Self::unavailable()
        } else {
            Self::internal(error)
        }
    }
    /// 500 for a server bug. `message` goes to stderr, never to the client,
    /// which gets a fixed message.
    pub fn internal(message: impl std::fmt::Display) -> Self {
        eprintln!("request failed: {message}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Server operation failed")
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}
/// `GET /api/health`: always 200 `{"status": "ok", "persistence": bool}`.
/// `persistence` is false when persistence is off, and otherwise the cached
/// probe answer [`Health`] keeps.
pub async fn health(State(app): State<App>) -> Json<serde_json::Value> {
    let persistence = match &app.db {
        Some(db) => app.health.persistence(db).await,
        None => false,
    };
    Json(serde_json::json!({ "status": "ok", "persistence": persistence }))
}

/// /api/health's cached answer to whether PostgreSQL responds. Probes take no
/// progress permit, so busy saves no longer read as persistence being off,
/// and they run at most once per `HEALTH_TTL` whatever the request rate.
#[derive(Default)]
pub struct Health {
    state: Mutex<Probe>,
}
#[derive(Clone, Copy, Default)]
struct Probe {
    /// When the last probe finished, and whether the database answered.
    last: Option<(Instant, bool)>,
    /// A probe is in flight; other callers answer from `last` meanwhile.
    running: bool,
}
impl Health {
    /// Whether `db` answered `SELECT 1` on a pool connection within
    /// `HEALTH_TIMEOUT`, as of the last probe.
    pub async fn persistence(self: &Arc<Self>, db: &PgPool) -> bool {
        self.check(|| {
            let db = db.clone();
            // A slow probe is indistinguishable from a dead one to the frontend.
            async move {
                let probe = sqlx::query("SELECT 1").execute(&db);
                matches!(tokio::time::timeout(HEALTH_TIMEOUT, probe).await, Ok(Ok(_)))
            }
        })
        .await
    }
    /// Answers from the cache while it is fresh. Otherwise one caller runs
    /// `probe` and waits for it, and every concurrent caller answers at once
    /// from the previous probe (false before the first one lands) instead of
    /// waiting too. The probe runs in its own task, so it finishes and is
    /// recorded even if the request that started it is dropped.
    async fn check<F>(self: &Arc<Self>, probe: impl FnOnce() -> F) -> bool
    where
        F: Future<Output = bool> + Send + 'static,
    {
        if let Some(answer) = self.cached(Instant::now()) {
            return answer;
        }
        let health = Arc::clone(self);
        let probe = probe();
        tokio::spawn(async move {
            let mut refresh = Refresh { health, up: None };
            refresh.finish(probe.await)
        })
        .await
        .unwrap_or(false)
    }
    /// The answer to give without probing, or `None` when the caller must
    /// probe: it then owns the refresh until [`Health::record`].
    fn cached(&self, now: Instant) -> Option<bool> {
        let mut state = self.lock();
        let Probe { last, running } = *state;
        match last {
            Some((at, up)) if now.saturating_duration_since(at) < HEALTH_TTL => Some(up),
            _ if running => Some(last.is_some_and(|(_, up)| up)),
            _ => {
                state.running = true;
                None
            }
        }
    }
    /// Ends the refresh; `None` (the probe task unwound) keeps the previous
    /// answer and lets the next caller probe again.
    fn record(&self, now: Instant, up: Option<bool>) {
        let mut state = self.lock();
        state.running = false;
        if let Some(up) = up {
            state.last = Some((now, up));
        }
    }
    /// Nothing panics while holding the lock, but a poisoned cache must
    /// never take health down with it.
    fn lock(&self) -> MutexGuard<'_, Probe> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
/// Records a probe when its task ends, even by unwinding, so a failed probe
/// cannot leave `running` set and freeze the cached answer.
struct Refresh {
    health: Arc<Health>,
    up: Option<bool>,
}
impl Refresh {
    /// Sets the answer that dropping `self` records.
    fn finish(&mut self, up: bool) -> bool {
        self.up = Some(up);
        up
    }
}
impl Drop for Refresh {
    fn drop(&mut self) {
        self.health.record(Instant::now(), self.up);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[derive(Deserialize)]
    struct Shape {
        route: String,
    }

    async fn extract(content_type: Option<&str>, body: &'static str) -> Result<Shape, StatusCode> {
        let mut request = axum::http::Request::builder().method("POST").uri("/");
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        let request = request.body(Body::from(body)).unwrap();
        match ApiJson::<Shape>::from_request(request, &()).await {
            Ok(ApiJson(shape)) => Ok(shape),
            Err(error) => Err(error.status),
        }
    }

    #[tokio::test]
    async fn json_rejections_keep_their_status() {
        let json = Some("application/json");
        assert_eq!(
            extract(json, r#"{"route":"UD"}"#).await.unwrap().route,
            "UD"
        );
        let unsupported = Some(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(extract(None, r#"{"route":"UD"}"#).await.err(), unsupported);
        assert_eq!(
            extract(Some("text/plain"), r#"{"route":"UD"}"#).await.err(),
            unsupported
        );
        assert_eq!(
            extract(json, "{").await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        let unprocessable = Some(StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(extract(json, r#"{"route":1}"#).await.err(), unprocessable);
        assert_eq!(extract(json, "{}").await.err(), unprocessable);
    }

    #[test]
    fn every_embedded_puzzle_parses() {
        let catalog = load_catalog().unwrap();
        assert!(!catalog.is_empty());
        assert!(
            catalog
                .values()
                .all(|board| !board.fingerprint().is_empty())
        );
    }

    #[test]
    fn health_answers_from_cache_between_probes() {
        let health = Health::default();
        let start = Instant::now();
        // The first caller probes; the others answer at once, false until a
        // probe lands.
        assert_eq!(health.cached(start), None);
        assert_eq!(health.cached(start), Some(false));
        health.record(start, Some(true));
        assert_eq!(health.cached(start + HEALTH_TTL / 2), Some(true));
        // A stale answer is refreshed by one caller while the rest reuse it.
        let stale = start + HEALTH_TTL;
        assert_eq!(health.cached(stale), None);
        assert_eq!(health.cached(stale), Some(true));
        health.record(stale, Some(false));
        assert_eq!(health.cached(stale + HEALTH_TTL / 2), Some(false));
        // A probe that unwound records nothing, and the next caller retries.
        let later = stale + HEALTH_TTL;
        assert_eq!(health.cached(later), None);
        health.record(later, None);
        assert_eq!(health.cached(later), None);
    }

    async fn until(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !condition() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn health_probes_once_and_callers_never_wait_for_it() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let health = Arc::new(Health::default());
        let (answer, answered) = tokio::sync::oneshot::channel();
        let first = tokio::spawn({
            let health = Arc::clone(&health);
            async move {
                health
                    .check(|| async move { answered.await.unwrap_or(false) })
                    .await
            }
        });
        until(|| health.lock().running).await;
        let probes = AtomicUsize::new(0);
        let counted = || {
            probes.fetch_add(1, Ordering::Relaxed);
            std::future::ready(true)
        };
        // While the first probe runs, callers answer at once, without probing.
        let busy = tokio::time::timeout(Duration::from_millis(500), health.check(&counted));
        assert!(!busy.await.unwrap());
        // Dropping the request that started the probe does not drop the probe.
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        answer.send(true).unwrap();
        until(|| !health.lock().running).await;
        // Its answer is reused while fresh.
        assert!(health.check(&counted).await);
        assert_eq!(probes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn unreachable_databases_are_503_and_failed_queries_500() {
        let closed = Error::database(sqlx::Error::PoolClosed);
        assert_eq!(closed.status, StatusCode::SERVICE_UNAVAILABLE);
        let failed = Error::database(sqlx::Error::RowNotFound);
        assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
