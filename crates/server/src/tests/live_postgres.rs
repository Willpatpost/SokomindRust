//! Live PostgreSQL tests. They are ignored, so an ordinary test run never
//! connects to a user's database; `npm run test:db` selects them by the
//! `live_postgres` name prefix with `--ignored`. SOKOMIND_TEST_DATABASE_URL
//! must name a dedicated, disposable database that allows CREATE SCHEMA.
//!
//! Each test runs in a schema of its own (see `live`), migrated as at startup
//! and dropped afterwards, and writes every row it reads, so the tests share
//! no rows and run in parallel against one database; no public table is
//! touched. The one state they share is sqlx's migration advisory lock, which
//! is per database, not per schema: `MIGRATING` keeps the test that holds it
//! on purpose apart from the other tests' migrations.
use super::*;
use sqlx::{
    Connection, PgConnection,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    env,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::RwLock, task::JoinSet};

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
        // pg_settings reports both limits in milliseconds, where
        // current_setting would switch to seconds for whole ones.
        let limits = "SELECT s.setting, l.setting FROM pg_settings s, pg_settings l
            WHERE s.name = 'statement_timeout' AND l.name = 'lock_timeout'";
        let pool_limits: (String, String) = sqlx::query_as(limits).fetch_one(&db).await.unwrap();
        let millis = |limit: Duration| limit.as_millis().to_string();
        assert_eq!(
            pool_limits,
            (
                millis(database::STATEMENT_TIMEOUT),
                millis(database::LOCK_TIMEOUT)
            )
        );
        // The zeros win over limits set earlier, as through PGOPTIONS.
        let migration = database::migration_options(database::bounded_options(options.clone()));
        let migrator = database::connect(migration, 1).await.unwrap();
        let migration_limits: (String, String) =
            sqlx::query_as(limits).fetch_one(&migrator).await.unwrap();
        migrator.close().await;
        assert_eq!(migration_limits, ("0".to_owned(), "0".to_owned()));

        // Hold the migrations table past both request limits. Migrating
        // under the bounded settings fails on the lock timeout; startup's
        // path, whose connection has no limits, waits it out.
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
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
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
