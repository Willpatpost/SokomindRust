mod api;
mod limit;
mod progress;
mod solve;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{StatusCode, Uri, header, HeaderValue},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use limit::SaveLimiter;
use sqlx::postgres::PgPoolOptions;
use std::{env, net::SocketAddr, path::{Path, PathBuf}, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = match database_url()? {
        Some(url) => {
            let mut pool = None;
            // Direct runs can race the database startup; retry briefly so
            // compose's restart policy is a backstop, not the only defense.
            for attempt in 0..30 {
                match PgPoolOptions::new()
                    .max_connections(5)
                    .acquire_timeout(Duration::from_secs(3))
                    .connect(&url)
                    .await
                {
                    Ok(connected) => {
                        pool = Some(connected);
                        break;
                    }
                    Err(error) if attempt == 29 => return Err(error.into()),
                    Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
                }
            }
            let pool = pool.expect("the retry loop returns or connects");
            sqlx::migrate!("../../migrations").run(&pool).await?;
            Some(pool)
        }
        None => {
            eprintln!(
                "DATABASE_URL and DATABASE_PASSWORD are unset; game and solver work, server progress persistence is disabled."
            );
            None
        }
    };
    let catalog: Vec<api::Puzzle> =
        serde_json::from_str(include_str!("../../../data/puzzles.json"))?;
    for puzzle in &catalog {
        sokomind_core::Board::parse(&puzzle.rows.join("\n"))?;
    }
    let concurrency = match env::var("SOLVE_CONCURRENCY") {
        Ok(value) => match value.parse::<usize>() {
            Ok(n) if (1..=8).contains(&n) => n,
            Ok(n) => {
                let clamped = n.clamp(1, 8);
                eprintln!("SOLVE_CONCURRENCY={n} is outside 1..8; using {clamped}");
                clamped
            }
            Err(_) => {
                eprintln!("SOLVE_CONCURRENCY={value:?} is not a number; using 1");
                1
            }
        },
        Err(_) => 1,
    };
    let state = api::App {
        db,
        catalog: Arc::new(catalog),
        slots: Arc::new(Semaphore::new(concurrency)),
        saves: Arc::new(SaveLimiter::new()),
    };
    let static_dir = env::var("STATIC_DIR").unwrap_or_else(|_| "web/dist".into());
    let index = Path::new(&static_dir).join("index.html");
    let serve_dir =
        ServeDir::new(&static_dir).fallback(Router::new().fallback(move |uri: Uri| {
            let index = index.clone();
            async move { spa_fallback(uri, index).await }
        }));
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
        .method_not_allowed_fallback(method_not_allowed)
        .fallback_service(serve_dir)
        .layer(middleware::from_fn(static_cache))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .with_state(state);
    let bind = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    if Path::new(&static_dir).is_dir() {
        eprintln!("Serving web UI from {static_dir}");
    } else {
        eprintln!(
            "{static_dir} not found; serving the API only. Set STATIC_DIR or build the web UI (npm run build) to serve it here."
        );
    }
    eprintln!("Sokomind API listening at http://{bind}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

/// Missing assets must 404: serving the app shell with a 200 would make a
/// stale page hang on a dead content hash after a rebuild.
async fn spa_fallback(uri: Uri, index: PathBuf) -> Response {
    if Path::new(uri.path()).extension().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(&index).await {
        Ok(body) => {
            let mut response = body.into_response();
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            response
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Vite content-hashes everything under /assets/, so those responses are
/// immutable; the shell must always be revalidated after a rebuild.
async fn static_cache(uri: Uri, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    if response.status().is_success() && !response.headers().contains_key(header::CACHE_CONTROL) {
        let path = uri.path();
        let value = if path.starts_with("/assets/") {
            "public, max-age=31536000, immutable"
        } else if path == "/" || path == "/index.html" {
            "no-cache"
        } else {
            return response;
        };
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    }
    response
}

async fn api_not_found() -> api::Error {
    api::Error::not_found()
}

async fn method_not_allowed() -> api::Error {
    api::Error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed".into())
}

/// DATABASE_PASSWORD composes the URL here so deployments can pass a raw
/// password: components are percent-encoded, which URL text cannot do for
/// itself.
fn database_url() -> Result<Option<String>, Box<dyn std::error::Error>> {
    if let Ok(url) = env::var("DATABASE_URL") {
        return Ok(Some(url));
    }
    match env::var("DATABASE_PASSWORD") {
        Ok(password) => {
            let user = env::var("DATABASE_USER").unwrap_or_else(|_| "sokomind".into());
            let host = env::var("DATABASE_HOST").unwrap_or_else(|_| "db".into());
            let port = env::var("DATABASE_PORT").unwrap_or_else(|_| "5432".into());
            let name = env::var("DATABASE_NAME").unwrap_or_else(|_| "sokomind".into());
            Ok(Some(format!(
                "postgres://{}:{}@{}:{}/{}",
                percent_encode(&user),
                percent_encode(&password),
                host,
                port,
                percent_encode(&name)
            )))
        }
        Err(_) => Ok(None),
    }
}
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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
