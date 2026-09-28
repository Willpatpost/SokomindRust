mod api;
mod client;
mod config;
mod database;
#[cfg(test)]
mod integration_tests;
mod limit;
mod progress;
mod solve;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    routing::{get, post},
};
use config::Config;
use std::net::SocketAddr;
use tokio::net::TcpListener;

const BODY_LIMIT: usize = 128 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration, catalog and bind errors fail before the database wait,
    // not after it.
    let config = Config::from_env()?;
    let catalog = api::load_catalog()?;
    let listener = TcpListener::bind(&config.bind).await?;
    let db = match &config.database {
        Some(settings) => Some(database::open(settings).await?),
        None => {
            eprintln!(
                "DATABASE_URL and DATABASE_PASSWORD are unset; game and solver work, server progress persistence is disabled."
            );
            None
        }
    };
    eprintln!("Sokomind API listening at http://{}", config.bind);
    let state = api::App::new(config, catalog, db);
    // axum's serve() gives hyper no timer, so hyper's 30 s header read
    // timeout never arms; nginx, the required edge, bounds slow clients and
    // connection counts.
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

/// API only: nginx serves the web app and adds the security and cache
/// headers. Any unrouted path, under /api or not, is a JSON 404.
fn router(state: api::App) -> Router {
    Router::new()
        .route("/api/health", get(api::health))
        .route("/api/solve", post(solve::solve))
        .route(
            "/api/progress/{id}",
            get(progress::get).post(progress::save),
        )
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(api_not_found)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state)
}

async fn api_not_found() -> api::Error {
    api::Error::not_found()
}

async fn method_not_allowed() -> api::Error {
    api::Error::new(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn router_fallbacks_and_health_without_database() {
        use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
        use serde_json::{Value, json};
        use tower::ServiceExt;

        let state = api::App::new(Config::defaults(), api::load_catalog().unwrap(), None);
        let app = router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
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
}
