use super::*;
use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use tower::ServiceExt;

const PROFILE: &str = "0123456789abcdef0123456789abcdef";

fn app_state(db: Option<PgPool>) -> api::App {
    api::App {
        db,
        catalog: Arc::new(api::load_catalog().unwrap()),
        slots: Arc::new(Semaphore::new(1)),
        progress_slots: Arc::new(Semaphore::new(4)),
        proxies: Arc::new(TrustedProxies::default()),
        saves: Arc::new(RateLimiter::new(100, RATE_WINDOW)),
        solves: Arc::new(RateLimiter::new(100, RATE_WINDOW)),
    }
}

fn test_router(state: api::App) -> Router {
    router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
}

async fn request(app: Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("x-profile-id", PROFILE)
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
    assert_eq!(body["proof"]["kind"], "optimal");
    assert!(body["stats"]["unique_states"].as_u64().unwrap() >= 1);
    assert_eq!(state.slots.available_permits(), 1);
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
        let db = PgPoolOptions::new()
            .connect_lazy("postgres://invalid@127.0.0.1:1/invalid")
            .unwrap();
        let mut state = app_state(Some(db));
        state.progress_slots = Arc::new(Semaphore::new(1));
        let saving = tokio::spawn(request(
            test_router(state.clone()),
            "POST",
            "/api/progress/ultra-tiny",
            json!({"route":"D"}),
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.progress_slots.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        saving.abort();
        assert!(saving.await.unwrap_err().is_cancelled());
        assert_eq!(state.progress_slots.available_permits(), 0);
        release.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.progress_slots.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    });
}

/// Explicitly ignored so an ordinary test run never connects to a user's
/// database. The supplied dedicated test DB must allow CREATE SCHEMA. Every
/// table/migration is scoped to a generated schema; no public table is touched.
#[tokio::test]
#[ignore = "requires SOKOMIND_TEST_DATABASE_URL pointing to a dedicated test database"]
async fn live_postgres_persistence() {
    let url = env::var("SOKOMIND_TEST_DATABASE_URL")
        .expect("set SOKOMIND_TEST_DATABASE_URL to a dedicated disposable PostgreSQL database");
    let options: PgConnectOptions = url.parse().unwrap();
    let mut admin = PgConnection::connect_with(&database::bounded_options(options.clone()))
        .await
        .unwrap();
    let schema = format!(
        "sokomind_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&mut admin)
        .await
        .unwrap();
    let pool_result = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(database::ACQUIRE_TIMEOUT)
        .connect_with(
            database::bounded_options(options).options([("search_path", schema.as_str())]),
        )
        .await;
    let pool = match pool_result {
        Ok(pool) => pool,
        Err(error) => {
            sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(&mut admin)
                .await
                .unwrap();
            panic!("connect to isolated test schema: {error}");
        }
    };
    // Always clean up our own schema, including when assertions panic.
    let result = tokio::spawn(live_checks(pool.clone())).await;
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&mut admin)
        .await
        .unwrap();
    if let Err(error) = result {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("live database test task was cancelled");
    }
}

