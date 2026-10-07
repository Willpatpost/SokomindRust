//! The accept checks a rise must pass, one unit each, in this order: Lane,
//! Sink, Line for each unsettled box, Matched, then the pocket check. Ban,
//! the first check of the ladder's accept, is the ladder's own.
//!
//! - Lane: a goal that the state leaves unfilled had an open lane at the
//!   root and has none now. A lane into goal `q` from direction `d` is
//!   the source cell `q - d` and the support cell `q - 2d`, both on the
//!   board. The lane is open when the support is empty and the source is
//!   empty or holds a box of the goal's group, or when the support holds
//!   such a box and the source is empty. The root's open lanes are computed
//!   once per frame, by [`root_lanes`].
//! - Sink: [`SinkLines::sink_ok`](crate::deadlock::SinkLines::sink_ok).
//! - Line, the probe's test (e): an unsettled box `x` must have a push the
//!   keeper can make while the settled boxes, the boxes on sink lines and
//!   `x` block its walk. The push must go to a cell that holds no settled
//!   box and is not dead for `x`'s label. Other boxes do not block the
//!   keeper here, as in the probe. Each unit checks one box.
//! - Matched: [`GoalReach::matched`](crate::deadlock::GoalReach::matched).
//! - Pockets: `Corral::pockets_begin` in one unit, then one
//!   `Corral::pockets_step` unit at a time until the check ends.
//!
//! Each unit first refreshes Deadlock's occupancy to the state it checks,
//! because the unit before it may have refreshed it for another state.
use super::{Facts, Tools};
use crate::corral::{PocketStep, Pockets};
use sokomind_core::{MAX_BOXES, NONE, OPPOSITE, State};

/// One accept check, in accept order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Check {
    /// A goal lost its last open lane since the root.
    Lane,
    /// Every sink line's boxes still fit its goals.
    Sink,
    /// Line for the first unsettled box at slot `i` or later. With no such
    /// box, the unit passes on to Matched.
    Line(u8),
    /// Each box still reaches a distinct goal of its own label alone.
    Matched,
    /// Opens the pocket check.
    PocketsInit,
    /// One unit of the pocket check.
    PocketPop,
}

/// What one check unit found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// The state passes the checks run so far. This check runs next.
    Next(Check),
    /// The state fails a check.
    Reject,
    /// The state passes every check.
    Pass,
}

/// Runs one unit, `check`, of accept on `state`. `lanes` is
/// [`root_lanes`] of the frame root. `pockets` holds the pocket check:
/// PocketsInit opens it and each PocketPop unit advances it. From
/// PocketsInit until the check ends, the caller must not use `tools.reach`
/// or `tools.corral` for anything else.
///
/// - Lane, Sink, Line and Matched give `Next` of the following check, or
///   `Reject`. Line's following check is Line of the next slot.
/// - PocketsInit gives `Next(PocketPop)`.
/// - PocketPop gives `Reject` for a dead pocket, `Pass` once no pocket is
///   dead, and otherwise `Next(PocketPop)`.
pub(super) fn run(
    tools: &mut Tools<'_>,
    facts: &Facts,
    pockets: &mut Pockets,
    state: &State,
    lanes: u32,
    check: Check,
) -> Verdict {
    let (board, heuristic) = (tools.board, tools.heuristic);
    let boxes = &state.boxes[..board.labels().len()];
    tools.deadlock.refresh(boxes);
    match check {
        Check::Lane => next_if(!lane_lost(tools, lanes), Check::Sink),
        Check::Sink => {
            let deadlock = &*tools.deadlock;
            let ok = facts.sink_lines.sink_ok(board, deadlock);
            next_if(ok, Check::Line(0))
        }
        Check::Line(from) => {
            let from = usize::from(from);
            match (from..boxes.len()).find(|&i| !board.on_goal(i, boxes[i])) {
                Some(i) => {
                    let ok = line_mobile(tools, facts, state, i);
                    next_if(ok, Check::Line(i as u8 + 1))
                }
                None => Verdict::Next(Check::Matched),
            }
        }
        Check::Matched => {
            let ok = facts.goal_reach.matched(&facts.blocks, heuristic, state);
            next_if(ok, Check::PocketsInit)
        }
        Check::PocketsInit => {
            let corral = &mut *tools.corral;
            corral.pockets_begin(pockets, board, tools.reach, state);
            Verdict::Next(Check::PocketPop)
        }
        Check::PocketPop => {
            let (corral, deadlock) = (&mut *tools.corral, &*tools.deadlock);
            match corral.pockets_step(pockets, board, heuristic, tools.reach, deadlock, state) {
                PocketStep::Dead => Verdict::Reject,
                PocketStep::Alive => Verdict::Pass,
                PocketStep::More => Verdict::Next(Check::PocketPop),
            }
        }
    }
}

