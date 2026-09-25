mod api;
mod client;
mod limit;
mod progress;
mod solve;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    routing::{get, post},
};
use client::TrustedProxies;
use limit::RateLimiter;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    env,
    net::SocketAddr,
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::Semaphore};

const BODY_LIMIT: usize = 128 * 1024;
const DB_POOL_SIZE: u32 = 5;
const DB_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(3);
/// Direct runs can race the database startup; retry briefly so compose's
/// restart policy is a backstop, not the only defense.
const DB_RETRY_WINDOW: Duration = Duration::from_secs(30);
const DB_RETRY_PAUSE: Duration = Duration::from_secs(1);
const SAVES_PER_MINUTE: u32 = 60;
const RATE_WINDOW: Duration = Duration::from_secs(60);
const RETENTION_SWEEP: Duration = Duration::from_secs(3600);
const MAX_RETENTION_DAYS: i32 = 36_500;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration errors fail before the database wait, not after it.
    let proxies = TrustedProxies::parse(&env::var("TRUSTED_PROXIES").unwrap_or_default())?;
    let concurrency = env_setting("SOLVE_CONCURRENCY", 1..=8, 1);
    let solve_rate = env_setting("SOLVE_RATE_PER_MINUTE", 1..=600, 20);
    let retention = retention_days();
    let db = match database_url()? {
        Some(url) => {
            let pool = connect(&url).await?;
            sqlx::migrate!("../../migrations").run(&pool).await?;
            if let Some(days) = retention {
                tokio::spawn(expire_progress(pool.clone(), days));
            }
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
    let state = api::App {
        db,
        catalog: Arc::new(catalog),
        slots: Arc::new(Semaphore::new(concurrency)),
        proxies: Arc::new(proxies),
        saves: Arc::new(RateLimiter::new(SAVES_PER_MINUTE, RATE_WINDOW)),
        solves: Arc::new(RateLimiter::new(solve_rate as u32, RATE_WINDOW)),
    };
    // API only: nginx serves the web app and adds the security and cache
    // headers. Any unrouted path, under /api or not, is a JSON 404.
    let app = Router::new()
        .route("/api/health", get(api::health))
        .route("/api/puzzles", get(api::catalog))
        .route("/api/solve", post(solve::solve))
        .route("/api/progress", get(progress::list))
        .route(
            "/api/progress/{id}",
            get(progress::get).post(progress::save),
        )
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(api_not_found)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state);
    let bind = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = TcpListener::bind(&bind).await?;
    eprintln!("Sokomind API listening at http://{bind}");
    // axum's serve() gives hyper no timer, so hyper's 30 s header read
    // timeout never arms; nginx, the required edge, bounds slow clients and
    // connection counts.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

/// Unset or empty means `default`; out-of-range values are clamped and
/// anything else falls back to `default`, with a warning either way.
fn env_setting(name: &str, range: RangeInclusive<usize>, default: usize) -> usize {
    let value = env::var(name).unwrap_or_default();
    if value.is_empty() {
        return default;
    }
    match value.parse::<usize>() {
        Ok(n) if range.contains(&n) => n,
        Ok(n) => {
            let (start, end) = range.into_inner();
            let clamped = n.clamp(start, end);
            eprintln!("{name}={n} is outside {start}..{end}; using {clamped}");
            clamped
        }
        Err(_) => {
            eprintln!("{name}={value:?} is not a number; using {default}");
            default
        }
    }
}

/// Retries only while the database is unreachable, for about
/// DB_RETRY_WINDOW; bad credentials or a missing database fail at once.
async fn connect(url: &str) -> Result<PgPool, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + DB_RETRY_WINDOW;
    loop {
        let error = match PgPoolOptions::new()
            .max_connections(DB_POOL_SIZE)
            .acquire_timeout(DB_ACQUIRE_TIMEOUT)
            .connect(url)
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(error) => error,
        };
        if !api::db_unreachable(&error) || Instant::now() >= deadline {
            return Err(format!("PostgreSQL connection failed: {error}").into());
        }
        eprintln!("PostgreSQL is not reachable yet ({error}); retrying");
        tokio::time::sleep(DB_RETRY_PAUSE).await;
    }
}

/// PROGRESS_RETENTION_DAYS: unset or 0 keeps progress forever.
fn retention_days() -> Option<i32> {
    let value = env::var("PROGRESS_RETENTION_DAYS").unwrap_or_default();
    if value.is_empty() {
        return None;
    }
    match value.parse::<i32>() {
        Ok(0) => None,
        Ok(days) if (1..=MAX_RETENTION_DAYS).contains(&days) => Some(days),
        _ => {
            eprintln!(
                "PROGRESS_RETENTION_DAYS={value:?} is not a number of days in 0..{MAX_RETENTION_DAYS}; progress is kept forever"
            );
            None
        }
    }
}

/// Deletes records whose best route was last improved more than `days`
/// ago: at startup (the first tick is immediate), then every hour.
async fn expire_progress(db: PgPool, days: i32) {
    let mut sweep = tokio::time::interval(RETENTION_SWEEP);
    loop {
        sweep.tick().await;
        let result = sqlx::query(
            "DELETE FROM progress WHERE updated_at < now() - make_interval(days => $1)",
        )
        .bind(days)
        .execute(&db)
        .await;
        match result {
            Ok(done) if done.rows_affected() > 0 => eprintln!(
                "Deleted {} progress records older than {days} days",
                done.rows_affected()
            ),
            Ok(_) => {}
            Err(error) => eprintln!("Progress retention sweep failed: {error}"),
        }
    }
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
