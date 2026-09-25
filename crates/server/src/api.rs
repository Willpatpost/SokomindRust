use crate::client::TrustedProxies;
use crate::limit::RateLimiter;
use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Request, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

/// Under the frontend's 1.5 s health timeout even when the database hangs.
const HEALTH_TIMEOUT: Duration = Duration::from_millis(900);
/// Bodies are at most 128 KiB; a client still sending after this long is
/// holding a connection and a handler open, not uploading.
const BODY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct App {
    pub db: Option<PgPool>,
    pub catalog: Arc<Vec<Puzzle>>,
    pub slots: Arc<Semaphore>,
    pub proxies: Arc<TrustedProxies>,
    pub saves: Arc<RateLimiter>,
    pub solves: Arc<RateLimiter>,
}
#[derive(Serialize, Deserialize)]
pub struct Puzzle {
    pub id: String,
    pub title: String,
    pub difficulty: String,
    pub boxes: u32,
    pub rows: Vec<String>,
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    #[serde(
        rename = "generationMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub generation_mode: Option<String>,
    #[serde(
        rename = "topologyFamily",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub topology_family: Option<String>,
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
            Ok(Err(rejection)) => Err(Error(rejection.status(), rejection.body_text())),
            Err(_) => Err(Error(
                StatusCode::REQUEST_TIMEOUT,
                "Request body timed out".into(),
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
            Err(rejection) => Err(Error(rejection.status(), rejection.body_text())),
        }
    }
}
pub struct Error(pub StatusCode, pub String);
impl Error {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    pub fn not_found() -> Self {
        Self(StatusCode::NOT_FOUND, "Unknown API endpoint".into())
    }
    pub fn unavailable() -> Self {
        Self(
            StatusCode::SERVICE_UNAVAILABLE,
            "PostgreSQL persistence is unavailable".into(),
        )
    }
    /// An unreachable database is the same 503 as an unconfigured one;
    /// any other database failure is a server bug.
    pub fn database(error: sqlx::Error) -> Self {
        if db_unreachable(&error) {
            eprintln!("database unavailable: {error}");
            Self::unavailable()
        } else {
            Self::internal(error)
        }
    }
    pub fn internal(message: impl std::fmt::Display) -> Self {
        eprintln!("request failed: {message}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Server operation failed".into(),
        )
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
        tokio::time::timeout(HEALTH_TIMEOUT, sqlx::query("SELECT 1").execute(db))
            .await
            .map(|result| result.is_ok())
            .unwrap_or(false)
    } else {
        false
    };
    Json(serde_json::json!({ "status": "ok", "persistence": persistence }))
}
pub async fn catalog(State(app): State<App>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&*app.catalog).expect("catalog serialization"))
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