/// The goal columns with an open lane in `root`. Bit `t` stands for column
/// `t` of `heuristic.goal_cells()`, whether or not the goal is filled.
/// Refreshes Deadlock's occupancy to `root`.
pub(super) fn root_lanes(tools: &mut Tools<'_>, root: &State) -> u32 {
    let boxes = tools.board.labels().len();
    tools.deadlock.refresh(&root.boxes[..boxes]);
    let mut lanes = 0;
    for t in 0..tools.heuristic.goal_cells().len() {
        if lane_open(tools, t) {
            lanes |= 1 << t;
        }
    }
    lanes
}

/// Whether some goal column that the state leaves unfilled had an open
/// lane at the root, its bit in `lanes`, and has none now. Deadlock's
/// occupancy must hold the state. A filled goal is a settled cell: a goal
/// holds a box of its own group exactly when the box is on a goal of its
/// own label.
fn lane_lost(tools: &Tools<'_>, lanes: u32) -> bool {
    let deadlock = &*tools.deadlock;
    let mut goals = tools.heuristic.goal_cells().iter().enumerate();
    goals.any(|(t, &q)| {
        let open_at_root = (lanes >> t) & 1 != 0;
        open_at_root && !super::settled(tools.board, deadlock, q) && !lane_open(tools, t)
    })
}

/// Whether goal column `t` has an open lane, as Deadlock's occupancy was
/// last refreshed. Box `i` belongs to the goal's group when
/// `heuristic.group(i)` contains `t`.
fn lane_open(tools: &Tools<'_>, t: usize) -> bool {
    let (neighbors, heuristic) = (tools.board.neighbors(), tools.heuristic);
    let deadlock = &*tools.deadlock;
    let around = neighbors[usize::from(heuristic.goal_cells()[t])];
    // The source in direction `d` from the goal feeds it by a push in
    // direction `OPPOSITE[d]`, so the four `d` visit the probe's four
    // lanes, which it names by their push direction.
    (0..4).any(|d| {
        let source = around[d];
        if source == NONE {
            return false;
        }
        let support = neighbors[usize::from(source)][d];
        if support == NONE {
            return false;
        }
        match (deadlock.at(source), deadlock.at(support)) {
            (None, None) => true,
            (Some(j), None) | (None, Some(j)) => heuristic.group(j).contains(&t),
            (Some(_), Some(_)) => false,
        }
    })
}

/// Whether box `i` of `state`, which is unsettled, passes Line. Deadlock's
/// occupancy must hold `state`. The blocked cells, which are the settled
/// boxes, the boxes on sink lines and box `i`'s own cell, are at most one
/// per box plus one. So they fit an inline `[Cell; MAX_BOXES + 1]` that
/// goes to `Reach::fill_from`.
fn line_mobile(tools: &mut Tools<'_>, facts: &Facts, state: &State, i: usize) -> bool {
    let (board, heuristic) = (tools.board, tools.heuristic);
    let boxes = &state.boxes[..board.labels().len()];
    let x = boxes[i];
    let mut blocked = [NONE; MAX_BOXES + 1];
    blocked[0] = x;
    let mut len = 1;
    for (j, &c) in boxes.iter().enumerate() {
        if j != i && (board.on_goal(j, c) || facts.sink_lines.on_line(c)) {
            blocked[len] = c;
            len += 1;
        }
    }
    tools.reach.fill_from(board, state.player, &blocked[..len]);
    let (reach, deadlock) = (&*tools.reach, &*tools.deadlock);
    let around = board.neighbors()[usize::from(x)];
    // A blocked or unreached stand has distance `NONE`, the probe's `FAR`.
    (0..4).any(|d| {
        let (ahead, stand) = (around[d], around[OPPOSITE[d]]);
        ahead != NONE
            && stand != NONE
            && reach.distance(stand) != NONE
            && !super::settled(board, deadlock, ahead)
            && !heuristic.dead(i, ahead)
    })
}

