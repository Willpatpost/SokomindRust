use crate::api::{App, Error};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sokomind_core::{Board, Game};
use sqlx::Row;

fn profile(headers: &HeaderMap) -> Result<&str, Error> {
    let value = headers
        .get("x-profile-id")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| Error::bad("Missing x-profile-id"))?;
    if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::bad("Profile must be 32 hexadecimal characters"));
    }
    Ok(value)
}
#[derive(Serialize)]
pub struct Record {
    puzzle_id: String,
    moves: i32,
    pushes: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Save {
    route: String,
}

pub async fn list(State(app): State<App>, headers: HeaderMap) -> Result<Json<Vec<Record>>, Error> {
    let profile = profile(&headers)?;
    let db = app.db.as_ref().ok_or_else(Error::unavailable)?;
    let rows = sqlx::query(
        "SELECT puzzle_id, moves, pushes FROM progress WHERE profile = $1 ORDER BY puzzle_id",
    )
    .bind(profile)
    .fetch_all(db)
    .await
    .map_err(Error::internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|r| Record {
                puzzle_id: r.get("puzzle_id"),
                moves: r.get("moves"),
                pushes: r.get("pushes"),
                route: None,
            })
            .collect(),
    ))
}
pub async fn get(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Record>, Error> {
    let profile = profile(&headers)?;
    let db = app.db.as_ref().ok_or_else(Error::unavailable)?;
    let row = sqlx::query("SELECT puzzle_id, moves, pushes, route FROM progress WHERE profile = $1 AND puzzle_id = $2").bind(profile).bind(id).fetch_optional(db).await.map_err(Error::internal)?
        .ok_or_else(|| Error(StatusCode::NOT_FOUND, "No saved route".into()))?;
    Ok(Json(Record {
        puzzle_id: row.get("puzzle_id"),
        moves: row.get("moves"),
        pushes: row.get("pushes"),
        route: Some(row.get("route")),
    }))
}
pub async fn save(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Save>,
) -> Result<Json<serde_json::Value>, Error> {
    let profile = profile(&headers)?;
    let db = app.db.as_ref().ok_or_else(Error::unavailable)?;
    let puzzle = app
        .catalog
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| Error(StatusCode::NOT_FOUND, "Unknown catalog puzzle".into()))?;
    let mut game = Game::new(Board::parse(&puzzle.rows.join("\n")).map_err(Error::internal)?);
    // Never trust client counters, box coordinates, solved flags, or optimality claims.
    game.replay(&body.route).map_err(Error::bad)?;
    if !game.solved() {
        return Err(Error::bad("Route does not solve the puzzle"));
    }
    let result = sqlx::query("INSERT INTO progress (profile, puzzle_id, moves, pushes, route) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (profile,puzzle_id) DO UPDATE SET moves=EXCLUDED.moves, pushes=EXCLUDED.pushes, route=EXCLUDED.route, updated_at=now() WHERE (EXCLUDED.moves,EXCLUDED.pushes) < (progress.moves,progress.pushes)")
        .bind(profile).bind(id).bind(game.moves() as i32).bind(game.pushes as i32).bind(body.route).execute(db).await.map_err(Error::internal)?;
    Ok(Json(
        serde_json::json!({ "saved": true, "improved": result.rows_affected() > 0 }),
    ))
}
