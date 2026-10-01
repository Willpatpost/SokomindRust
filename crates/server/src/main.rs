//! Sokomind's HTTP API, which must run behind nginx: nginx serves the web
//! app, adds the security headers and bounds slow clients, so this binary
//! answers only these routes:
//! - `GET /api/health`: whether the API and its database answer
//!   ([`api::health`]).
//! - `POST /api/solve`: one native search under the server's limits
//!   ([`solve::solve`]).
//! - `GET /api/progress/{id}`: a browser profile's best route for a catalog
//!   puzzle, from PostgreSQL ([`progress::get`]).
//! - `POST /api/progress/{id}`: replays a route and keeps it when it beats
//!   the saved one ([`progress::save`]).
//!
//! Every error body is `{"error": "..."}` ([`api::Error`]): 400 invalid
//! input; 404 an unknown endpoint, catalog puzzle or saved route; 405 a wrong
//! method; 408 a body still arriving after [`api::BODY_TIMEOUT`]; 413 a body
//! over [`BODY_LIMIT`]; 415 a missing or non-JSON content type; 422 JSON of
//! the wrong shape; 429 a busy server or a spent rate budget; 503 persistence
//! off, unreachable or interrupted, or a failed search allocation; 500 a
//! server bug. Behind the bundled nginx, whose `client_max_body_size` matches
//! [`BODY_LIMIT`], an oversize body gets nginx's own HTML 413 instead; the
//! README notes this and nginx's HTML 5xx pages.
//!
//! Handlers admit a request in one order, cheapest and most client-caused
//! first, and skip the steps a route does not have: a solve names no catalog
//! puzzle and needs no database, and a read spends no rate budget.
//! 1. The request itself: 400.
//! 2. The catalog puzzle: 404.
//! 3. Persistence: 503.
//! 4. A free solve or progress slot: 429 busy, at once, since nothing queues.
//! 5. The client's solve or save budget: 429. It is charged last, so a
//!    request refused earlier spends none of it.
//!
//! Parsing a position and replaying a route cost CPU, so both run after
//! admission, on a blocking thread, and their 400 comes after every step.
//!
//! Settings come from the environment once at startup: [`config`] reads and
//! checks them, and README's Configuration table lists them.
// A binary's docs are for its maintainers, and cargo documents a binary's
// private items, so these links resolve.
#![allow(rustdoc::private_intra_doc_links)]
mod api;
mod client;
mod config;
mod database;
mod limit;
mod progress;
mod solve;
#[cfg(test)]
mod tests;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    routing::{get, post},
};
use config::Config;
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// The largest request body the JSON extractor reads; a larger one is 413.
/// It must hold the largest legal request: a solve whose `actions` is a full
/// [`MAX_ROUTE`](sokomind_core::MAX_ROUTE) route, on a board of
/// [`MAX_CELLS`](sokomind_core::MAX_CELLS) one-cell rows, the longest board
/// in JSON at four bytes a row. The assert below checks that with 1 KiB to
/// spare for the other fields.
///
/// deploy/nginx.conf's `client_max_body_size`, README's status list and the
/// [`api::BODY_TIMEOUT`] doc quote it; change them with it. Raise one
/// without nginx's and nginx refuses the bodies between them first, with
/// its own HTML 413.
const BODY_LIMIT: usize = 128 * 1024;
const _: () = assert!(BODY_LIMIT >= sokomind_core::MAX_ROUTE + 4 * sokomind_core::MAX_CELLS + 1024);

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
    // timeout never arms. Behind nginx, the required edge, that is moot:
    // nginx reads each whole request before passing it on and bounds slow
    // clients and connection counts. A direct run has only BODY_TIMEOUT,
    // which bounds bodies but not headers.
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

/// API only: nginx serves the web app and adds the security and cache
/// headers. Any unrouted path the API receives is a JSON 404.
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
