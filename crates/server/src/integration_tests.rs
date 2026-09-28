use super::*;
use crate::api::App;
use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
use serde_json::{Value, json};
use sqlx::{
    Connection, PgConnection, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    env,
    net::IpAddr,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{RwLock, Semaphore},
    task::JoinSet,
};
use tower::ServiceExt;

const PROFILE: &str = "0123456789abcdef0123456789abcdef";
const CLIENT: [u8; 4] = [127, 0, 0, 1];
/// Progress for the catalog's `ultra-tiny`, which `D` solves in one move and
/// one push, and `LRD` in three moves and one push.
const PROGRESS: &str = "/api/progress/ultra-tiny";

/// The App main builds from an empty environment after `adjust`, with `db`
/// as its pool.
fn app_with(db: Option<PgPool>, adjust: impl FnOnce(&mut Config)) -> App {
    let mut config = Config::defaults();
    adjust(&mut config);
    App::new(config, api::load_catalog().unwrap(), db)
}

fn app_state(db: Option<PgPool>) -> App {
    app_with(db, |_| {})
}

fn test_router(state: App) -> Router {
    router(state).layer(MockConnectInfo(SocketAddr::from((CLIENT, 0))))
}

/// A pool whose every connection attempt is refused; like the server's pool,
/// it stops trying to acquire after `database::ACQUIRE_TIMEOUT`.
fn refused_pool() -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(database::ACQUIRE_TIMEOUT)
        .connect_lazy("postgres://invalid@127.0.0.1:1/invalid")
        .unwrap()
}

