//! PostgreSQL: migrations, the bounded request pool and retention.
use crate::{api::Error, config};
use sqlx::{
    PgPool,
    migrate::{MigrateError, Migrator},
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::time::{Duration, Instant};

/// How long a pool waits for a connection; a request that waits longer
/// fails with 503.
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(1);
/// The deadline [`run`] gives one request's database work, pool acquisition
/// included.
pub const EXECUTION_TIMEOUT: Duration = Duration::from_millis(2500);
/// The longest one request-pool statement may run.
pub const STATEMENT_TIMEOUT: Duration = Duration::from_millis(1500);
/// The longest one request-pool statement may wait for a lock.
pub const LOCK_TIMEOUT: Duration = Duration::from_millis(500);
// Each limit must sit below the next: a lock wait is part of a statement, so
// a longer lock_timeout would never fire, and the database should end a
// statement itself before `run` stops waiting and leaves it running.
const _: () = assert!(
    LOCK_TIMEOUT.as_millis() < STATEMENT_TIMEOUT.as_millis()
        && STATEMENT_TIMEOUT.as_millis() < EXECUTION_TIMEOUT.as_millis()
);
/// Direct runs can race the database startup; retry briefly so compose's
/// restart policy is a backstop, not the only defense.
const RETRY_WINDOW: Duration = Duration::from_secs(30);
const RETRY_PAUSE: Duration = Duration::from_secs(1);
const RETENTION_SWEEP: Duration = Duration::from_secs(3600);
/// Batches per sweep, so a large backlog drains over several sweeps instead
/// of in one long burst of deletes: each replica deletes at most this many
/// times PROGRESS_RETENTION_BATCH_SIZE records per `RETENTION_SWEEP`.
/// README's Configuration table and .env.example quote this cap; change them
/// with it.
pub const RETENTION_MAX_BATCHES: usize = 20;
/// statement_timeout, or a cancel request, stopped the statement.
const QUERY_CANCELED: &str = "57014";
/// lock_timeout ran out while the statement waited for a lock.
const LOCK_NOT_AVAILABLE: &str = "55P03";
/// The database ended the statement to break a deadlock.
const DEADLOCK_DETECTED: &str = "40P01";

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

/// Why a database operation stopped short in a way a retry may get past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    /// A statement, lock or execution deadline ran out.
    TimedOut,
    /// A concurrent transaction deadlocked with it.
    Conflict,
}

/// Why the database stopped `error`'s statement, if it did, rather than
/// rejecting it.
pub fn interrupted(error: &sqlx::Error) -> Option<Interruption> {
    let sqlx::Error::Database(error) = error else {
        return None;
    };
    match &*error.code()? {
        QUERY_CANCELED | LOCK_NOT_AVAILABLE => Some(Interruption::TimedOut),
        DEADLOCK_DETECTED => Some(Interruption::Conflict),
        _ => None,
    }
}

