//! The accept checks a rise must pass, one unit each, in the probe's order:
//! Lane, Sink, Line for each unsettled box, Matched, then the pocket check.
//! This ports the probe's `lane_open`, `lane_lost`, `line_mobile_ok` and
//! `accept` (p4b.rs 726-752, 813-838 and 872-901). The probe's `verdict`
//! audit is not ported. Ban, the first check of the ladder's accept, comes
//! with the ladder in step 8.
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
        stage::reference::{FAR, Geo, Owned, ROUTES, at, macro_ends, random_state, route_pushes},
        testkit::{Lcg, catalog, huge},
    };
    use sokomind_core::{Cell, NONE, OPPOSITE, State, WALL};

    /// The probe's P2e chain roots f19, f20 and f21 on huge (p4b.rs
    /// 1347-1397): f20 seals the pocket (1,8)-(1,9) behind the X boxes on
    /// (1,7), (1,10) and (2,9), and f21 keeps the seal.
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

    /// One fixture of plan §7.2: a frame root and a child on huge, with
    /// what the levels and the checks must give.
    struct Fixture {
        /// The fixture's name in the plan's table.
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

    /// The fixtures of plan §7.2, built as the probe's `fixtures` builds
    /// them (p4b.rs 1303-1407). The root is the start unless stated.
    fn huge_fixtures(geo: &Geo<'_>) -> Vec<Fixture> {
        let board = geo.board;
        let start = board.initial();
        let at = |r, c| at(board, r, c);
        let place = |base: &State, moves: &[(Cell, Cell)], player| {
            geo.place(base, moves, player).expect("the fixture fits")
        };
        let parse = |rows: &[&str]| geo.parse_show(rows).expect("the picture parses");
        let ends = macro_ends(board, ROUTES[1].1).expect("route 893");
        let f2_root = place(&start, &[(at(3, 1), at(2, 1))], at(3, 1));
        let a = geo.box_of(&start, b'A').expect("huge has an A box");
        let b = geo.box_of(&start, b'B').expect("huge has a B box");
        let f6_moves = [(at(12, 4), at(13, 2)), (at(12, 10), at(13, 12))];
        let (f19, f20, f21) = (parse(&F19), parse(&F20), parse(&F21));
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

    /// Plan §7.2 on huge: each fixture's levels, the check that first
    /// rejects its child, and every check that rejects it.
    #[test]
    fn fixtures() {
        let board = huge();
        let mut owned = Owned::new(&board);
        let geo = Geo::new(&board, &owned.heuristic);
        let cases = huge_fixtures(&geo);
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

    /// Plan §7.3 on huge's three probe routes, of 248, 278 and 236 pushes.
    /// With each push state as its own root, Sink, Matched and the pocket
    /// check reject none, and Line rejects only pushes 78 to 80 of route
    /// 503, which put B on (1,10), (1,11) and (1,12). Each route rises
    /// exactly 23 times, a rise being a push state whose level exceeds the
    /// previous rise's, from the start. Against the previous rise as root,
    /// no check rejects a rise.
    #[test]
    fn route_replay() {
        let board = huge();
        let mut owned = Owned::new(&board);
        let geo = Geo::new(&board, &owned.heuristic);
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
                let want = [(78, vec!["e"]), (79, vec!["e"]), (80, vec!["e"])];
                assert_eq!(rejected, want);
                for (k, c) in [(78, 10), (79, 11), (80, 12)] {
                    let b = geo.box_of(&states[k - 1], b'B');
                    assert_eq!(b, Some(at(&board, 1, c)), "push {k}");
                }
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
            assert_eq!(rises, 23, "{moves}");
        }
    }

    /// Random (root, child) pairs of every catalog board: the root's lanes
    /// match the probe's `lane_open` goal by goal, the Lane unit matches
    /// the probe's `lane_lost`, and each Line unit matches the probe's test
    /// (e) on the first unsettled box from its slot on (p4b.rs 726-750 and
    /// 813-838). Some pair must lose a lane and some box must fail Line.
    #[test]
    fn checks_match_probe() {
        let mut rng = Lcg(0xc4ec);
        let (mut lost, mut immobile) = (0, 0);
        for (id, board) in catalog() {
            let mut owned = Owned::new(&board);
            let geo = Geo::new(&board, &owned.heuristic);
            let on_line = probe_on_line(&geo);
            let mut pairs = Vec::new();
            for _ in 0..30 {
                let root = random_state(&mut rng, &board, &owned.heuristic);
                let child = random_state(&mut rng, &board, &owned.heuristic);
                pairs.push((root, child));
            }
            let (mut tools, facts, scratch) = owned.parts(&board);
            let heuristic = tools.heuristic;
            let pockets = &mut scratch.pockets;
            for (p, (root, child)) in pairs.iter().enumerate() {
                let lanes = root_lanes(&mut tools, root);
                let root_occ = geo.occ(root);
                for (t, &q) in heuristic.goal_cells().iter().enumerate() {
                    let g = geo.goal_group[usize::from(q)];
                    let open = probe_lane_open(&geo, q, g, &root_occ);
                    assert_eq!((lanes >> t) & 1 != 0, open, "{id} pair {p} column {t}");
                }
                let lane = run(&mut tools, facts, pockets, child, lanes, Check::Lane);
                let want = if probe_lane_lost(&geo, child, root) {
                    lost += 1;
                    Verdict::Reject
                } else {
                    Verdict::Next(Check::Sink)
                };
                assert_eq!(lane, want, "{id} pair {p}");
                let w = geo.settled(child);
                for i in 0..geo.slots {
                    let check = Check::Line(i as u8);
                    let got = run(&mut tools, facts, pockets, child, lanes, check);
                    let unsettled = (i..geo.slots).find(|&j| !w[usize::from(child.boxes[j])]);
                    let want = match unsettled {
                        None => Verdict::Next(Check::Matched),
                        Some(j) if probe_line(&geo, &on_line, child, j) => {
                            Verdict::Next(Check::Line(j as u8 + 1))
                        }
                        Some(_) => Verdict::Reject,
                    };
                    immobile += usize::from(want == Verdict::Reject);
                    assert_eq!(got, want, "{id} pair {p} slot {i}");
                }
            }
        }
        assert!(lost > 0, "no pair lost a lane");
        assert!(immobile > 0, "no box failed Line");
    }

    /// The probe's `lane_open` (p4b.rs 726-741): some lane into goal `q`
    /// of group `g` is open in the occupancy `occ` of `Geo::occ`.
    fn probe_lane_open(geo: &Geo<'_>, q: Cell, g: usize, occ: &[usize]) -> bool {
        let fits = |o: usize| o != 0 && geo.group[o - 1] == g;
        (0..4).any(|d| {
            let src = geo.nb[usize::from(q)][OPPOSITE[d]];
            if src == NONE {
                return false;
            }
            let sup = geo.nb[usize::from(src)][OPPOSITE[d]];
            if sup == NONE {
                return false;
            }
            let (s, p) = (occ[usize::from(src)], occ[usize::from(sup)]);
            ((s == 0 || fits(s)) && p == 0) || (fits(p) && s == 0)
        })
    }

    /// The probe's `lane_lost` (p4b.rs 743-750): some goal that `child`
    /// leaves unfilled had an open lane in `root` and has none in `child`.
    fn probe_lane_lost(geo: &Geo<'_>, child: &State, root: &State) -> bool {
        let (occ, root_occ) = (geo.occ(child), geo.occ(root));
        geo.goals.iter().any(|&(q, g)| {
            !geo.filled(q, g, &occ)
                && probe_lane_open(geo, q, g, &root_occ)
                && !probe_lane_open(geo, q, g, &occ)
        })
    }

    /// The probe's `on_line` (p4b.rs 197-202): the cells of its
    /// `sink_lines` (640-666), every wall-to-wall run of two or more cells
    /// along a row or a column with a wall across the run at each cell.
    fn probe_on_line(geo: &Geo<'_>) -> Vec<bool> {
        let tiles = geo.board.tiles();
        let mut on_line = vec![false; geo.nb.len()];
        for (back, fwd, a, b) in [(2, 3, 0, 1), (0, 1, 2, 3)] {
            for (c, around) in geo.nb.iter().enumerate() {
                if tiles[c] == WALL || around[back] != NONE {
                    continue;
                }
                let mut run = Vec::new();
                let mut frozen = true;
                let mut x = c as Cell;
                loop {
                    let n = geo.nb[usize::from(x)];
                    frozen &= n[a] == NONE || n[b] == NONE;
                    run.push(x);
                    if n[fwd] == NONE {
                        break;
                    }
                    x = n[fwd];
                }
                if frozen && run.len() >= 2 {
                    for x in run {
                        on_line[usize::from(x)] = true;
                    }
                }
            }
        }
        on_line
    }

    /// The probe's test (e) for box `i` of `s`, which is unsettled (p4b.rs
    /// 813-838): with the settled boxes, the boxes on `on_line` cells and
    /// the box itself blocking the keeper, the keeper reaches the stand of
    /// a push of the box to a cell that holds no settled box and is not
    /// dead for its group.
    fn probe_line(geo: &Geo<'_>, on_line: &[bool], s: &State, i: usize) -> bool {
        let w = geo.settled(s);
        let mut blocked = w.clone();
        for &c in &s.boxes[..geo.slots] {
            if on_line[usize::from(c)] {
                blocked[usize::from(c)] = true;
            }
        }
        let x = s.boxes[i];
        blocked[usize::from(x)] = true;
        let k = geo.flood(s.player, &blocked);
        let (g, around) = (geo.group[i], geo.nb[usize::from(x)]);
        (0..4).any(|d| {
            let (y, st) = (around[d], around[OPPOSITE[d]]);
            y != NONE
                && st != NONE
                && k[usize::from(st)] != FAR
                && !w[usize::from(y)]
                && !geo.dead[g][usize::from(y)]
        })
    }
}
