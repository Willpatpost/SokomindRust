use crate::api::{ApiJson, ApiPath, App, Error};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sokomind_core::{Board, Game};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Save {
    route: String,
}

const LIST: &str = "
SELECT puzzle_id, moves, pushes, fingerprint FROM progress
WHERE profile = $1 ORDER BY puzzle_id";
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

fn current_fingerprint(app: &App, id: &str) -> Option<String> {
    app.catalog
        .iter()
        .find(|puzzle| puzzle.id == id)
        .and_then(|puzzle| Board::parse(&puzzle.rows.join("\n")).ok())
        .map(|board| board.fingerprint)
}

pub async fn list(State(app): State<App>, headers: HeaderMap) -> Result<Json<Vec<Record>>, Error> {
    let profile = profile(&headers)?;
    let db = app.db.as_ref().ok_or_else(Error::unavailable)?;
    let rows = sqlx::query(LIST)
        .bind(profile)
        .fetch_all(db)
        .await
        .map_err(Error::database)?;
    // A catalog layout change retires old records: they cannot be beaten by
    // fresh saves and their routes no longer replay.
    let records = rows
        .into_iter()
        .filter(|row| {
            current_fingerprint(&app, &row.get::<String, _>("puzzle_id"))
                .is_some_and(|current| current == row.get::<String, _>("fingerprint"))
        })
        .map(|row| Record {
            puzzle_id: row.get("puzzle_id"),
            moves: row.get("moves"),
            pushes: row.get("pushes"),
            route: None,
        })
        .collect();
    Ok(Json(records))
}
pub async fn get(
    State(app): State<App>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<Record>, Error> {
    let profile = profile(&headers)?;
    let db = app.db.as_ref().ok_or_else(Error::unavailable)?;
    let Some(fingerprint) = current_fingerprint(&app, &id) else {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "Unknown catalog puzzle".into(),
        ));
    };
    let row = sqlx::query(FETCH)
        .bind(profile)
        .bind(&id)
        .bind(&fingerprint)
        .fetch_optional(db)
        .await
        .map_err(Error::database)?
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
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<Save>,
) -> Result<Json<serde_json::Value>, Error> {
    let profile = profile(&headers)?;
    if body.route.len() > MAX_SAVED_ROUTE {
        return Err(Error::bad("Route exceeds 10000 moves"));
    }
    let Some(puzzle) = app.catalog.iter().find(|p| p.id == id) else {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "Unknown catalog puzzle".into(),
        ));
    };
    let db = app.db.as_ref().ok_or_else(Error::unavailable)?;
    // Charged after the cheap checks: the budget guards replays and writes,
    // so malformed or misaddressed saves should not spend it.
    if !app.saves.allow(app.proxies.client(&headers, peer.ip())) {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many saves; try again shortly".into(),
        ));
    }
    let rows = puzzle.rows.join("\n");
    let route = body.route;
    let replay_route = route.clone();
    // Replay is pure CPU: keep it off the async runtime, like the solver.
    let verified = tokio::task::spawn_blocking(move || {
        let mut game = Game::new(Board::parse(&rows)?);
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