async fn live_checks(db: PgPool) {
    sqlx::migrate!("../../migrations").run(&db).await.unwrap();
    // Reapplying must be a no-op and verifies historical checksums too.
    sqlx::migrate!("../../migrations").run(&db).await.unwrap();
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(applied, 3);
    let state = app_state(Some(db.clone()));
    let app = test_router(state.clone());
    let fingerprint = state.puzzle("ultra-tiny").unwrap().fingerprint().to_owned();
    let old = "puzzle-v1:00000000";
    assert_ne!(fingerprint, old);
    upsert(&db, "ultra-tiny", old, "D").await;
    assert_eq!(
        request(app.clone(), "GET", "/api/progress/ultra-tiny", Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/api/progress/ultra-tiny",
            json!({"route":"U"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/api/progress/ultra-tiny",
            json!({"route":"LRD"})
        )
        .await
        .0,
        StatusCode::OK
    );

    // Concurrent contenders exercise the actual conditional UPSERT,
    // independently of arrival order.
    let mut tasks = Vec::new();
    for index in 0..24 {
        let db = db.clone();
        let fingerprint = fingerprint.clone();
        tasks.push(tokio::spawn(async move {
            let route = if index % 2 == 0 { "D" } else { "LRD" };
            upsert(&db, "ultra-tiny", &fingerprint, route).await;
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let (status, best) = request(app.clone(), "GET", "/api/progress/ultra-tiny", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(best["moves"], 1);
    assert_eq!(best["pushes"], 1);
    assert_eq!(best["route"], "D");
    let (_, unchanged) = request(
        app.clone(),
        "POST",
        "/api/progress/ultra-tiny",
        json!({"route":"LRD"}),
    )
    .await;
    assert_eq!(unchanged["improved"], false);
    let copies: i64 =
        sqlx::query_scalar("SELECT count(*) FROM progress WHERE puzzle_id='ultra-tiny'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(copies, 2);
    let mut tasks = Vec::new();
    for pushes in [3, 2, 1, 2, 3, 1] {
        let db = db.clone();
        tasks.push(tokio::spawn(async move {
            sqlx::query(progress::UPSERT)
                .bind(PROFILE)
                .bind("push-tie")
                .bind("puzzle-v1:11111111")
                .bind(3i32)
                .bind(pushes)
                .bind("LRD")
                .execute(&db)
                .await
                .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let pushes: i32 = sqlx::query_scalar("SELECT pushes FROM progress WHERE puzzle_id='push-tie'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(pushes, 1);
    sqlx::query("DELETE FROM progress WHERE puzzle_id='push-tie'")
        .execute(&db)
        .await
        .unwrap();

    // Lock waits return 503 inside the deadline; a later request can use the
    // same pool again. Hold a table lock so both reads and writes contend.
    let mut lock = db.begin().await.unwrap();
    sqlx::query("LOCK TABLE progress IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let started = Instant::now();
    let (status, error) =
        request(app.clone(), "GET", "/api/progress/ultra-tiny", Value::Null).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(error["error"].as_str().unwrap().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(3));

    // One save owns progress admission while waiting on SQL. Another request
    // is rejected immediately, health degrades, and native solving still runs.
    let mut single = state.clone();
    single.progress_slots = Arc::new(Semaphore::new(1));
    let single_app = test_router(single.clone());
    let saving = tokio::spawn(request(
        single_app.clone(),
        "POST",
        "/api/progress/ultra-tiny",
        json!({"route":"D"}),
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        while single.progress_slots.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let started = Instant::now();
    assert_eq!(
        request(
            single_app.clone(),
            "GET",
            "/api/progress/ultra-tiny",
            Value::Null
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert!(started.elapsed() < Duration::from_millis(200));
    let (_, health) = request(single_app.clone(), "GET", "/api/health", Value::Null).await;
    assert_eq!(health["persistence"], false);
    assert_eq!(
        request(single_app.clone(), "POST", "/api/solve", solve_body())
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(saving.await.unwrap().0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(single.progress_slots.available_permits(), 1);
    lock.rollback().await.unwrap();
    assert_eq!(
        request(app.clone(), "GET", "/api/progress/ultra-tiny", Value::Null)
            .await
            .0,
        StatusCode::OK
    );

    // Exercise PostgreSQL's statement cancellation and pool recovery.
    let error = database::run(sqlx::query("SELECT pg_sleep(5)").execute(&db))
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&db)
            .await
            .unwrap(),
        1
    );
    retention_checks(&db).await;

    // A closed/unavailable pool maps to the same stable API response and
    // releases admission. This does not close the shared harness pool.
    let unavailable = PgPoolOptions::new()
        .connect_lazy("postgres://invalid@127.0.0.1:1/invalid")
        .unwrap();
    unavailable.close().await;
    let unavailable_state = app_state(Some(unavailable));
    assert_eq!(
        request(
            test_router(unavailable_state.clone()),
            "GET",
            "/api/progress/ultra-tiny",
            Value::Null
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(unavailable_state.progress_slots.available_permits(), 4);
    let refused = PgPoolOptions::new()
        .acquire_timeout(database::ACQUIRE_TIMEOUT)
        .connect_lazy("postgres://invalid@127.0.0.1:1/invalid")
        .unwrap();
    let refused_state = app_state(Some(refused));
    let started = Instant::now();
    assert_eq!(
        request(
            test_router(refused_state.clone()),
            "GET",
            "/api/progress/ultra-tiny",
            Value::Null
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(refused_state.progress_slots.available_permits(), 4);
}

async fn upsert(db: &PgPool, puzzle: &str, fingerprint: &str, route: &str) {
    sqlx::query(progress::UPSERT)
        .bind(PROFILE)
        .bind(puzzle)
        .bind(fingerprint)
        .bind(route.len() as i32)
        .bind(1i32)
        .bind(route)
        .execute(db)
        .await
        .unwrap();
}

async fn retention_checks(db: &PgPool) {
    sqlx::query(
        "INSERT INTO progress (profile,puzzle_id,fingerprint,moves,pushes,route,updated_at)
        SELECT $1, 'expired-' || n, 'puzzle-v1:11111111', 1, 1, 'D', now() - interval '60 days'
        FROM generate_series(1,10000) n",
    )
    .bind(PROFILE)
    .execute(db)
    .await
    .unwrap();
    sqlx::query("ANALYZE progress").execute(db).await.unwrap();
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT profile,puzzle_id,fingerprint FROM progress
        WHERE updated_at < now() - interval '30 days' ORDER BY updated_at LIMIT 500 FOR UPDATE SKIP LOCKED")
        .fetch_all(db).await.unwrap();
    assert!(
        plan.iter()
            .any(|line| line.contains("progress_updated_at_idx")),
        "{plan:?}"
    );
    let mut lock = db.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM progress WHERE puzzle_id='expired-1' FOR UPDATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    assert_eq!(database::expire_batch(db, 30, 10).await.unwrap(), 10);
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM progress WHERE puzzle_id='expired-1'")
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(retained, 1);
    lock.rollback().await.unwrap();
    let mut deleted = 10;
    loop {
        let batch = database::expire_batch(db, 30, 500).await.unwrap();
        assert!(batch <= 500);
        deleted += batch;
        if batch < 500 {
            break;
        }
    }
    assert_eq!(deleted, 10000);
    let recent: i64 = sqlx::query_scalar("SELECT count(*) FROM progress")
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(recent, 2);
}