/// `Next(next)` when the check passed, else `Reject`.
fn next_if(passed: bool, next: Check) -> Verdict {
    if passed {
        Verdict::Next(next)
    } else {
        Verdict::Reject
    }
}

/// The check that first rejects `state`, running units from Lane on, or
/// `None` when it passes every check.
#[cfg(test)]
pub(super) fn first_reject(
    tools: &mut Tools<'_>,
    facts: &Facts,
    pockets: &mut Pockets,
    state: &State,
    lanes: u32,
) -> Option<Check> {
    let mut check = Check::Lane;
    loop {
        match run(tools, facts, pockets, state, lanes, check) {
            Verdict::Next(next) => check = next,
            Verdict::Reject => return Some(check),
            Verdict::Pass => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Check, Facts, Pockets, Tools, Verdict, first_reject, root_lanes, run};
    use crate::{
        corral::Corral, deadlock::Deadlock, heuristic::Heuristic, reach::Reach, stage::Scratch,
        testkit::huge,
    };
    use sokomind_core::{Board, Cell, State, Step, WALL, decode_direction};

    /// Three solution routes of huge, each with its move count: 626, 893
    /// and 503 moves, of 248, 278 and 236 pushes. Each file ends in a
    /// newline, so the route helpers trim it.
    const ROUTES: [(usize, &str); 3] = [
        (626, include_str!("../../testdata/huge-626.txt")),
        (893, include_str!("../../testdata/huge-893.txt")),
        (503, include_str!("../../testdata/huge-503.txt")),
    ];

    /// Three roots of one chain on huge, f19, f20 and f21: f20 seals the
    /// pocket (1,8)-(1,9) behind the X boxes on (1,7), (1,10) and (2,9),
    /// and f21 keeps the seal.
    const F19: [&str; 15] = [
        "###############",
        "#AXX...X...xxb#",
        "#X.X@.###.XXDX#",
        "#.....###.B.X.#",
        "#.....###.....#",
        "####.......####",
        "#......#......#",
        "#...H#####..G.#",
        "#......#......#",
        "###.........###",
        "###.........###",
        "#######.#######",
        "#.............#",
        "#.XC.......dX.#",
        "###############",
    ];
    /// See [`F19`].
    const F20: [&str; 15] = [
        "###############",
        "#AXX...X..Xxxb#",
        "#X.X..###X.XBX#",
        "#.....###.....#",
        "#.....###.....#",
        "####.......####",
        "#......#......#",
        "#...H#####..G.#",
        "#......#......#",
        "###.........###",
        "###.........###",
        "#######.#######",
        "#.............#",
        "#.XC......@DX.#",
        "###############",
    ];
    /// See [`F19`].
    const F21: [&str; 15] = [
        "###############",
        "#AXX...X..XxXB#",
        "#X.X..###X.x@X#",
        "#.....###.....#",
        "#.....###.....#",
        "####.......####",
        "#......#......#",
        "#...H#####..G.#",
        "#......#......#",
        "###.........###",
        "###.........###",
        "#######.#######",
        "#.............#",
        "#.XC.......DX.#",
        "###############",
    ];

    /// Every component a check unit borrows, owned and built for one board.
    struct Owned {
        heuristic: Heuristic,
        reach: Reach,
        deadlock: Deadlock,
        corral: Corral,
        scratch: Scratch,
        facts: Facts,
    }

    impl Owned {
        fn new(board: &Board) -> Self {
            let cells = board.tiles().len();
            let heuristic = Heuristic::new(board);
            let mut scratch = Scratch::new(cells).expect("stage scratch");
            let player = board.initial().player;
            let built = Facts::build(board, &heuristic, player, &mut scratch);
            let facts = built.expect("stage facts");
            Self {
                heuristic,
                reach: Reach::new(cells),
                deadlock: Deadlock::new(board),
                corral: Corral::new(board),
                scratch,
                facts,
            }
        }

        /// The units' three borrows, for `board`, the board these were
        /// built for.
        fn parts<'a>(&'a mut self, board: &'a Board) -> (Tools<'a>, &'a Facts, &'a mut Scratch) {
            let tools = Tools {
                board,
                heuristic: &self.heuristic,
                reach: &mut self.reach,
                deadlock: &mut self.deadlock,
                corral: &mut self.corral,
            };
            (tools, &self.facts, &mut self.scratch)
        }
    }

    /// The cell in row `r` and column `c` of `board`.
    fn at(board: &Board, r: usize, c: usize) -> Cell {
        (r * board.width() + c) as Cell
    }

    /// The state after each push of `route`, trimmed, in push order.
    fn route_pushes(board: &Board, route: &str) -> Vec<State> {
        let mut state = board.initial();
        let mut pushes = Vec::new();
        for action in route.trim().bytes() {
            let direction = decode_direction(action).expect("a route action");
            if let Step::Push(_) = board.step(&mut state, direction).expect("a legal move") {
                pushes.push(state);
            }
        }
        pushes
    }

    /// Along `route`, trimmed, the state after the last push of each run of
    /// pushes of one box.
    fn macro_ends(board: &Board, route: &str) -> Vec<State> {
        let mut state = board.initial();
        let mut ends = Vec::new();
        let mut current = None;
        let mut last = state;
        for action in route.trim().bytes() {
            let direction = decode_direction(action).expect("a route action");
            if let Step::Push(slot) = board.step(&mut state, direction).expect("a legal move") {
                if current.is_some_and(|s| s != slot) {
                    ends.push(last);
                }
                current = Some(slot);
                last = state;
            }
        }
        ends.push(last);
        ends
    }

    /// `base` with each `(from, to)` box move applied in order and the
    /// keeper on `player`. Panics when a `from` holds no box, or when a
    /// `to` or `player` is a wall or holds a box.
    fn place(board: &Board, base: &State, moves: &[(Cell, Cell)], player: Cell) -> State {
        let boxes = board.labels().len();
        let free = |state: &State, c: Cell| {
            board.tiles()[usize::from(c)] != WALL && !state.boxes[..boxes].contains(&c)
        };
        let mut state = *base;
        for &(from, to) in moves {
            let i = state.boxes[..boxes].iter().position(|&c| c == from);
            assert!(free(&state, to), "{to} is not free");
            state.boxes[i.expect("a box to move")] = to;
        }
        assert!(free(&state, player), "{player} is not free");
        state.player = player;
        state
    }

    /// The cell of the first box labelled `label`.
    fn box_of(board: &Board, state: &State, label: u8) -> Cell {
        let i = board.labels().iter().position(|&l| l == label);
        state.boxes[i.expect("a box of the label")]
    }

    /// The state a picture draws, its rows from the board's top row: each
    /// box letter fills its label's slots in row-major order, and '@' is
    /// the keeper. Panics when a label's box count is off or no keeper is
    /// drawn.
    fn picture(board: &Board, rows: &[&str]) -> State {
        let labels = board.labels();
        let mut state = board.initial();
        let mut placed = vec![false; labels.len()];
        let mut player = None;
        for (r, row) in rows.iter().enumerate() {
            for (c, ch) in row.bytes().enumerate() {
                if ch == b'@' {
                    player = Some(at(board, r, c));
                } else if labels.contains(&ch) {
                    let i = (0..labels.len()).find(|&i| labels[i] == ch && !placed[i]);
                    let i = i.expect("a slot left for the box");
                    placed[i] = true;
                    state.boxes[i] = at(board, r, c);
                }
            }
        }
        assert!(placed.iter().all(|&p| p), "a box is missing");
        state.player = player.expect("a keeper");
        state
    }

    /// One fixture on huge: a frame root and a child, with what the levels
    /// and the checks must give.
    struct Fixture {
        /// The fixture's name.
        name: &'static str,
        /// The frame root, which gives the lanes.
        root: State,
        /// The state the checks run on.
        child: State,
        /// The levels of the root and of the child.
        levels: (i8, i8),
        /// The check that first rejects the child, with any Line unit
        /// written as `Line(0)`.
        first: Option<Check>,
        /// Every check that rejects the child, by name, in accept order.
        rejected: &'static [&'static str],
    }

    /// The fixtures on huge, each child one or two box moves from its root
    /// or a picture. The root is the start unless stated.
    fn huge_fixtures(board: &Board) -> Vec<Fixture> {
        let start = board.initial();
        let at = |r, c| at(board, r, c);
        let place =
            |base: &State, moves: &[(Cell, Cell)], player| place(board, base, moves, player);
        let ends = macro_ends(board, ROUTES[1].1);
        let f2_root = place(&start, &[(at(3, 1), at(2, 1))], at(3, 1));
        let (a, b) = (box_of(board, &start, b'A'), box_of(board, &start, b'B'));
        let f6_moves = [(at(12, 4), at(13, 2)), (at(12, 10), at(13, 12))];
        let [f19, f20, f21] = [F19, F20, F21].map(|rows| picture(board, &rows));
        vec![
            Fixture {
                name: "m11",
                root: ends[10],
                child: ends[11],
                levels: (-3, -2),
                first: None,
                rejected: &[],
            },
            Fixture {
                name: "F2",
                root: f2_root,
                child: place(&f2_root, &[(at(3, 3), at(1, 2))], at(1, 3)),
                levels: (-5, -4),
                first: Some(Check::Lane),
                rejected: &["lane", "e"],
            },
            Fixture {
                name: "F3",
                root: start,
                child: place(&start, &[(at(2, 2), at(13, 3))], at(13, 4)),
                levels: (-6, -5),
                first: Some(Check::Lane),
                rejected: &["lane", "e", "g"],
            },
            Fixture {
                name: "F4",
                root: start,
                child: place(&start, &[(at(10, 8), at(13, 12))], at(13, 11)),
                levels: (-6, -5),
                first: Some(Check::Line(0)),
                rejected: &["e", "g"],
            },
            Fixture {
                name: "F5",
                root: start,
                child: place(&start, &[(at(3, 3), at(1, 2))], at(1, 3)),
                levels: (-6, -5),
                first: Some(Check::Lane),
                rejected: &["lane", "e", "g"],
            },
            Fixture {
                name: "U5a",
                root: start,
                child: place(&start, &[(at(3, 3), at(1, 2)), (a, at(1, 4))], at(1, 3)),
                levels: (-6, -4),
                first: Some(Check::Lane),
                rejected: &["lane", "sink", "e", "g"],
            },
            Fixture {
                name: "U5b",
                root: start,
                child: place(&start, &[(b, at(1, 10)), (at(3, 3), at(1, 12))], at(1, 11)),
                levels: (-6, -4),
                first: Some(Check::Lane),
                rejected: &["lane", "sink", "e", "g"],
            },
            Fixture {
                name: "F6",
                root: start,
                child: place(&start, &f6_moves, at(11, 7)),
                levels: (-6, -2),
                first: Some(Check::Line(0)),
                rejected: &["e", "matched", "g"],
            },
            Fixture {
                name: "F7",
                root: f19,
                child: f20,
                levels: (13, 14),
                first: Some(Check::PocketPop),
                rejected: &["g"],
            },
            Fixture {
                name: "F8",
                root: f20,
                child: f21,
                levels: (14, 15),
                first: Some(Check::PocketPop),
                rejected: &["g"],
            },
        ]
    }

    /// Every check that rejects `state`, with no early exit. Each check
    /// appears once, by its name in the fixture tables: "lane", "sink", "e"
    /// for Line, "matched" and "g" for the pocket check.
    fn rejects(
        tools: &mut Tools<'_>,
        facts: &Facts,
        pockets: &mut Pockets,
        state: &State,
        lanes: u32,
    ) -> Vec<&'static str> {
        let starts = [
            Check::Lane,
            Check::Sink,
            Check::Line(0),
            Check::Matched,
            Check::PocketsInit,
        ];
        let mut names = Vec::new();
        for start in starts {
            // Run the check's own units alone: a unit that hands on to the
            // next check ends it as passed.
            let mut check = start;
            loop {
                match run(tools, facts, pockets, state, lanes, check) {
                    Verdict::Next(next) if name(next) == name(start) => check = next,
                    Verdict::Reject => {
                        names.push(name(start));
                        break;
                    }
                    Verdict::Next(_) | Verdict::Pass => break,
                }
            }
        }
        names
    }

    /// The name of the check a unit belongs to, as the fixture tables write
    /// it.
    fn name(check: Check) -> &'static str {
        match check {
            Check::Lane => "lane",
            Check::Sink => "sink",
            Check::Line(_) => "e",
            Check::Matched => "matched",
            Check::PocketsInit | Check::PocketPop => "g",
        }
    }

    /// Each fixture's levels, the check that first rejects its child, and
    /// every check that rejects it.
    #[test]
    fn fixtures() {
        let board = huge();
        let cases = huge_fixtures(&board);
        let mut owned = Owned::new(&board);
        let (mut tools, facts, scratch) = owned.parts(&board);
        let heuristic = tools.heuristic;
        let pockets = &mut scratch.pockets;
        let level = |state: &State| facts.rooms.level(&board, heuristic, state);
        for f in &cases {
            assert_eq!((level(&f.root), level(&f.child)), f.levels, "{}", f.name);
            let lanes = root_lanes(&mut tools, &f.root);
            let first = match first_reject(&mut tools, facts, pockets, &f.child, lanes) {
                Some(Check::Line(_)) => Some(Check::Line(0)),
                other => other,
            };
            assert_eq!(first, f.first, "{}", f.name);
            let names = rejects(&mut tools, facts, pockets, &f.child, lanes);
            assert_eq!(names, f.rejected, "{}", f.name);
        }
    }

    /// Huge's three routes, of 248, 278 and 236 pushes. With each push
    /// state as its own root, Sink, Matched and the pocket check reject
    /// none, and only Line rejects any, on route 503, where pushes 78 to 80
    /// put B on row 1. Against the previous rise as root, a rise being a
    /// push state whose level exceeds the previous rise's, from the start,
    /// no check rejects a rise, so the accept checks keep every rise of a
    /// real solution.
    #[test]
    fn route_replay() {
        let board = huge();
        let mut owned = Owned::new(&board);
        let (mut tools, facts, scratch) = owned.parts(&board);
        let heuristic = tools.heuristic;
        let pockets = &mut scratch.pockets;
        let level = |state: &State| facts.rooms.level(&board, heuristic, state);
        for ((moves, route), pushes) in ROUTES.into_iter().zip([248, 278, 236]) {
            assert_eq!(route.trim().len(), moves);
            let states = route_pushes(&board, route);
            assert_eq!(states.len(), pushes, "{moves}");
            let mut rejected = Vec::new();
            for (k, state) in states.iter().enumerate() {
                let lanes = root_lanes(&mut tools, state);
                let names = rejects(&mut tools, facts, pockets, state, lanes);
                if !names.is_empty() {
                    rejected.push((k + 1, names));
                }
            }
            if moves == 503 {
                assert!(
                    rejected.iter().all(|(_, names)| *names == ["e"]),
                    "{rejected:?}"
                );
            } else {
                assert!(rejected.is_empty(), "{moves}: {rejected:?}");
            }
            let mut root = board.initial();
            let mut root_level = level(&root);
            let mut rises = 0;
            for state in &states {
                let now = level(state);
                if now <= root_level {
                    continue;
                }
                let lanes = root_lanes(&mut tools, &root);
                let names = rejects(&mut tools, facts, pockets, state, lanes);
                assert!(names.is_empty(), "{moves} rise {rises}: {names:?}");
                root = *state;
                root_level = now;
                rises += 1;
            }
        }
    }
}