/// The two ways a configured database is out of reach, by name: a refused
/// pool fails when its acquire timeout runs out, a closed one at once.
async fn unreachable_pools() -> [(&'static str, PgPool); 2] {
    let closed = refused_pool();
    closed.close().await;
    [("refused", refused_pool()), ("closed", closed)]
}

async fn request(app: Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    request_as(app, PROFILE, method, path, body).await
}

async fn request_as(
    app: Router,
    profile: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("x-profile-id", profile)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// /api/health's `persistence`, after checking that health answered 200 with
/// nothing else in its body.
async fn persistence(app: Router) -> bool {
    let (status, health) = request(app, "GET", "/api/health", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let persistence = health["persistence"].as_bool().unwrap();
    assert_eq!(
        health,
        json!({ "status": "ok", "persistence": persistence })
    );
    persistence
}

/// Waits, for at most a second, until exactly `permits` of `slots` are free.
async fn until_available(slots: &Semaphore, permits: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while slots.available_permits() != permits {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{permits} permits never became free"));
}

fn solve_body() -> Value {
    json!({"rows": ["OOOOO", "O R O", "O A O", "O a O", "OOOOO"],
        "mode":"optimal", "time_ms":1000, "max_states":1000, "memory_mib":4})
}

#[tokio::test]
async fn router_solves_valid_invalid_and_busy_requests() {
    let state = app_state(None);
    let app = test_router(state.clone());
    let mut invalid = solve_body();
    invalid["time_ms"] = json!(0);
    assert_eq!(
        request(app.clone(), "POST", "/api/solve", invalid).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut invalid = solve_body();
    invalid["mode"] = json!("magic");
    assert_eq!(
        request(app.clone(), "POST", "/api/solve", invalid).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut invalid = solve_body();
    invalid["actions"] = json!("U"); // a wall above the player
    assert_eq!(
        request(app.clone(), "POST", "/api/solve", invalid).await.0,
        StatusCode::BAD_REQUEST
    );
    let permit = state.slots.clone().acquire_owned().await.unwrap();
    let (status, body) = request(app.clone(), "POST", "/api/solve", solve_body()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(body["error"].as_str().unwrap().contains("Solver busy"));
    drop(permit);
    let (status, body) = request(app, "POST", "/api/solve", solve_body()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["route"], "D");
    assert_eq!(body["moves"], 1);
    assert_eq!(
        body["proof"],
        json!({"kind": "optimal", "lower_bound": 1, "upper_bound": 1})
    );
    assert!(body["stats"]["unique_states"].as_u64().unwrap() >= 1);
    // The stats object carries exactly the shared counter names, all integers.
    let stats = body["stats"].as_object().unwrap();
    let mut keys: Vec<&str> = stats.keys().map(String::as_str).collect();
    let mut fields = sokomind_search::SearchStats::FIELDS;
    keys.sort_unstable();
    fields.sort_unstable();
    assert_eq!(keys, fields);
    assert!(stats.values().all(Value::is_u64));
    assert_eq!(state.slots.available_permits(), 1);
}

#[tokio::test]
async fn solve_limits_follow_the_state_cap() {
    let state = app_state(None);
    let app = test_router(state.clone());
    let mut over = solve_body();
    over["max_states"] = json!(sokomind_search::MAX_STATES + 1);
    let (status, body) = request(app.clone(), "POST", "/api/solve", over).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("1..1000000 states"), "{error}");
    // At the cap, the 4 MiB budget sizes the arena down instead of failing.
    let mut at_cap = solve_body();
    at_cap["max_states"] = json!(sokomind_search::MAX_STATES);
    let (status, body) = request(app, "POST", "/api/solve", at_cap).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["route"], "D");
    assert_eq!(body["moves"], 1);
    assert_eq!(state.slots.available_permits(), 1);
}

#[tokio::test]
async fn busy_solves_keep_the_rate_budget() {
    let state = app_with(None, |config| config.solve_rate = 1);
    let app = test_router(state.clone());
    let permit = state.slots.clone().acquire_owned().await.unwrap();
    for _ in 0..3 {
        let (status, body) = request(app.clone(), "POST", "/api/solve", solve_body()).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(body["error"].as_str().unwrap().contains("Solver busy"));
    }
    drop(permit);
    // The busy answers spent nothing, so the one solve a minute still runs.
    let (status, body) = request(app.clone(), "POST", "/api/solve", solve_body()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["route"], "D");
    let (status, body) = request(app, "POST", "/api/solve", solve_body()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(body["error"].as_str().unwrap().contains("Too many solve"));
}

#[tokio::test]
async fn progress_reads_and_saves_check_in_the_same_order() {
    let save = json!({"route":"D"});
    let unknown = "/api/progress/no-such-puzzle";
    let bad_profile = "not-a-profile";
    // Without persistence: the request, then the puzzle, then the database.
    let offline = test_router(app_state(None));
    for (method, body) in [("GET", Value::Null), ("POST", save.clone())] {
        let cases = [
            (bad_profile, unknown, StatusCode::BAD_REQUEST),
            (PROFILE, unknown, StatusCode::NOT_FOUND),
            (PROFILE, PROGRESS, StatusCode::SERVICE_UNAVAILABLE),
        ];
        for (profile, path, status) in cases {
            let (got, _) = request_as(offline.clone(), profile, method, path, body.clone()).await;
            assert_eq!(got, status, "{method} {path} as {profile}");
        }
    }
    // With every progress slot taken, both answer busy at once and a busy
    // save spends none of the client's rate budget.
    let state = app_with(Some(refused_pool()), |config| {
        config.progress_concurrency = 1;
    });
    let app = test_router(state.clone());
    let permit = state.progress_slots.clone().try_acquire_owned().unwrap();
    let (status, body) = request(app.clone(), "GET", PROGRESS, Value::Null).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "Progress busy; retry later");
    for _ in 0..=api::SAVES_PER_MINUTE {
        let (status, body) = request(app.clone(), "POST", PROGRESS, save.clone()).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["error"], "Progress busy; retry later");
    }
    drop(permit);
    assert!(state.saves.allow(IpAddr::from(CLIENT)));
    assert_eq!(state.progress_slots.available_permits(), 1);
}

#[test]
fn aborted_save_retains_admission_until_queued_replay_finishes() {
    // Occupy the runtime's sole blocking thread, so the real save handler's
    // replay is queued when its HTTP future is cancelled.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (release, wait) = std::sync::mpsc::channel();
        let (entered, entry) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            wait.recv().unwrap();
        });
        entry.await.unwrap();
        // The save is cancelled before it reaches the database.
        let state = app_with(Some(refused_pool()), |config| {
            config.progress_concurrency = 1;
        });
        let saving = tokio::spawn(request(
            test_router(state.clone()),
            "POST",
            PROGRESS,
            json!({"route":"D"}),
        ));
        until_available(&state.progress_slots, 0).await;
        saving.abort();
        assert!(saving.await.unwrap_err().is_cancelled());
        assert_eq!(state.progress_slots.available_permits(), 0);
        release.send(()).unwrap();
        blocker.await.unwrap();
        until_available(&state.progress_slots, 1).await;
    });
}

