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
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    env,
    fmt::Display,
    net::SocketAddr,
    ops::RangeInclusive,
    str::FromStr,
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
    // Configuration, catalog and bind errors fail before the database wait,
    // not after it.
    let proxies = TrustedProxies::parse(&env::var("TRUSTED_PROXIES").unwrap_or_default())?;
    let concurrency = env_setting("SOLVE_CONCURRENCY", 1..=8, 1);
    let solve_rate = env_setting("SOLVE_RATE_PER_MINUTE", 1..=600, 20);
    let retention = retention(env_setting(
        "PROGRESS_RETENTION_DAYS",
        0..=MAX_RETENTION_DAYS,
        0,
    ));
    let options = database_options()?;
    let catalog = api::load_catalog()?;
    let bind = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = TcpListener::bind(&bind).await?;
    let db = match options {
        Some(options) => {
            let pool = connect(options).await?;
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
    let state = api::App {
        db,
        catalog: Arc::new(catalog),
        slots: Arc::new(Semaphore::new(concurrency)),
        proxies: Arc::new(proxies),
        saves: Arc::new(RateLimiter::new(SAVES_PER_MINUTE, RATE_WINDOW)),
        solves: Arc::new(RateLimiter::new(solve_rate, RATE_WINDOW)),
    };
    eprintln!("Sokomind API listening at http://{bind}");
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

/// Reads `name` from the environment; see [`parse_setting`].
fn env_setting<T: FromStr + PartialOrd + Copy + Display>(
    name: &str,
    range: RangeInclusive<T>,
    default: T,
) -> T {
    let (value, warning) = parse_setting(name, &env::var(name).unwrap_or_default(), range, default);
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    value
}

/// Empty means `default`; out-of-range values are clamped and anything
/// else falls back to `default`, with a warning either way.
fn parse_setting<T: FromStr + PartialOrd + Copy + Display>(
    name: &str,
    value: &str,
    range: RangeInclusive<T>,
    default: T,
) -> (T, Option<String>) {
    if value.is_empty() {
        return (default, None);
    }
    let (start, end) = (*range.start(), *range.end());
    match value.parse::<T>() {
        Ok(n) if range.contains(&n) => (n, None),
        Ok(n) => {
            let clamped = if n < start { start } else { end };
            let warning = format!("{name}={n} is outside {start}..{end}; using {clamped}");
            (clamped, Some(warning))
        }
        Err(_) => {
            let warning = format!("{name}={value:?} is not a number; using {default}");
            (default, Some(warning))
        }
    }
}

/// PROGRESS_RETENTION_DAYS: 0, the default, keeps progress forever.
fn retention(days: i32) -> Option<i32> {
    (days > 0).then_some(days)
}

/// Retries only while the database is unreachable, for about
/// DB_RETRY_WINDOW; bad credentials or a missing database fail at once.
async fn connect(options: PgConnectOptions) -> Result<PgPool, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + DB_RETRY_WINDOW;
    loop {
        let error = match PgPoolOptions::new()
            .max_connections(DB_POOL_SIZE)
            .acquire_timeout(DB_ACQUIRE_TIMEOUT)
            .connect_with(options.clone())
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
    api::Error::new(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
}

/// DATABASE_URL wins. Otherwise DATABASE_PASSWORD enables persistence and
/// the other DATABASE_* parts default to the compose service; the parts
/// are passed as they are, so a raw password needs no URL encoding.
fn database_options() -> Result<Option<PgConnectOptions>, Box<dyn std::error::Error>> {
    if let Ok(url) = env::var("DATABASE_URL") {
        return Ok(Some(url.parse()?));
    }
    let Ok(password) = env::var("DATABASE_PASSWORD") else {
        return Ok(None);
    };
    let part = |name: &str, default: &str| -> String {
        env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default.into())
    };
    let port = part("DATABASE_PORT", "5432");
    let port: u16 = port
        .parse()
        .map_err(|_| format!("DATABASE_PORT={port:?} is not a port number"))?;
    Ok(Some(
        PgConnectOptions::new()
            .host(&part("DATABASE_HOST", "db"))
            .port(port)
            .username(&part("DATABASE_USER", "sokomind"))
            .password(&password)
            .database(&part("DATABASE_NAME", "sokomind")),
    ))
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

    #[test]
    fn settings_default_clamp_and_reject() {
        let cases = [
            ("", 1, None),
            ("4", 4, None),
            ("8", 8, None),
            ("0", 1, Some("N=0 is outside 1..8; using 1")),
            ("99", 8, Some("N=99 is outside 1..8; using 8")),
            ("-1", 1, Some("N=\"-1\" is not a number; using 1")),
            ("two", 1, Some("N=\"two\" is not a number; using 1")),
        ];
        for (value, expected, warning) in cases {
            let (n, message) = parse_setting::<usize>("N", value, 1..=8, 1);
            assert_eq!((n, message.as_deref()), (expected, warning), "{value:?}");
        }
    }

    #[test]
    fn retention_zero_keeps_forever_and_large_values_clamp() {
        let cases = [
            ("", None),
            ("0", None),
            ("-5", None),
            ("30", Some(30)),
            ("36500", Some(36_500)),
            ("99999", Some(36_500)),
            ("soon", None),
        ];
        for (value, expected) in cases {
            let (days, _) = parse_setting("D", value, 0..=MAX_RETENTION_DAYS, 0);
            assert_eq!(retention(days), expected, "{value:?}");
        }
    }
}
