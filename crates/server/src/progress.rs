//! `/api/progress/{id}`: a browser profile's best route per catalog puzzle.
use crate::api::{ApiJson, ApiPath, App, Error};
use crate::database;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sokomind_core::{Game, MAX_ROUTE};
use std::net::SocketAddr;

/// About ten times the longest known solution to a catalog puzzle (988
/// moves), bounding stored rows to ~10 KB; anything longer is wandering, not
/// a best route. It must stay within core's `MAX_ROUTE`, which the progress
/// table's CHECK mirrors.
const MAX_SAVED_ROUTE: usize = 10_000;
const _: () = assert!(MAX_SAVED_ROUTE <= MAX_ROUTE);

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
/// A saved best route, as the progress table holds it and a read returns
/// it.
#[derive(Serialize, sqlx::FromRow)]
pub struct Record {
    /// The catalog puzzle's id.
    puzzle_id: String,
    /// The route's moves, counted by the server's replay.
    moves: i32,
    /// The route's pushes, counted the same way.
    pushes: i32,
    /// The route as `UDLR` letters from the puzzle's start.
    route: String,
}
/// A save's JSON body: only the route, since the server replays it to count
/// moves and pushes and trusts no client counter.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Save {
    /// The route as `UDLR` letters from the puzzle's start, at most
    /// [`MAX_SAVED_ROUTE`] of them.
    route: String,
}

const FETCH: &str = "
SELECT puzzle_id, moves, pushes, route FROM progress
WHERE profile = $1 AND puzzle_id = $2 AND fingerprint = $3";
pub(crate) const UPSERT: &str = "
INSERT INTO progress (profile, puzzle_id, fingerprint, moves, pushes, route)
VALUES ($1, $2, $3, $4, $5, $6)
ON CONFLICT (profile, puzzle_id, fingerprint) DO UPDATE
SET moves = EXCLUDED.moves,
    pushes = EXCLUDED.pushes,
    route = EXCLUDED.route,
    updated_at = now()
WHERE (EXCLUDED.moves, EXCLUDED.pushes) < (progress.moves, progress.pushes)";

/// `GET /api/progress/{id}`: the `x-profile-id` profile's [`Record`] for the
/// catalog's current layout of `id`, or 404 when it has none. Admission
/// follows the crate's shared order and spends no rate budget.
pub async fn get(
    State(app): State<App>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<Record>, Error> {
    let profile = profile(&headers)?;
    // A catalog layout change retires old records: they cannot be beaten by
    // fresh saves and their routes no longer replay.
    let fingerprint = app.puzzle(&id)?.fingerprint();
    let db = app.db()?;
    let _permit = app.progress_permit()?;
    let record = database::run(
        sqlx::query_as::<_, Record>(FETCH)
            .bind(profile)
            .bind(&id)
            .bind(fingerprint)
            .fetch_optional(db),
    )
    .await?
    .ok_or_else(|| Error::new(StatusCode::NOT_FOUND, "No saved route"))?;
    Ok(Json(record))
}
/// `POST /api/progress/{id}`: replays the [`Save`] route on `id` and keeps it
/// when it has fewer moves, then fewer pushes, than the profile's saved one.
/// Answers `{"saved": true, "improved": bool}`; admission follows the
/// crate's shared order, charging the save budget last.
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
    let permit = app.progress_permit()?;
    // Charged last: the budget guards replays and writes, so malformed,
    // misaddressed or busy saves should not spend it.
    app.charge(
        &app.saves,
        &headers,
        peer,
        "Too many saves; try again shortly",
    )?;
    let route = body.route;
    // Replay is pure CPU: keep it off the async runtime, like the solver.
    let (_permit, moves, pushes, fingerprint, route) = tokio::task::spawn_blocking(move || {
        // If the HTTP future is dropped, the blocking job retains admission
        // until it finishes. Successful jobs return it across the DB write.
        let mut game = Game::new(board);
        // Never trust client counters, box coordinates, solved flags, or optimality claims.
        game.replay(&route).map_err(|error| error.to_string())?;
        if !game.solved() {
            return Err("Route does not solve the puzzle".to_string());
        }
        let (moves, pushes) = (game.moves() as i32, game.pushes() as i32);
        Ok((
            permit,
            moves,
            pushes,
            game.board().fingerprint().to_owned(),
            route,
        ))
    })
    .await
    .map_err(Error::internal)?
    .map_err(Error::bad)?;
    let result = database::run(
        sqlx::query(UPSERT)
            .bind(profile)
            .bind(id)
            .bind(fingerprint)
            .bind(moves)
            .bind(pushes)
            .bind(route)
            .execute(db),
    )
    .await?;
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
        type ProfileCase<'a> = (Option<&'a [u8]>, Result<&'a str, &'a str>);
        let cases: [ProfileCase<'_>; 7] = [
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
                    assert_eq!(error.status, StatusCode::BAD_REQUEST);
                    assert_eq!(error.message, want);
                }
                _ => panic!("unexpected result for {value:?}"),
            }
        }
    }
}