#[tokio::test]
async fn health_reports_unreachable_databases_quickly_and_caches_the_answer() {
    for (name, db) in unreachable_pools().await {
        let app = test_router(app_state(Some(db)));
        // sqlx keeps retrying a refused connection until its acquire
        // timeout; the probe's own deadline answers inside the frontend's
        // 1.5 s health timeout.
        let started = Instant::now();
        assert!(!persistence(app.clone()).await, "{name}");
        assert!(started.elapsed() < Duration::from_millis(1500), "{name}");
        // The failure is cached, so an immediate repeat does not probe again.
        let started = Instant::now();
        assert!(!persistence(app).await, "{name}");
        assert!(started.elapsed() < Duration::from_millis(200), "{name}");
    }
}

/// A configured database that cannot be reached gives reads and saves the
/// same stable 503, in time, and each request returns its progress slot.
#[tokio::test]
async fn unreachable_databases_fail_progress_requests_and_release_admission() {
    for (name, db) in unreachable_pools().await {
        let state = app_state(Some(db));
        let app = test_router(state.clone());
        let slots = state.progress_slots.available_permits();
        for (method, body) in [("GET", Value::Null), ("POST", json!({"route":"D"}))] {
            let started = Instant::now();
            let (status, error) = request(app.clone(), method, PROGRESS, body).await;
            let case = format!("{method} with a {name} pool");
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{case}");
            assert_eq!(
                error,
                json!({ "error": "PostgreSQL persistence is unavailable" }),
                "{case}"
            );
            assert!(started.elapsed() < Duration::from_secs(3), "{case}");
            assert_eq!(state.progress_slots.available_permits(), slots, "{case}");
        }
    }
}

// Live PostgreSQL tests. They are ignored, so an ordinary test run never
// connects to a user's database; `npm run test:db` selects them by the
// `live_postgres` name prefix with `--ignored`. SOKOMIND_TEST_DATABASE_URL
// must name a dedicated, disposable database that allows CREATE SCHEMA.
//
// Each test runs in a schema of its own (see `live`), migrated as at startup
// and dropped afterwards, and writes every row it reads, so the tests share
// no rows and run in parallel against one database; no public table is
// touched. The one state they share is sqlx's migration advisory lock, which
// is per database, not per schema: `MIGRATING` keeps the test that holds it
// on purpose apart from the other tests' migrations.

/// Every test's migration step shares this; the migration test takes it
/// exclusively for its whole run.
static MIGRATING: RwLock<()> = RwLock::const_new(());
/// Tells apart the schemas of tests that start at the same instant.
static SCHEMAS: AtomicUsize = AtomicUsize::new(0);

/// A well-formed layout fingerprint that `ultra-tiny` does not have, for rows
/// written straight to the table.
const OTHER_LAYOUT: &str = "puzzle-v1:00000000";

/// A progress row's `(moves, pushes, route)`.
type Row = (i32, i32, &'static str);

/// Runs `checks` in a new schema of the dedicated test database, migrated
/// the way startup migrates, and drops the schema afterwards, also when a
/// check panics. `checks` gets the bounded request pool and the options it
/// is built from, which reach the schema without the request limits.
async fn live<F, Fut>(checks: F)
where
    F: FnOnce(PgPool, PgConnectOptions) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let url = env::var("SOKOMIND_TEST_DATABASE_URL")
        .expect("set SOKOMIND_TEST_DATABASE_URL to a dedicated disposable PostgreSQL database");
    let options: PgConnectOptions = url.parse().unwrap();
    let mut admin = PgConnection::connect_with(&database::bounded_options(options.clone()))
        .await
        .unwrap();
    let schema = format!(
        "sokomind_test_{}_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SCHEMAS.fetch_add(1, Ordering::Relaxed)
    );
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&mut admin)
        .await
        .unwrap();
    let scoped = options.options([("search_path", schema.as_str())]);
    let pool_result = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(database::ACQUIRE_TIMEOUT)
        .connect_with(database::bounded_options(scoped.clone()))
        .await;
    let pool = match pool_result {
        Ok(pool) => pool,
        Err(error) => {
            drop_schema(&mut admin, &schema).await;
            panic!("connect to isolated test schema: {error}");
        }
    };
    let checks = checks(pool.clone(), scoped.clone());
    // A panic ends only this task, so the schema is dropped either way.
    let result = tokio::spawn(async move {
        {
            let _shared = MIGRATING.read().await;
            database::run_migrations(&scoped).await.unwrap();
        }
        checks.await;
    })
    .await;
    pool.close().await;
    drop_schema(&mut admin, &schema).await;
    if let Err(error) = result {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("live database test task was cancelled");
    }
}

