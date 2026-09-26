use crate::client::TrustedProxies;
use crate::limit::RateLimiter;
use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Request, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, de::DeserializeOwned};
use sokomind_core::Board;
use sqlx::PgPool;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

/// Under the frontend's 1.5 s health timeout even when the database hangs.
const HEALTH_TIMEOUT: Duration = Duration::from_millis(900);
/// Bodies are at most 128 KiB; a client still sending after this long is
/// holding a connection and a handler open, not uploading.
const BODY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct App {
    pub db: Option<PgPool>,
    pub catalog: Arc<HashMap<String, Board>>,
    pub slots: Arc<Semaphore>,
    pub progress_slots: Arc<Semaphore>,
    pub proxies: Arc<TrustedProxies>,
    pub saves: Arc<RateLimiter>,
    pub solves: Arc<RateLimiter>,
}
impl App {
    pub fn progress_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Error> {
        self.progress_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::too_many("Progress busy; retry later"))
    }
    pub fn db(&self) -> Result<&PgPool, Error> {
        self.db.as_ref().ok_or_else(Error::unavailable)
    }
    pub fn puzzle(&self, id: &str) -> Result<&Board, Error> {
        self.catalog
            .get(id)
            .ok_or_else(|| Error::new(StatusCode::NOT_FOUND, "Unknown catalog puzzle"))
    }
}
/// The fields of a `data/puzzles.json` entry the server needs; serde
/// ignores the rest.
#[derive(Deserialize)]
struct Puzzle {
    id: String,
    rows: Vec<String>,
}
/// Parses the embedded catalog once, keyed by puzzle id.
pub fn load_catalog() -> Result<HashMap<String, Board>, String> {
    let puzzles: Vec<Puzzle> = serde_json::from_str(include_str!("../../../data/puzzles.json"))
        .map_err(|error| format!("puzzle catalog: {error}"))?;
    let mut catalog = HashMap::with_capacity(puzzles.len());
    for Puzzle { id, rows } in puzzles {
        let board =
            Board::parse(&rows.join("\n")).map_err(|error| format!("puzzle {id}: {error}"))?;
        if catalog.insert(id.clone(), board).is_some() {
            return Err(format!("puzzle {id} appears twice in the catalog"));
        }
    }
    Ok(catalog)
}
/// JSON body extractor that keeps the API's `{error}` response shape for
/// malformed payloads instead of axum's plain-text rejections.
pub struct ApiJson<T>(pub T);
impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Error;
    async fn from_request(request: Request, state: &S) -> Result<Self, Error> {
        match tokio::time::timeout(BODY_TIMEOUT, Json::<T>::from_request(request, state)).await {
            Ok(Ok(Json(value))) => Ok(ApiJson(value)),
            Ok(Err(rejection)) => Err(Error::new(rejection.status(), rejection.body_text())),
            Err(_) => Err(Error::new(
                StatusCode::REQUEST_TIMEOUT,
                "Request body timed out",
            )),
        }
    }
}
/// Path extractor with the same `{error}` rejections.
pub struct ApiPath<T>(pub T);
impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Error> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(ApiPath(value)),
            Err(rejection) => Err(Error::new(rejection.status(), rejection.body_text())),
        }
    }
}
#[derive(Debug)]
pub struct Error(pub StatusCode, pub String);
impl Error {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self(status, message.into())
    }
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    pub fn too_many(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message)
    }
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "Unknown API endpoint")
    }
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "PostgreSQL persistence is unavailable",
        )
    }
    pub fn database_timeout() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "PostgreSQL operation timed out; retry later",
        )
    }
    /// Transient resource/lock failures can be retried, while malformed SQL
    /// and constraint errors remain internal failures.
    pub fn database(error: sqlx::Error) -> Self {
        if matches!(&error, sqlx::Error::Database(error) if error.code().is_some_and(|code| matches!(&*code, "57014" | "55P03" | "40P01")))
        {
            eprintln!("database operation interrupted: {error}");
            Self::database_timeout()
        } else if db_unreachable(&error) {
            eprintln!("database unavailable: {error}");
            Self::unavailable()
        } else {
            Self::internal(error)
        }
    }
    pub fn internal(message: impl std::fmt::Display) -> Self {
        eprintln!("request failed: {message}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Server operation failed")
    }
}
/// Failures that mean the database cannot be reached right now (refused,
/// reset, restarting, saturated) rather than that a query is wrong.
pub fn db_unreachable(error: &sqlx::Error) -> bool {
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
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}
pub async fn health(State(app): State<App>) -> Json<serde_json::Value> {
    // A slow probe is indistinguishable from a dead one to the frontend.
    let persistence = if let Some(db) = &app.db {
        let Ok(_permit) = app.progress_permit() else {
            return Json(serde_json::json!({ "status": "ok", "persistence": false }));
        };
        tokio::time::timeout(HEALTH_TIMEOUT, sqlx::query("SELECT 1").execute(db))
            .await
            .map(|result| result.is_ok())
            .unwrap_or(false)
    } else {
        false
    };
    Json(serde_json::json!({ "status": "ok", "persistence": persistence }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[derive(Deserialize)]
    struct Shape {
        route: String,
    }

    async fn extract(content_type: Option<&str>, body: &'static str) -> Result<Shape, StatusCode> {
        let mut request = axum::http::Request::builder().method("POST").uri("/");
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        let request = request.body(Body::from(body)).unwrap();
        match ApiJson::<Shape>::from_request(request, &()).await {
            Ok(ApiJson(shape)) => Ok(shape),
            Err(error) => Err(error.0),
        }
    }

    #[tokio::test]
    async fn json_rejections_keep_their_status() {
        let json = Some("application/json");
        assert_eq!(
            extract(json, r#"{"route":"UD"}"#).await.unwrap().route,
            "UD"
        );
        let unsupported = Some(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(extract(None, r#"{"route":"UD"}"#).await.err(), unsupported);
        assert_eq!(
            extract(Some("text/plain"), r#"{"route":"UD"}"#).await.err(),
            unsupported
        );
        assert_eq!(
            extract(json, "{").await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        let unprocessable = Some(StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(extract(json, r#"{"route":1}"#).await.err(), unprocessable);
        assert_eq!(extract(json, "{}").await.err(), unprocessable);
    }

    #[test]
    fn every_embedded_puzzle_parses() {
        let catalog = load_catalog().unwrap();
        assert!(!catalog.is_empty());
        assert!(
            catalog
                .values()
                .all(|board| !board.fingerprint().is_empty())
        );
    }

    #[test]
    fn unreachable_databases_are_503_and_failed_queries_500() {
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert!(db_unreachable(&sqlx::Error::Io(refused)));
        assert!(db_unreachable(&sqlx::Error::PoolTimedOut));
        assert!(!db_unreachable(&sqlx::Error::RowNotFound));
        assert!(!db_unreachable(&sqlx::Error::Configuration(
            "bad url".into()
        )));
        let closed = Error::database(sqlx::Error::PoolClosed);
        assert_eq!(closed.0, StatusCode::SERVICE_UNAVAILABLE);
        let failed = Error::database(sqlx::Error::RowNotFound);
        assert_eq!(failed.0, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
