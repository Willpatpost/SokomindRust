mod api;
mod progress;
mod solve;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{any, get, post},
};
use sqlx::postgres::PgPoolOptions;
use std::{env, path::Path, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tower_http::services::{ServeDir, ServeFile};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = if let Ok(url) = env::var("DATABASE_URL") {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(3))
            .connect(&url)
            .await?;
        sqlx::migrate!("../../migrations").run(&pool).await?;
        Some(pool)
    } else {
        eprintln!(
            "DATABASE_URL is unset; game and solver work, server progress persistence is disabled."
        );
        None
    };
    let catalog: Vec<api::Puzzle> =
        serde_json::from_str(include_str!("../../../data/puzzles.json"))?;
    for puzzle in &catalog {
        sokomind_core::Board::parse(&puzzle.rows.join("\n"))?;
    }
    let concurrency = env::var("SOLVE_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 8);
    let state = api::App {
        db,
        catalog: Arc::new(catalog),
        slots: Arc::new(Semaphore::new(concurrency)),
    };
    let static_dir = env::var("STATIC_DIR").unwrap_or_else(|_| "web/dist".into());
    let serve_dir = ServeDir::new(&static_dir)
        .fallback(ServeFile::new(Path::new(&static_dir).join("index.html")));
    let app = Router::new()
        .route("/api/health", get(api::health))
        .route("/api/puzzles", get(api::catalog))
        .route("/api/solve", post(solve::solve))
        .route("/api/progress", get(progress::list))
        .route(
            "/api/progress/{id}",
            get(progress::get).post(progress::save),
        )
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .fallback_service(serve_dir)
        .layer(DefaultBodyLimit::max(128 * 1024))
        .with_state(state);
    let bind = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    if Path::new(&static_dir).is_dir() {
        eprintln!("Serving web UI from {static_dir}");
    } else {
        eprintln!(
            "{static_dir} not found; run `npm run build` to serve the web UI from this server"
        );
    }
    eprintln!("Sokomind API listening at http://{bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn api_not_found() -> api::Error {
    api::Error::not_found()
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