async fn drop_schema(admin: &mut PgConnection, schema: &str) {
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(admin)
        .await
        .unwrap();
}

/// Writes `row` for PROFILE through the save handler's conditional UPSERT,
/// with its counters taken as given rather than replayed.
async fn upsert(db: &PgPool, puzzle: &str, fingerprint: &str, (moves, pushes, route): Row) {
    sqlx::query(progress::UPSERT)
        .bind(PROFILE)
        .bind(puzzle)
        .bind(fingerprint)
        .bind(moves)
        .bind(pushes)
        .bind(route)
        .execute(db)
        .await
        .unwrap();
}

/// Inserts `count` one-move rows for PROFILE, `{prefix}1` onwards, last
/// improved `days` ago.
async fn insert_aged(db: &PgPool, prefix: &str, count: i32, days: i32) {
    sqlx::query(
        "INSERT INTO progress (profile, puzzle_id, fingerprint, moves, pushes, route, updated_at)
        SELECT $1, $2 || n, $3, 1, 1, 'D', now() - make_interval(days => $4)
        FROM generate_series(1, $5) n",
    )
    .bind(PROFILE)
    .bind(prefix)
    .bind(OTHER_LAYOUT)
    .bind(days)
    .bind(count)
    .execute(db)
    .await
    .unwrap();
}

/// The fingerprint of the catalog's current layout of `puzzle`.
fn fingerprint(puzzle: &str) -> String {
    app_state(None)
        .puzzle(puzzle)
        .unwrap()
        .fingerprint()
        .to_owned()
}

/// Startup's migration path: its own unbounded connection, closed before the
/// bounded pool is used, which keeps its limits.
#[tokio::test]
#[ignore = "requires SOKOMIND_TEST_DATABASE_URL pointing to a dedicated test database"]
async fn live_postgres_migrations_run_without_the_request_limits() {
    live(|db, options| async move {
        // Below, this test holds sqlx's per-database migration lock for
        // seconds. Meanwhile no other test migrates, so the lock timeout it
        // expects can only come from the table it locks itself.
        let _exclusive = MIGRATING.write().await;
        // Reapplying must be a no-op and verifies historical checksums too.
        database::run_migrations(&options).await.unwrap();
        let applied: i64 =
            sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(applied, 3);
        let limits = "SELECT current_setting('statement_timeout'), current_setting('lock_timeout')";
        let pool_limits: (String, String) = sqlx::query_as(limits).fetch_one(&db).await.unwrap();
        assert_eq!(pool_limits, ("1500ms".to_owned(), "500ms".to_owned()));
        // The zeros win over limits set earlier, as through PGOPTIONS.
        let migration = database::migration_options(database::bounded_options(options.clone()));
        let migrator = database::connect(migration, 1).await.unwrap();
        let migration_limits: (String, String) =
            sqlx::query_as(limits).fetch_one(&migrator).await.unwrap();
        migrator.close().await;
        assert_eq!(migration_limits, ("0".to_owned(), "0".to_owned()));

        // Hold the migrations table past both request limits. Under the
        // bounded settings, as before D1, migrating fails; startup's path
        // waits it out.
        let mut lock = db.begin().await.unwrap();
        sqlx::query("LOCK TABLE _sqlx_migrations IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let bounded = database::connect(database::bounded_options(options.clone()), 1)
            .await
            .unwrap();
        let error = database::migrate(bounded).await.unwrap_err();
        assert!(error.to_string().contains("lock timeout"), "{error}");
        let waiting = tokio::spawn(async move {
            database::run_migrations(&options)
                .await
                .map_err(|e| e.to_string())
        });
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!waiting.is_finished(), "migrating gave up on the lock wait");
        lock.rollback().await.unwrap();
        waiting.await.unwrap().unwrap();
    })
    .await;
}

