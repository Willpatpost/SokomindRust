use crate::{api::Error, config};
use sqlx::{
    PgPool,
    migrate::MigrateError,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::time::{Duration, Instant};

pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(1);
pub const EXECUTION_TIMEOUT: Duration = Duration::from_millis(2500);
/// Direct runs can race the database startup; retry briefly so compose's
/// restart policy is a backstop, not the only defense.
const RETRY_WINDOW: Duration = Duration::from_secs(30);
const RETRY_PAUSE: Duration = Duration::from_secs(1);
const RETENTION_SWEEP: Duration = Duration::from_secs(3600);
const RETENTION_MAX_BATCHES: usize = 20;

/// Startup's database work: applies pending migrations on a connection of
/// their own, opens the bounded request pool, and starts the retention sweep
/// when one is configured.
pub async fn open(settings: &config::Database) -> Result<PgPool, Box<dyn std::error::Error>> {
    run_migrations(&settings.options).await?;
    let pool = connect(
        bounded_options(settings.options.clone()),
        settings.pool_size,
    )
    .await?;
    if let Some(days) = settings.retention_days {
        tokio::spawn(expire_progress(
            pool.clone(),
            days,
            settings.retention_batch_size,
        ));
    }
    Ok(pool)
}

/// Retries only while the database is unreachable, for about
/// RETRY_WINDOW; bad credentials or a missing database fail at once.
pub async fn connect(
    options: PgConnectOptions,
    pool_size: u32,
) -> Result<PgPool, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + RETRY_WINDOW;
    loop {
        let error = match PgPoolOptions::new()
            .max_connections(pool_size)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_with(options.clone())
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(error) => error,
        };
        if !unreachable(&error) || Instant::now() >= deadline {
            return Err(format!("PostgreSQL connection failed: {error}").into());
        }
        eprintln!("PostgreSQL is not reachable yet ({error}); retrying");
        tokio::time::sleep(RETRY_PAUSE).await;
    }
}

/// Failures that mean the database cannot be reached right now (refused,
/// reset, restarting, saturated) rather than that a query is wrong.
pub fn unreachable(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Io(_) | sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed => true,
        // Class 08 is connection exceptions; 53300 is too many connections;
        // 57P01-57P03 are shutdowns and a server still starting up.
        sqlx::Error::Database(error) => error.code().is_some_and(|code| {
            code.starts_with("08") || matches!(&*code, "53300" | "57P01" | "57P02" | "57P03")
        }),
        _ => false,
    }
}

/// Per-session limits also stop SQL after an HTTP client drops its request.
/// Keep lock_timeout below statement_timeout, and both below our deadline.
pub fn bounded_options(options: PgConnectOptions) -> PgConnectOptions {
    options.options([("statement_timeout", "1500ms"), ("lock_timeout", "500ms")])
}

/// Migrations run without the request limits: an index build or backfill can
/// outlast statement_timeout, and sqlx's migration lock can wait for another
/// replica longer than lock_timeout. Explicit zeros also override limits set
/// through PGOPTIONS or as role or database defaults.
pub fn migration_options(options: PgConnectOptions) -> PgConnectOptions {
    options.options([("statement_timeout", "0"), ("lock_timeout", "0")])
}

/// Applies pending migrations before the bounded pool opens, on one
/// connection without its statement and lock timeouts, and closes it.
pub async fn run_migrations(options: &PgConnectOptions) -> Result<(), Box<dyn std::error::Error>> {
    let migrator = connect(migration_options(options.clone()), 1).await?;
    migrate(migrator).await?;
    Ok(())
}

/// Applies pending migrations through `migrator`, a one-connection pool
/// opened with [`migration_options`], then closes it whatever the result.
/// Closing ends the session, which releases sqlx's advisory lock even when a
/// failed migration left it held.
pub async fn migrate(migrator: PgPool) -> Result<(), MigrateError> {
    let result = sqlx::migrate!("../../migrations").run(&migrator).await;
    migrator.close().await;
    result
}

/// Includes pool acquisition, execution and response decoding. There is no
/// retry here: callers must not create a second write after an uncertain result.
pub async fn run<T>(future: impl Future<Output = Result<T, sqlx::Error>>) -> Result<T, Error> {
    match tokio::time::timeout(EXECUTION_TIMEOUT, future).await {
        Ok(result) => result.map_err(Error::database),
        Err(_) => Err(Error::database_timeout()),
    }
}

const EXPIRE: &str = "
WITH expired AS (
    SELECT profile, puzzle_id, fingerprint FROM progress
    WHERE updated_at < now() - make_interval(days => $1)
    ORDER BY updated_at
    LIMIT $2 FOR UPDATE SKIP LOCKED
)
DELETE FROM progress AS p USING expired AS e
WHERE (p.profile, p.puzzle_id, p.fingerprint) = (e.profile, e.puzzle_id, e.fingerprint)";

/// One short transaction. Concurrent saves/replicas can skip locked rows;
/// they remain eligible for a later batch rather than blocking the sweep.
pub async fn expire_batch(db: &PgPool, days: i32, batch_size: i64) -> Result<u64, Error> {
    run(sqlx::query(EXPIRE).bind(days).bind(batch_size).execute(db))
        .await
        .map(|result| result.rows_affected())
}

/// Deletes records whose best route was last improved more than `days`
/// ago: at startup (the first tick is immediate), then every hour.
async fn expire_progress(db: PgPool, days: i32, batch_size: i64) {
    let mut sweep = tokio::time::interval(RETENTION_SWEEP);
    loop {
        sweep.tick().await;
        let mut deleted = 0;
        for _ in 0..RETENTION_MAX_BATCHES {
            match expire_batch(&db, days, batch_size).await {
                Ok(count) => {
                    deleted += count;
                    if count < batch_size as u64 {
                        break;
                    }
                }
                Err(error) => {
                    eprintln!("Progress retention sweep failed: {}", error.1);
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
        if deleted > 0 {
            eprintln!("Deleted {deleted} progress records older than {days} days");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn migration_limits_come_last_so_they_win() {
        // PostgreSQL applies startup -c flags in order, so these zeros beat
        // PGOPTIONS or any flags added before them.
        let earlier = bounded_options(PgConnectOptions::new_without_pgpass());
        let options = migration_options(earlier);
        let flags = options.get_options().unwrap();
        assert!(
            flags.ends_with("-c statement_timeout=0 -c lock_timeout=0"),
            "{flags}"
        );
    }

    #[test]
    fn only_connection_failures_count_as_unreachable() {
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert!(unreachable(&sqlx::Error::Io(refused)));
        assert!(unreachable(&sqlx::Error::PoolTimedOut));
        assert!(unreachable(&sqlx::Error::PoolClosed));
        assert!(!unreachable(&sqlx::Error::RowNotFound));
        assert!(!unreachable(&sqlx::Error::Configuration("bad url".into())));
    }

    #[tokio::test]
    async fn execution_deadline_is_service_unavailable() {
        let started = std::time::Instant::now();
        let error = run(std::future::pending::<Result<(), sqlx::Error>>())
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.1, "PostgreSQL operation timed out; retry later");
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
