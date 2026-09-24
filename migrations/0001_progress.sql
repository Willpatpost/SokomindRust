CREATE TABLE progress (
    profile TEXT NOT NULL CHECK (profile ~ '^[0-9a-fA-F]{32}$'),
    puzzle_id TEXT NOT NULL,
    moves INTEGER NOT NULL CHECK (moves >= 0),
    pushes INTEGER NOT NULL CHECK (pushes >= 0 AND pushes <= moves),
    route TEXT NOT NULL CHECK (length(route) = moves AND length(route) <= 100000 AND route !~ '[^UDLR]'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (profile, puzzle_id)
);