/// Sets [`STATEMENT_TIMEOUT`] and [`LOCK_TIMEOUT`] as per-session limits,
/// which also stop SQL after an HTTP client drops its request.
pub fn bounded_options(options: PgConnectOptions) -> PgConnectOptions {
    let statement = format!("{}ms", STATEMENT_TIMEOUT.as_millis());
    let lock = format!("{}ms", LOCK_TIMEOUT.as_millis());
    options.options([
        ("statement_timeout", statement.as_str()),
        ("lock_timeout", lock.as_str()),
    ])
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

/// The schema migrations, embedded from migrations/ at build time.
pub static MIGRATIONS: Migrator = sqlx::migrate!("../../migrations");

/// Applies pending migrations through `migrator`, a one-connection pool
/// opened with [`migration_options`], then closes it whatever the result.
/// Closing ends the session, which releases sqlx's advisory lock even when a
/// failed migration left it held.
pub async fn migrate(migrator: PgPool) -> Result<(), MigrateError> {
    let result = MIGRATIONS.run(&migrator).await;
    migrator.close().await;
    result
}

/// Runs `future` under the request deadline, [`EXECUTION_TIMEOUT`], which
/// covers pool acquisition, execution and response decoding. On expiry the
/// future is dropped and the result is [`Error::interrupted`], a 503; any
/// other failure goes through [`Error::database`]. There is no retry here:
/// callers must not create a second write after an uncertain result.
pub async fn run<T>(future: impl Future<Output = Result<T, sqlx::Error>>) -> Result<T, Error> {
    match tokio::time::timeout(EXECUTION_TIMEOUT, future).await {
        Ok(result) => result.map_err(Error::database),
        Err(_) => Err(Error::interrupted(Interruption::TimedOut)),
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

/// One statement in its own short transaction. It skips rows that
/// concurrent saves or another replica's sweep hold locked (SKIP LOCKED);
/// those stay eligible for a later batch instead of blocking this one.
pub async fn expire_batch(db: &PgPool, days: i32, batch_size: i64) -> Result<u64, Error> {
    run(sqlx::query(EXPIRE).bind(days).bind(batch_size).execute(db))
        .await
        .map(|result| result.rows_affected())
}

/// One retention sweep: [`expire_batch`] batches of up to `batch_size`
/// records until one comes back short, one fails, or
/// [`RETENTION_MAX_BATCHES`] full batches have run. Returns the records
/// deleted and whether the cap ended the sweep; a failure is logged and
/// ends it with what it deleted so far.
pub async fn sweep(db: &PgPool, days: i32, batch_size: i64) -> (u64, bool) {
    let mut deleted = 0;
    for _ in 0..RETENTION_MAX_BATCHES {
        match expire_batch(db, days, batch_size).await {
            Ok(count) => {
                deleted += count;
                if count < batch_size as u64 {
                    return (deleted, false);
                }
            }
            Err(error) => {
                eprintln!("Progress retention sweep failed: {}", error.message);
                return (deleted, false);
            }
        }
        tokio::task::yield_now().await;
    }
    (deleted, true)
}

/// Deletes records whose best route was last improved more than `days`
/// ago: at startup (the first tick is immediate), then every hour.
async fn expire_progress(db: PgPool, days: i32, batch_size: i64) {
    let mut hourly = tokio::time::interval(RETENTION_SWEEP);
    loop {
        hourly.tick().await;
        let (deleted, capped) = sweep(&db, days, batch_size).await;
        if deleted > 0 {
            eprintln!("Deleted {deleted} progress records older than {days} days");
        }
        if capped {
            eprintln!(
                "Progress retention sweep reached its cap of {RETENTION_MAX_BATCHES} batches; any remaining expired records wait for the next sweep"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use sqlx::error::{DatabaseError, ErrorKind};
    use std::{borrow::Cow, error::Error as StdError, fmt};

    /// A database error that carries only its SQLSTATE, which is all
    /// `interrupted` and `unreachable` read.
    #[derive(Debug)]
    struct Code(&'static str);
    impl fmt::Display for Code {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "SQLSTATE {}", self.0)
        }
    }
    impl StdError for Code {}
    impl DatabaseError for Code {
        fn message(&self) -> &str {
            self.0
        }
        fn code(&self) -> Option<Cow<'_, str>> {
            Some(self.0.into())
        }
        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }
    fn sqlstate(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(Code(code)))
    }

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

    #[test]
    fn only_database_errors_are_interruptions() {
        assert_eq!(interrupted(&sqlx::Error::PoolTimedOut), None);
        assert_eq!(interrupted(&sqlx::Error::RowNotFound), None);
    }

    #[test]
    fn sqlstates_name_their_interruption() {
        let timed_out = Some(Interruption::TimedOut);
        assert_eq!(interrupted(&sqlstate("57014")), timed_out);
        assert_eq!(interrupted(&sqlstate("55P03")), timed_out);
        assert_eq!(
            interrupted(&sqlstate("40P01")),
            Some(Interruption::Conflict)
        );
        assert_eq!(interrupted(&sqlstate("23505")), None);
    }

    #[test]
    fn database_errors_answer_by_sqlstate() {
        let conflict = Error::database(sqlstate("40P01"));
        assert_eq!(conflict.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            conflict.message,
            "PostgreSQL operation conflicted with a concurrent one; retry later"
        );
        let down = Error::database(sqlstate("08006"));
        assert_eq!(down.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(down.message, "PostgreSQL persistence is unavailable");
        let violation = Error::database(sqlstate("23505"));
        assert_eq!(violation.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// The clock is paused, so tokio skips straight to the deadline: the
    /// test takes no real time and sees `EXECUTION_TIMEOUT` pass, give or
    /// take the timer's 1 ms rounding.
    #[tokio::test(start_paused = true)]
    async fn execution_deadline_is_service_unavailable() {
        let started = tokio::time::Instant::now();
        let error = run(std::future::pending::<Result<(), sqlx::Error>>())
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.message, "PostgreSQL operation timed out; retry later");
        let elapsed = started.elapsed();
        let deadline = EXECUTION_TIMEOUT..=EXECUTION_TIMEOUT + Duration::from_millis(1);
        assert!(deadline.contains(&elapsed), "{elapsed:?}");
    }

    /// A batch that fails ends the sweep at once, with nothing deleted and
    /// the cap not reached; a closed pool fails every batch without waiting.
    #[tokio::test]
    async fn a_failed_batch_ends_the_sweep() {
        let closed = PgPoolOptions::new()
            .connect_lazy("postgres://invalid@127.0.0.1:1/invalid")
            .unwrap();
        closed.close().await;
        assert_eq!(sweep(&closed, 30, 500).await, (0, false));
    }
}
