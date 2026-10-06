//! Router tests: routing, admission and the request limits.
use super::*;
use futures_util::{StreamExt, stream};
use std::convert::Infallible;

#[tokio::test]
async fn router_fallbacks_and_health() {
    let app = test_router(app_state());
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
        (
            "GET",
            "/api/progress/ultra-tiny",
            StatusCode::NOT_FOUND,
            not_found,
        ),
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
            json!({ "status": "ok" }),
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
    let state = app_state();
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
    let state = app_state();
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
async fn solve_memory_caps_at_1024_mib() {
    let state = app_state();
    let app = test_router(state.clone());
    let mut over = solve_body();
    over["memory_mib"] = json!(1025);
    let (status, body) = request(app, "POST", "/api/solve", over).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("4..1024 MiB"), "{error}");
    // Refused before it takes a solve slot.
    assert_eq!(state.solve_slots.available_permits(), 1);
}

/// The body limit admits the largest legal solve, a full route on the board
/// with the longest JSON, and answers a body one byte over it with the
/// `{error}` 413. The board is all wall, so the admitted solve is a 400.
#[tokio::test]
async fn body_limit_admits_the_largest_solve() {
    let app = test_router(app_state());
    let mut largest = solve_body();
    largest["rows"] = Value::from(vec!["O"; sokomind_core::MAX_CELLS]);
    largest["actions"] = json!("D".repeat(sokomind_core::MAX_ROUTE));
    assert!(largest.to_string().len() <= BODY_LIMIT);
    let (status, body) = request(app.clone(), "POST", "/api/solve", largest).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let mut over = json!({ "rows": [], "mode": "fast", "actions": "" });
    let padding = BODY_LIMIT + 1 - over.to_string().len();
    over["actions"] = json!("D".repeat(padding));
    assert_eq!(over.to_string().len(), BODY_LIMIT + 1);
    let (status, body) = request(app, "POST", "/api/solve", over).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(body["error"].is_string(), "{body}");
}

#[tokio::test]
async fn busy_solves_keep_the_rate_budget() {
    let state = app_with(|config| config.solve_rate = 1);
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
        test_router(app_state()).oneshot(request),
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
