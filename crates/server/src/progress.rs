use crate::api::{ApiJson, ApiPath, App, Error};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sokomind_core::Game;
use sqlx::Row;
use std::net::SocketAddr;

/// Twenty times the longest route the catalog can need, bounding stored
/// rows to ~10 KB; anything longer is wandering, not a best route.
const MAX_SAVED_ROUTE: usize = 10_000;

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
    route: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Save {
    route: String,
}

const FETCH: &str = "
SELECT puzzle_id, moves, pushes, route FROM progress
WHERE profile = $1 AND puzzle_id = $2 AND fingerprint = $3";
const UPSERT: &str = "
INSERT INTO progress (profile, puzzle_id, fingerprint, moves, pushes, route)
VALUES ($1, $2, $3, $4, $5, $6)
ON CONFLICT (profile, puzzle_id, fingerprint) DO UPDATE
SET moves = EXCLUDED.moves,
    pushes = EXCLUDED.pushes,
    route = EXCLUDED.route,
    updated_at = now()
WHERE (EXCLUDED.moves, EXCLUDED.pushes) < (progress.moves, progress.pushes)";

pub async fn get(
    State(app): State<App>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<Record>, Error> {
    let profile = profile(&headers)?;
    let db = app.db()?;
    // A catalog layout change retires old records: they cannot be beaten by
    // fresh saves and their routes no longer replay.
    let fingerprint = &app.puzzle(&id)?.fingerprint;
    let row = sqlx::query(FETCH)
        .bind(profile)
        .bind(&id)
        .bind(fingerprint)
        .fetch_optional(db)
        .await
        .map_err(Error::database)?
        .ok_or_else(|| Error::new(StatusCode::NOT_FOUND, "No saved route"))?;
    Ok(Json(Record {
        puzzle_id: row.get("puzzle_id"),
        moves: row.get("moves"),
        pushes: row.get("pushes"),
        route: row.get("route"),
    }))
}
pub async fn save(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<Save>,
) -> Result<Json<serde_json::Value>, Error> {
    let profile = profile(&headers)?;
    if body.route.len() > MAX_SAVED_ROUTE {
        return Err(Error::bad(format!("Route exceeds {MAX_SAVED_ROUTE} moves")));
    }
    let board = app.puzzle(&id)?.clone();
    let db = app.db()?;
    // Charged after the cheap checks: the budget guards replays and writes,
    // so malformed or misaddressed saves should not spend it.
    if !app.saves.allow(app.proxies.client(&headers, peer.ip())) {
        return Err(Error::too_many("Too many saves; try again shortly"));
    }
    let route = body.route;
    let replay_route = route.clone();
    // Replay is pure CPU: keep it off the async runtime, like the solver.
    let verified = tokio::task::spawn_blocking(move || {
        let mut game = Game::new(board);
        // Never trust client counters, box coordinates, solved flags, or optimality claims.
        game.replay(&replay_route)?;
        if !game.solved() {
            return Err("Route does not solve the puzzle".to_string());
        }
        Ok((
            game.moves() as i32,
            game.pushes as i32,
            game.board.fingerprint.clone(),
        ))
    })
    .await
    .map_err(Error::internal)?
    .map_err(Error::bad)?;
    let result = sqlx::query(UPSERT)
        .bind(profile)
        .bind(id)
        .bind(verified.2)
        .bind(verified.0)
        .bind(verified.1)
        .bind(route)
        .execute(db)
        .await
        .map_err(Error::database)?;
    Ok(Json(
        serde_json::json!({ "saved": true, "improved": result.rows_affected() > 0 }),
    ))
}
