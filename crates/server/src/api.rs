use axum::{
    Json,
    extract::State,
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
}
#[derive(Serialize, Deserialize)]
pub struct Puzzle {
    pub id: String,
    pub title: String,
    pub difficulty: String,
    pub boxes: u32,
    pub rows: Vec<String>,
    pub hint: Option<String>,
}
pub struct Error(pub StatusCode, pub String);
impl Error {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
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
    let persistence = if let Some(db) = &app.db {
        sqlx::query("SELECT 1").execute(db).await.is_ok()
    } else {
        false
    };
    Json(serde_json::json!({ "status": "ok", "persistence": persistence }))
}
pub async fn catalog(State(app): State<App>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&*app.catalog).expect("catalog serialization"))
}
