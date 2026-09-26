use crate::api::Error;
use sqlx::{PgPool, postgres::PgConnectOptions};
use std::{future::Future, time::Duration};

pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(1);
pub const EXECUTION_TIMEOUT: Duration = Duration::from_millis(2500);

/// Per-session limits also stop SQL after an HTTP client drops its request.
/// Keep lock_timeout below statement_timeout, and both below our deadline.
pub fn bounded_options(options: PgConnectOptions) -> PgConnectOptions {
    options.options([("statement_timeout", "1500ms"), ("lock_timeout", "500ms")])
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

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
