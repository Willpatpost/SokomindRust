use crate::limit::SaveLimiter;
use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct App {
    pub db: Option<PgPool>,
    pub catalog: Arc<Vec<Puzzle>>,
    pub slots: Arc<Semaphore>,
    pub saves: Arc<SaveLimiter>,
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
impl<T, S> axum::extract::FromRequest<S> for ApiJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Error;
    async fn from_request(request: Request, state: &S) -> Result<Self, Error> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(Error::bad(rejection.body_text())),
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
    pub fn internal(message: impl std::fmt::Display) -> Self {
        eprintln!("request failed: {message}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Server operation failed".into(),
        )
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}
pub async fn health(State(app): State<App>) -> Json<serde_json::Value> {
    // Stay under the frontend's 1.5 s health timeout even when the database
    // is down: a slow probe is indistinguishable from a dead one.
    let persistence = if let Some(db) = &app.db {
        tokio::time::timeout(
            std::time::Duration::from_millis(900),
            sqlx::query("SELECT 1").execute(db),
        )
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
