use crate::api::{ApiJson, ApiPath, App, Error};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sokomind_core::Game;
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
#[derive(Serialize, sqlx::FromRow)]
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
    let record = sqlx::query_as::<_, Record>(FETCH)
        .bind(profile)
        .bind(&id)
        .bind(fingerprint)
        .fetch_optional(db)
        .await
        .map_err(Error::database)?
        .ok_or_else(|| Error::new(StatusCode::NOT_FOUND, "No saved route"))?;
    Ok(Json(record))
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
    // Replay is pure CPU: keep it off the async runtime, like the solver.
    let (moves, pushes, fingerprint, route) = tokio::task::spawn_blocking(move || {
        let mut game = Game::new(board);
        // Never trust client counters, box coordinates, solved flags, or optimality claims.
        game.replay(&route)?;
        if !game.solved() {
            return Err("Route does not solve the puzzle".to_string());
        }
        let (moves, pushes) = (game.moves() as i32, game.pushes() as i32);
        Ok((moves, pushes, game.into_parts().0.fingerprint, route))
    })
    .await
    .map_err(Error::internal)?
    .map_err(Error::bad)?;
    let result = sqlx::query(UPSERT)
        .bind(profile)
        .bind(id)
        .bind(fingerprint)
        .bind(moves)
        .bind(pushes)
        .bind(route)
        .execute(db)
        .await
        .map_err(Error::database)?;
    Ok(Json(
        serde_json::json!({ "saved": true, "improved": result.rows_affected() > 0 }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn profile_needs_32_hex_characters() {
        const HEX: &[u8] = b"0123456789abcdefABCDEF0123456789";
        const MISSING: &str = "Missing x-profile-id";
        const MALFORMED: &str = "Profile must be 32 hexadecimal characters";
        let cases: [(Option<&[u8]>, Result<&str, &str>); 7] = [
            (None, Err(MISSING)),
            (Some(HEX), Ok("0123456789abcdefABCDEF0123456789")),
            (Some(&HEX[1..]), Err(MALFORMED)),
            (
                Some(b"0123456789abcdef0123456789abcdefa".as_slice()),
                Err(MALFORMED),
            ),
            (
                Some(b"0123456789abcdef0123456789abcdeg".as_slice()),
                Err(MALFORMED),
            ),
            (Some(b"".as_slice()), Err(MALFORMED)),
            // Not visible ASCII, so to_str() fails: treated as missing.
            (
                Some(b"0123456789abcdef0123456789abcde\xe9".as_slice()),
                Err(MISSING),
            ),
        ];
        for (value, expected) in cases {
            let mut headers = HeaderMap::new();
            if let Some(value) = value {
                headers.insert("x-profile-id", HeaderValue::from_bytes(value).unwrap());
            }
            match (profile(&headers), expected) {
                (Ok(got), Ok(want)) => assert_eq!(got, want),
                (Err(error), Err(want)) => {
                    assert_eq!(error.0, StatusCode::BAD_REQUEST);
                    assert_eq!(error.1, want);
                }
                _ => panic!("unexpected result for {value:?}"),
            }
        }
    }
}
