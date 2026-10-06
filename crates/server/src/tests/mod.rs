//! Tests that send requests through the whole router, as main builds it.
mod router;
use super::*;
use crate::api::App;
use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
use serde_json::{Value, json};
use tower::ServiceExt;

const CLIENT: [u8; 4] = [127, 0, 0, 1];

/// The App main builds from an empty environment after `adjust`.
fn app_with(adjust: impl FnOnce(&mut Config)) -> App {
    let mut config = Config::defaults();
    adjust(&mut config);
    App::new(config)
}

fn app_state() -> App {
    app_with(|_| {})
}

fn test_router(state: App) -> Router {
    router(state).layer(MockConnectInfo(SocketAddr::from((CLIENT, 0))))
}

async fn request(app: Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
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
