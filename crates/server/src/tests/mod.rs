//! Tests that send requests through the whole router, as main builds it.
//! Those in `router` need no database; those in `live_postgres` need a
//! dedicated PostgreSQL database, so they run only when asked for.
mod live_postgres;
mod router;
use super::*;
use crate::api::App;
use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::Semaphore;
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
