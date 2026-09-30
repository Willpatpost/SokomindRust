//! Router tests that need no database: routing, admission, the request
//! limits, and the answers a configured but unreachable database gives.
use super::*;
use futures_util::{StreamExt, stream};
use sqlx::postgres::PgPoolOptions;
use std::{convert::Infallible, net::IpAddr, time::Instant};

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

#[tokio::test]
async fn router_fallbacks_and_health_without_database() {
    let app = test_router(app_state(None));
    let not_found = json!({ "error": "Unknown API endpoint" });
    let cases = [
        ("GET", "/api", StatusCode::NOT_FOUND, not_found.clone()),
        ("GET", "/api/", StatusCode::NOT_FOUND, not_found.clone()),
        ("GET", "/api/x", StatusCode::NOT_FOUND, not_found.clone()),
        (
            "GET",
            "/api/puzzles",
            StatusCode::NOT_FOUND,
            not_found.clone(),
        ),
        ("GET", "/api/progress", StatusCode::NOT_FOUND, not_found),
        (
            "GET",
            "/api/solve",
            StatusCode::METHOD_NOT_ALLOWED,
            json!({ "error": "Method not allowed" }),
        ),
        (
            "GET",
            "/api/health",
            StatusCode::OK,
            json!({ "status": "ok", "persistence": false }),
        ),
    ];
    for (method, path, status, body) in cases {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status, "{method} {path}");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json, body, "{method} {path}");
    }
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
    let permit = state.solve_slots.clone().acquire_owned().await.unwrap();
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
    assert_eq!(state.solve_slots.available_permits(), 1);
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
    assert!(error.contains("1..60000000 states"), "{error}");
    // At the cap, the 4 MiB budget sizes the arena down instead of failing.
    let mut at_cap = solve_body();
    at_cap["max_states"] = json!(sokomind_search::MAX_STATES);
    let (status, body) = request(app, "POST", "/api/solve", at_cap).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["route"], "D");
    assert_eq!(body["moves"], 1);
    assert_eq!(state.solve_slots.available_permits(), 1);
}

#[tokio::test]
async fn solve_memory_caps_at_128_mib() {
    let state = app_state(None);
    let app = test_router(state.clone());
    let mut over = solve_body();
    over["memory_mib"] = json!(129);
    let (status, body) = request(app, "POST", "/api/solve", over).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("4..128 MiB"), "{error}");
    // Refused before it takes a solve slot.
    assert_eq!(state.solve_slots.available_permits(), 1);
}

#[tokio::test]
async fn busy_solves_keep_the_rate_budget() {
    let state = app_with(None, |config| config.solve_rate = 1);
    let app = test_router(state.clone());
    let permit = state.solve_slots.clone().acquire_owned().await.unwrap();
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

/// A client that stops sending mid-body gets the `{error}` 408 once
/// `BODY_TIMEOUT` passes instead of holding the handler open. The clock is
/// paused, so tokio skips straight to each timer and the wait takes no real
/// time.
#[tokio::test(start_paused = true)]
async fn stalled_bodies_time_out_with_408() {
    // The start of a JSON body, then nothing, ever.
    let stalled = stream::iter([Ok::<_, Infallible>(r#"{"rows":"#)]).chain(stream::pending());
    let request = Request::builder()
        .method("POST")
        .uri("/api/solve")
        .header("content-type", "application/json")
        .body(Body::from_stream(stalled))
        .unwrap();
    let started = tokio::time::Instant::now();
    let response = tokio::time::timeout(
        2 * api::BODY_TIMEOUT,
        test_router(app_state(None)).oneshot(request),
    )
    .await
    .expect("the body timeout answers first")
    .unwrap();
    assert!(started.elapsed() >= api::BODY_TIMEOUT);
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    let bytes = axum::body::to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body, json!({ "error": "Request body timed out" }));
}
