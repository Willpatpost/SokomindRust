//! State the handlers share, the `{error}` type every failure answers with,
//! the JSON extractor that keeps that shape, and /api/health.
use crate::client::TrustedProxies;
use crate::config::Config;
use crate::limit::RateLimiter;
use axum::{
    Json,
    extract::{FromRequest, Request},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Bounds slow request bodies on direct runs: bodies are at most 128 KiB
/// (`BODY_LIMIT` in crates/server/src/main.rs), so a client still sending
/// after this long is holding a connection and a handler open, not
/// uploading. Behind nginx it never fires, because nginx reads each whole
/// body before passing the request on.
pub const BODY_TIMEOUT: Duration = Duration::from_secs(10);
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// What every handler shares. Each request gets a clone, which shares the
/// slots and budgets behind it rather than copying them.
#[derive(Clone)]
pub struct App {
    /// One permit per native solve that may run at once.
    pub solve_slots: Arc<Semaphore>,
    /// The peers trusted to name, through X-Forwarded-For, the client a
    /// budget charges.
    pub proxies: Arc<TrustedProxies>,
    /// Each client's native solve budget, `SOLVE_RATE_PER_MINUTE`.
    pub solves: Arc<RateLimiter>,
}
impl App {
    /// The state every handler shares: the slots and budgets `config` sets.
    pub fn new(config: Config) -> Self {
        Self {
            solve_slots: Arc::new(Semaphore::new(config.solve_concurrency as usize)),
            proxies: Arc::new(config.proxies),
            solves: Arc::new(RateLimiter::new(config.solve_rate, RATE_WINDOW)),
        }
    }
    /// A free solve slot, held until the permit drops, or 429 at once when
    /// none is free: nothing queues, so a busy server answers without
    /// holding the request.
    pub fn solve_permit(&self) -> Result<OwnedSemaphorePermit, Error> {
        Arc::clone(&self.solve_slots)
            .try_acquire_owned()
            .map_err(|_| Error::too_many("Solver busy; try the browser solver or retry later"))
    }
    /// Spends one request of `budget` for the client that `headers` and
    /// `peer` name, or answers 429 with `refused` when that client has none
    /// left. Handlers charge last, so a request refused for any other reason
    /// spends nothing.
    pub fn charge(
        &self,
        budget: &RateLimiter,
        headers: &HeaderMap,
        peer: SocketAddr,
        refused: &str,
    ) -> Result<(), Error> {
        if budget.allow(self.proxies.client(headers, peer.ip())) {
            Ok(())
        } else {
            Err(Error::too_many(refused))
        }
    }
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
/// A failed request: its status and the message its `{error}` body carries.
#[derive(Debug)]
pub struct Error {
    /// The response status.
    pub status: StatusCode,
    /// The body's `error`, written for the person using the page.
    pub message: String,
}
impl Error {
    /// `status` with `message` as its `error`.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    /// 400: the request itself is invalid.
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    /// 429: every slot is busy, or the client's budget is spent.
    pub fn too_many(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message)
    }
    /// 404 for a path no route matches.
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "Unknown API endpoint")
    }
    /// 500 for a server bug. `message` goes to stderr, never to the client,
    /// which gets a fixed message.
    pub fn internal(message: impl std::fmt::Display) -> Self {
        eprintln!("request failed: {message}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Server operation failed")
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}
/// `GET /api/health`: always 200 `{"status": "ok"}` while the API answers.
pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[derive(serde::Deserialize)]
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
            Err(error) => Err(error.status),
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
}