/// Saves replay the route and keep the best one per puzzle layout; reads
/// return only the record for the catalog's current layout.
#[tokio::test]
#[ignore = "requires SOKOMIND_TEST_DATABASE_URL pointing to a dedicated test database"]
async fn live_postgres_progress_keeps_the_best_route_per_layout() {
    live(|db, _| async move {
        let app = test_router(app_state(Some(db.clone())));
        assert_ne!(fingerprint("ultra-tiny"), OTHER_LAYOUT);
        // A record for another layout is neither returned nor beaten.
        upsert(&db, "ultra-tiny", OTHER_LAYOUT, (1, 1, "D")).await;
        let saved = |improved: bool| Some(json!({ "saved": true, "improved": improved }));
        let record = |moves: i32, route: &str| {
            Some(json!({ "puzzle_id": "ultra-tiny", "moves": moves, "pushes": 1, "route": route }))
        };
        // In order: (method, body, status, the whole reply where pinned).
        let steps = [
            (
                "GET",
                Value::Null,
                StatusCode::NOT_FOUND,
                Some(json!({ "error": "No saved route" })),
            ),
            // Into the wall: the route does not solve the puzzle.
            ("POST", json!({"route":"U"}), StatusCode::BAD_REQUEST, None),
            ("POST", json!({"route":"LRD"}), StatusCode::OK, saved(true)),
            ("GET", Value::Null, StatusCode::OK, record(3, "LRD")),
            ("POST", json!({"route":"D"}), StatusCode::OK, saved(true)),
            ("POST", json!({"route":"LRD"}), StatusCode::OK, saved(false)),
            ("GET", Value::Null, StatusCode::OK, record(1, "D")),
        ];
        for (step, (method, body, status, expected)) in steps.into_iter().enumerate() {
            let (got, reply) = request(app.clone(), method, PROGRESS, body).await;
            assert_eq!(got, status, "step {step}, {method}: {reply}");
            if let Some(expected) = expected {
                assert_eq!(reply, expected, "step {step}, {method}");
            }
        }
        let copies: i64 =
            sqlx::query_scalar("SELECT count(*) FROM progress WHERE puzzle_id = 'ultra-tiny'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(copies, 2);
    })
    .await;
}

/// Concurrent contenders exercise the actual conditional UPSERT: whatever
/// their arrival order, the record keeps the fewest moves, then the fewest
/// pushes.
#[tokio::test]
#[ignore = "requires SOKOMIND_TEST_DATABASE_URL pointing to a dedicated test database"]
async fn live_postgres_racing_upserts_keep_the_best_route() {
    live(|_, options| async move {
        // The race tests the UPSERT, not the request limits, and the live
        // tests run in parallel. Through the request pool (5 connections, 1 s
        // acquire, 500 ms lock waits) a loaded runner could fail it on a
        // timeout, so it gets a pool of its own with generous limits, which
        // still end a stuck lock wait in bounded time.
        let db = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(options.options([("statement_timeout", "20s"), ("lock_timeout", "10s")]))
            .await
            .unwrap();
        // (puzzle, its contenders, the row that must win)
        let cases: [(&'static str, &[Row], Row); 2] = [
            ("fewer-moves", &[(1, 1, "D"), (3, 1, "LRD")], (1, 1, "D")),
            (
                "fewer-pushes",
                &[(3, 3, "LRD"), (3, 2, "LRD"), (3, 1, "LRD")],
                (3, 1, "LRD"),
            ),
        ];
        for (puzzle, contenders, (moves, pushes, route)) in cases {
            let mut racers = JoinSet::new();
            for _ in 0..8 {
                for &row in contenders {
                    let db = db.clone();
                    racers.spawn(async move { upsert(&db, puzzle, OTHER_LAYOUT, row).await });
                }
            }
            racers.join_all().await;
            let kept: Vec<(i32, i32, String)> =
                sqlx::query_as("SELECT moves, pushes, route FROM progress WHERE puzzle_id = $1")
                    .bind(puzzle)
                    .fetch_all(&db)
                    .await
                    .unwrap();
            assert_eq!(kept, [(moves, pushes, route.to_owned())], "{puzzle}");
        }
        db.close().await;
    })
    .await;
}

/// Lock waits answer 503 inside the deadline, and a later request can use
/// the same pool again. A save waiting on SQL holds only its progress slot:
/// other progress requests are refused at once, while health and native
/// solving, which take none, still answer.
#[tokio::test]
#[ignore = "requires SOKOMIND_TEST_DATABASE_URL pointing to a dedicated test database"]
async fn live_postgres_lock_waits_fail_in_time_and_hold_only_progress_slots() {
    live(|db, _| async move {
        let app = test_router(app_state(Some(db.clone())));
        upsert(&db, "ultra-tiny", &fingerprint("ultra-tiny"), (1, 1, "D")).await;
        // Hold a table lock so both reads and writes contend.
        let mut lock = db.begin().await.unwrap();
        sqlx::query("LOCK TABLE progress IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let started = Instant::now();
        let (status, error) = request(app.clone(), "GET", PROGRESS, Value::Null).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(error["error"].as_str().unwrap().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));

        let single = app_with(Some(db.clone()), |config| config.progress_concurrency = 1);
        let single_app = test_router(single.clone());
        let saving = tokio::spawn(request(
            single_app.clone(),
            "POST",
            PROGRESS,
            json!({"route":"D"}),
        ));
        until_available(&single.progress_slots, 0).await;
        let started = Instant::now();
        let (status, _) = request(single_app.clone(), "GET", PROGRESS, Value::Null).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(persistence(single_app.clone()).await);
        let (status, _) = request(single_app, "POST", "/api/solve", solve_body()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(saving.await.unwrap().0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(single.progress_slots.available_permits(), 1);
        // The save above may already have finished; here every progress slot
        // is held for certain, and health, whose own cache has no answer
        // yet, probes the database and reports it.
        let held = app_with(Some(db.clone()), |config| config.progress_concurrency = 1);
        let permit = held.progress_slots.clone().try_acquire_owned().unwrap();
        assert!(persistence(test_router(held)).await);
        drop(permit);

        lock.rollback().await.unwrap();
        let (status, _) = request(app, "GET", PROGRESS, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
    })
    .await;
}

/// PostgreSQL itself cancels a statement at the pool's statement_timeout,
/// before the server's own deadline, and the pool recovers.
#[tokio::test]
#[ignore = "requires SOKOMIND_TEST_DATABASE_URL pointing to a dedicated test database"]
async fn live_postgres_statement_timeouts_leave_the_pool_usable() {
    live(|db, _| async move {
        let started = Instant::now();
        let error = database::run(sqlx::query("SELECT pg_sleep(5)").execute(&db))
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(started.elapsed() < database::EXECUTION_TIMEOUT);
        let one: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&db).await.unwrap();
        assert_eq!(one, 1);
    })
    .await;
}

/// Retention deletes, in bounded batches through its index, the records last
/// improved before its window, skips rows another transaction has locked,
/// and keeps everything newer.
#[tokio::test]
#[ignore = "requires SOKOMIND_TEST_DATABASE_URL pointing to a dedicated test database"]
async fn live_postgres_retention_expires_only_old_unlocked_records() {
    live(|db, _| async move {
        insert_aged(&db, "expired-", 10_000, 60).await;
        insert_aged(&db, "kept-", 2, 29).await;
        sqlx::query("ANALYZE progress").execute(&db).await.unwrap();
        let plan: Vec<String> = sqlx::query_scalar(
            "EXPLAIN SELECT profile, puzzle_id, fingerprint FROM progress
            WHERE updated_at < now() - interval '30 days' ORDER BY updated_at
            LIMIT 500 FOR UPDATE SKIP LOCKED",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert!(
            plan.iter()
                .any(|line| line.contains("progress_updated_at_idx")),
            "{plan:?}"
        );
        let mut lock = db.begin().await.unwrap();
        sqlx::query("SELECT 1 FROM progress WHERE puzzle_id = 'expired-1' FOR UPDATE")
            .execute(&mut *lock)
            .await
            .unwrap();
        assert_eq!(database::expire_batch(&db, 30, 10).await.unwrap(), 10);
        let retained: i64 =
            sqlx::query_scalar("SELECT count(*) FROM progress WHERE puzzle_id = 'expired-1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(retained, 1);
        lock.rollback().await.unwrap();
        let mut deleted = 10;
        loop {
            let batch = database::expire_batch(&db, 30, 500).await.unwrap();
            assert!(batch <= 500);
            deleted += batch;
            if batch < 500 {
                break;
            }
        }
        assert_eq!(deleted, 10_000);
        let left: Vec<String> = sqlx::query_scalar("SELECT puzzle_id FROM progress ORDER BY 1")
            .fetch_all(&db)
            .await
            .unwrap();
        assert_eq!(left, ["kept-1", "kept-2"]);
    })
    .await;
}
