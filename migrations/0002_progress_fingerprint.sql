-- Progress is keyed by layout fingerprint, so a changed catalog puzzle
-- starts fresh records instead of returning routes that no longer replay.
-- Pre-release data is intentionally discarded: old rows cannot be migrated
-- because fingerprints are computed from the catalog, not from SQL.
DROP TABLE progress;
CREATE TABLE progress (
    profile TEXT NOT NULL CHECK (profile ~ '^[0-9a-fA-F]{32}$'),
    puzzle_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL CHECK (fingerprint ~ '^puzzle-v1:[0-9a-f]{8}$'),
    moves INTEGER NOT NULL CHECK (moves >= 0),
    pushes INTEGER NOT NULL CHECK (pushes >= 0 AND pushes <= moves),
    route TEXT NOT NULL CHECK (length(route) = moves AND length(route) <= 100000 AND route !~ '[^UDLR]'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (profile, puzzle_id, fingerprint)
);
