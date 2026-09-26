use sokomind_core::{Board, Game, State};
use sokomind_search::{ExactSearch, Mode, Proof, Search, Status, StopReason};
use std::collections::{HashSet, VecDeque};

const TWO: &str = "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO";
/// The box is frozen against the top-right wall and can never reach its goal.
const CORNERED: &str = "OOOOO\nOR XO\nOS  O\nOOOOO";
/// Pushing both boxes against the wall forms a wall/box 2x2 square.
const WALL_PAIR: &str = "OOOOOOO\nOR    O\nO AB  O\nO     O\nOa b  O\nOOOOOOO";
/// D can only be pushed down, where it freezes on its goal. A's goal is to its
/// right, but the cell a right push needs is walled in by D and A, so every
/// line ends dead.
const FREEZE_CHAIN: &str = "O    O\nODO  O\nOd AaO\nOOO RO\nOOOOOO";
/// The player can only reach A's freezing side, so every line ends dead.
const FROZEN_UNSOLVABLE: &str = "OOOOOO\nODO RO\nOd AaO\nOOOOOO";
/// Each optimal route pushes a box past a same-label box, which reorders the
/// canonical group. Deadlock checks that mixed that order with the parent's
/// box index pruned these routes into false `Unsolvable` or `Optimal` proofs.
const REORDERED: [(&str, u32); 5] = [
    ("OOOOOO\nOOOROO\nOO XOO\nOOX SO\nOOSOOO\nOOOOOO", 4),
    ("OOOOOO\nO  R O\nO  X O\nO X SO\nO SOOO\nOOOOOO", 4),
    ("OOOOO\nO OSO\nOSX O\nO X O\nOR OO\nOOOOO", 11),
    ("OOOOOO\nO R  O\nOSXX O\nO  A O\nOOOaSO\nOOOOOO", 20),
    ("OOOOOO\nOOO  O\nOS X O\nOOXR O\nO   SO\nOOOOOO", 15),
];
/// Optimum 9; the same bug proved 17, and limits certified 15..=17.
const CROSSING: &str = "OOOOOOO\nOOOR OO\nOO X  O\nOSX   O\nOOSOOOO\nOOOOOOO";
/// One label group of eight boxes, large enough for incremental dual repair.
/// The optimum is one push, then seven times up, right, and push.
const EIGHT_IN_A_ROW: &str = "OOOOOOOOOO\nOR       O\nOXXXXXXXXO\nOSSSSSSSSO\nOOOOOOOOOO";
/// Hand-built boards with their BFS optima (`None` when unsolvable), besides
/// [`REORDERED`]. Every mode must finish on each within the default limits.
const BOARDS: [(&str, Option<u32>); 10] = [
    ("OOOOO\nO R O\nO A O\nO a O\nOOOOO", Some(1)),
    (TWO, Some(20)),
    ("OOOOOOO\nOR    O\nO XX  O\nO SS  O\nOOOOOOO", Some(5)),
    ("OOOOOOO\nORX  SO\nOOOOOOO", Some(3)),
    (WALL_PAIR, Some(10)),
    (FREEZE_CHAIN, None),
    (FROZEN_UNSOLVABLE, None),
    (CORNERED, None),
    (CROSSING, Some(9)),
    (EIGHT_IN_A_ROW, Some(22)),
];

// Independent primitive-move BFS oracle (not a second push-search implementation).
fn bfs(board: &Board) -> Option<u32> {
    bfs_within(board, usize::MAX).unwrap()
}

/// `bfs`, or `None` once it has seen more than `cap` states.
fn bfs_within(board: &Board, cap: usize) -> Option<Option<u32>> {
    let key = |s: State| (s.player, s.boxes);
    let mut queue = VecDeque::from([(board.initial(), 0)]);
    let mut seen = HashSet::from([key(board.initial())]);
    while let Some((state, moves)) = queue.pop_front() {
        if board.solved(&state) {
            return Some(Some(moves));
        }
        for direction in 0..4 {
            let mut next = state;
            if board.step(&mut next, direction).is_some() && seen.insert(key(next)) {
                if seen.len() > cap {
                    return None;
                }
                queue.push_back((next, moves + 1));
            }
        }
    }
    Some(None)
}

/// Runs a search one expansion at a time, checking after each that the live
/// lower bound stays at or below the optimum and the incumbent never worsens.
fn drive(
    board: &Board,
    mode: Mode,
    max_states: usize,
    optimum: Option<u32>,
    context: &str,
) -> Search {
    let mut search = Search::new(board.clone(), board.initial(), mode, max_states, 8).unwrap();
    let mut best = u32::MAX;
    while search.status() == Status::Running {
        search.advance(1);
        if let (Some(bound), Some(optimum)) = (search.lower_bound(), optimum) {
            assert!(
                bound <= optimum,
                "live bound {bound} > {optimum}: {context}"
            );
        }
        let current = search.best_moves().unwrap_or(u32::MAX);
        assert!(
            current <= best,
            "incumbent {current} after {best}: {context}"
        );
        best = current;
    }
    search
}

/// Everything a search claims must agree with the BFS optimum (`None` when
/// unsolvable): its bound, its certificate, and its route, which exists
/// exactly when it has an incumbent.
fn assert_consistent(board: &Board, search: &mut Search, optimum: Option<u32>, context: &str) {
    if let (Some(bound), Some(optimum)) = (search.lower_bound(), optimum) {
        assert!(
            bound <= optimum,
            "lower bound {bound} > {optimum}: {context}"
        );
    }
    let best = search.best_moves();
    match search.proof() {
        None => {}
        Some(Proof::Unsolvable) => assert_eq!(optimum, None, "{context}"),
        Some(Proof::Optimal { moves }) => assert_eq!(Some(moves), optimum, "{context}"),
        Some(Proof::Bounded {
            lower_bound,
            upper_bound,
        }) => {
            assert_eq!(Some(upper_bound), best, "{context}");
            let optimum = optimum.expect(context);
            assert!(
                lower_bound <= optimum && optimum <= upper_bound,
                "{context}"
            );
        }
    }
    let Some(best) = best else {
        assert_eq!(search.solution().unwrap(), None, "{context}");
        return;
    };
    let optimum = optimum.unwrap_or_else(|| panic!("route on an unsolvable board: {context}"));
    assert!(best >= optimum, "{context}");
    let route = search.solution().unwrap().unwrap();
    assert_route(board, &route, best, context);
}

/// Independently replays `route`, which must solve the board in `moves` moves.
fn assert_route(board: &Board, route: &str, moves: u32, context: &str) {
    assert_eq!(route.len(), moves as usize, "{context}");
    let mut game = Game::new(board.clone());
    game.replay(route).unwrap();
    assert!(game.solved(), "{context}");
}

/// Runs all three modes against the BFS optimum and returns the exact run's
/// node count. With `must_finish`, a limit is a failure; otherwise a run cut
/// short by one only has to stay consistent.
fn engines_agree(board: &Board, rows: &str, optimum: Option<u32>, must_finish: bool) -> usize {
    let mut full = 1;
    for mode in [Mode::Optimal, Mode::Fast, Mode::Quality] {
        let context = format!("{mode:?} on {rows:?}");
        let mut search = drive(board, mode, 20_000, optimum, &context);
        assert_consistent(board, &mut search, optimum, &context);
        if mode == Mode::Optimal {
            full = search.generated() as usize;
        } else {
            assert_eq!(search.proof(), None, "{context}");
            assert_eq!(search.lower_bound(), None, "{context}");
        }
        if !must_finish && matches!(search.status(), Status::StateLimit | Status::MemoryLimit) {
            continue;
        }
        let finished = if optimum.is_some() {
            Status::Solved
        } else {
            Status::Exhausted
        };
        assert_eq!(search.status(), finished, "{context}");
        assert_eq!(
            search.best_moves().is_some(),
            optimum.is_some(),
            "{context}"
        );
        if mode == Mode::Optimal {
            assert_eq!(search.best_moves(), optimum, "{context}");
            let proof = optimum.map_or(Proof::Unsolvable, |moves| Proof::Optimal { moves });
            assert_eq!(search.proof(), Some(proof), "{context}");
        }
    }
    full
}

/// Seeded LCG, so every run generates the same boards.
struct Lcg(u64);
impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as usize % bound
    }
    fn take(&mut self, free: &mut Vec<usize>) -> usize {
        let index = self.below(free.len());
        free.swap_remove(index)
    }
}

/// A walled 5..=6 x 5..=6 room with one to three X boxes, sometimes an A box,
/// their goals, and about a quarter of the spare floor walled. Boxes start off
/// the room's rim. Every board parses; many are unsolvable.
fn random_board(rng: &mut Lcg) -> String {
    let (width, height) = (5 + rng.below(2), 5 + rng.below(2));
    let mut cells = vec![b' '; width * height];
    let mut boxes = vec![b'X'; 1 + rng.below(3)];
    if rng.below(4) == 0 {
        boxes.push(b'A');
    }
    let mut free: Vec<usize> = (0..cells.len())
        .filter(|&c| {
            (1..width - 1).contains(&(c % width)) && (1..height - 1).contains(&(c / width))
        })
        .collect();
    for &label in &boxes {
        cells[rng.take(&mut free)] = label;
    }
    let mut free: Vec<usize> = (0..cells.len()).filter(|&c| cells[c] == b' ').collect();
    for &label in &boxes {
        let goal = if label == b'X' {
            b'S'
        } else {
            label.to_ascii_lowercase()
        };
        cells[rng.take(&mut free)] = goal;
    }
    cells[rng.take(&mut free)] = b'R';
    for cell in free {
        if rng.below(100) < 25 {
            cells[cell] = b'O';
        }
    }
    let wall = "O".repeat(width + 2);
    let mut rows = vec![wall.clone()];
    for row in cells.chunks(width) {
        rows.push(format!("O{}O", std::str::from_utf8(row).unwrap()));
    }
    rows.push(wall);
    rows.join("\n")
}

/// The names the server API and the wasm worker exchange with their clients.
#[test]
fn mode_and_status_names_are_stable() {
    for (name, mode) in [
        ("fast", Mode::Fast),
        ("quality", Mode::Quality),
        ("optimal", Mode::Optimal),
    ] {
        assert_eq!(Mode::parse(name), Ok(mode));
    }
    for name in ["", "Fast", "exact", "optimal "] {
        assert!(Mode::parse(name).is_err(), "{name:?}");
    }
    for (status, name) in [
        (Status::Running, "running"),
        (Status::Solved, "solved"),
        (Status::Exhausted, "exhausted"),
        (Status::StateLimit, "state_limit"),
        (Status::MemoryLimit, "memory_limit"),
        (Status::TimeLimit, "time_limit"),
        (Status::Cancelled, "cancelled"),
    ] {
        assert_eq!(status.as_str(), name);
    }
}

#[test]
fn engines_agree_with_bfs_on_fixed_boards() {
    let reordered = REORDERED.map(|(rows, moves)| (rows, Some(moves)));
    for (rows, optimum) in BOARDS.into_iter().chain(reordered) {
        let board = Board::parse(rows).unwrap();
        assert_eq!(bfs(&board), optimum, "{rows:?}");
        engines_agree(&board, rows, optimum, true);
    }
}

/// A stop before the first slice leaves only the start node and no claim.
#[test]
fn searches_stopped_before_starting_claim_nothing() {
    let board = Board::parse(TWO).unwrap();
    for mode in [Mode::Optimal, Mode::Fast, Mode::Quality] {
        let mut search = Search::new(board.clone(), board.initial(), mode, 20_000, 8).unwrap();
        search.stop(StopReason::Cancelled);
        search.advance(32);
        assert_eq!(search.status(), Status::Cancelled, "{mode:?}");
        assert_eq!((search.expanded(), search.generated()), (0, 1), "{mode:?}");
        assert_eq!(search.proof(), None, "{mode:?}");
        assert_eq!(search.solution().unwrap(), None, "{mode:?}");
    }
}

#[test]
fn interrupted_optimal_keeps_a_sound_gap() {
    let board = Board::parse(TWO).unwrap();
    let optimum = bfs(&board);
    let mut search = Search::new(board.clone(), board.initial(), Mode::Optimal, 20_000, 8).unwrap();
    // The first incumbent always appears when the solved child is generated,
    // at least one expansion before it can pop, so the stop lands mid-run.
    while search.status() == Status::Running && search.best_moves().is_none() {
        search.advance(1);
    }
    search.stop(StopReason::TimeLimit);
    assert_eq!(search.status(), Status::TimeLimit);
    let best = search.best_moves().unwrap();
    assert!(
        matches!(
            search.proof(),
            Some(Proof::Optimal { .. } | Proof::Bounded { .. })
        ),
        "expected a certificate, got {:?}",
        search.proof()
    );
    assert!(search.lower_bound().is_some_and(|bound| bound <= best));
    assert_consistent(&board, &mut search, optimum, "interrupted");
}

/// Sweeps the Fast state limit across the boundary where a solution child is
/// discovered at the exact moment the arena fills.
#[test]
fn fast_routes_survive_state_limit_boundaries() {
    let board = Board::parse(TWO).unwrap();
    let optimum = bfs(&board);
    let mut kept = 0;
    for max_states in 1..=40 {
        let context = format!("at {max_states} states");
        let mut search = drive(&board, Mode::Fast, max_states, optimum, &context);
        assert_consistent(&board, &mut search, optimum, &context);
        if search.best_moves().is_none() {
            assert_eq!(search.status(), Status::StateLimit, "{context}");
        } else if search.status() == Status::StateLimit {
            kept += 1;
        }
    }
    assert!(kept > 0, "no state limit kept the solution it found");
}

/// Sweeps every state limit up to the unlimited run's node count. A limit
/// always cuts an expansion short, so the frontier must count that node's
/// unpushed children (`interrupted_g`); some limits land exactly as the solved
/// child needs the arena's spare node (`Arena::insert` past the limit).
#[test]
fn limited_optimal_bounds_never_pass_the_optimum() {
    let (mut kept, mut spare) = (0, 0);
    for (rows, optimum) in [(CROSSING, 9), (TWO, 20)].into_iter().chain(REORDERED) {
        let board = Board::parse(rows).unwrap();
        assert_eq!(bfs(&board), Some(optimum), "{rows:?}");
        let full = drive(&board, Mode::Optimal, 20_000, Some(optimum), rows).generated() as usize;
        for max_states in 1..=full {
            let context = format!("{rows:?} at {max_states} states");
            let mut search = drive(&board, Mode::Optimal, max_states, Some(optimum), &context);
            assert_consistent(&board, &mut search, Some(optimum), &context);
            if max_states == full {
                assert_eq!(
                    search.proof(),
                    Some(Proof::Optimal { moves: optimum }),
                    "{context}"
                );
                continue;
            }
            assert_eq!(search.status(), Status::StateLimit, "{context}");
            let generated = search.generated() as usize;
            if search.best_moves().is_none() {
                assert!(generated <= max_states, "{context}");
                assert_eq!(search.proof(), None, "{context}");
                continue;
            }
            kept += 1;
            // Only the spare node can take the arena past its limit.
            assert!(generated <= max_states + 1, "{context}");
            if generated == max_states + 1 {
                spare += 1;
            }
            assert!(
                matches!(
                    search.proof(),
                    Some(Proof::Optimal { .. } | Proof::Bounded { .. })
                ),
                "incumbent without a certificate: {context}"
            );
        }
    }
    assert!(kept > 0, "no limited run kept an incumbent");
    assert!(spare > 0, "no limited run used the spare node");
}

/// All three modes against the BFS oracle on seeded random boards, plus two
/// limited exact runs per board. Boards whose oracle would see more than
/// 10,000 states are skipped to keep the test fast in debug builds.
#[test]
fn engines_agree_with_bfs_on_generated_boards() {
    let mut rng = Lcg(1);
    let (mut solvable, mut unsolvable) = (0, 0);
    for _ in 0..200 {
        let rows = random_board(&mut rng);
        let board = Board::parse(&rows).unwrap();
        let Some(optimum) = bfs_within(&board, 10_000) else {
            continue;
        };
        match optimum {
            Some(_) => solvable += 1,
            None => unsolvable += 1,
        }
        let full = engines_agree(&board, &rows, optimum, false);
        // Stop the exact engine midway and one node short of finishing.
        for max_states in [full / 2, full - 1] {
            if max_states == 0 {
                continue;
            }
            let context = format!("{rows:?} at {max_states} states");
            let mut search = drive(&board, Mode::Optimal, max_states, optimum, &context);
            assert_consistent(&board, &mut search, optimum, &context);
        }
    }
    assert!(
        solvable >= 40 && unsolvable >= 40,
        "{solvable} solvable, {unsolvable} unsolvable"
    );
}

/// Frozen move-optima ported from the reference solver's fixtures
/// (`tests/fixtures/solver-v2/{known-optima,benchmark-corpus,tunnel-soundness,
/// pattern-deadlock-soundness}.ts`), run through the exact kernel directly.
/// Each move count is the reference's independent step-oracle output; push
/// counts are deliberately not asserted because move-optimal search does not
/// optimize them. The large catalog boards, whose optima the reference only
/// established with machinery this port does not carry yet, stay out of the
/// gate until the exact kernel can reach them.
mod fixtures {
    use super::*;

    macro_rules! fixtures {
        (@moves None) => {
            None
        };
        (@moves $moves:literal) => {
            Some($moves)
        };
        ($($name:ident: $rows:literal => $moves:tt,)*) => {$(
            #[test]
            fn $name() {
                check(stringify!($name), $rows, fixtures!(@moves $moves));
            }
        )*};
    }

    fn check(name: &str, rows: &str, optimum: Option<u32>) {
        let board = Board::parse(rows).unwrap();
        let mut engine = ExactSearch::new(board.clone(), board.initial(), 1_000_000, 256).unwrap();
        while engine.status() == Status::Running {
            engine.advance(4096);
        }
        let finished = if optimum.is_some() {
            Status::Solved
        } else {
            Status::Exhausted
        };
        assert_eq!(engine.status(), finished, "{name}");
        assert_eq!(engine.best_moves(), optimum, "{name}");
        let proof = optimum.map_or(Proof::Unsolvable, |moves| Proof::Optimal { moves });
        assert_eq!(engine.proof(), Some(proof), "{name}");
        let route = engine.solution().unwrap();
        match optimum {
            Some(moves) => assert_route(&board, &route.expect(name), moves, name),
            None => assert_eq!(route, None, "{name}"),
        }
    }

    fixtures! {
        // Reference catalog snapshots.
        ultra_tiny: "OOOOO\nO R O\nO A O\nO a O\nOOOOO" => 1,
        tiny: "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO" => 20,
        tutorial_push: "OOOOO\nO XSO\nO   O\nO R O\nOOOOO" => 4,
        tutorial_corner: "OOOOOO\nO    O\nO RX O\nO  S O\nO    O\nOOOOOO" => 3,
        tutorial_around: "OOOOOOO\nOR    O\nOOOOX O\nO   S O\nOOOOOOO" => 4,
        beginner_three: "OOOOOOOO\nO R    O\nO XXXO O\nO SSSO O\nO      O\nOOOOOOOO" => 7,
        beginner_detour: "OOOOOOOO\nOR     O\nOOOO X O\nOS   X O\nOS     O\nOOOOOOOO" => 24,
        beginner_typed_line: "OOOOOOOOO\nOc b a  O\nO       O\nO A B C O\nO   R   O\nOOOOOOOOO" => 27,
        garden_1: "OOOOOOOOO\nO   R   O\nO A B C O\nO       O\nO a b c O\nOOOOOOOOO" => 16,
        box_5x5_a: "OOOOO\nOSX O\nO XRO\nO  SO\nOOOOO" => 6,
        medium: "OOOOOOO\nOa   bO\nO AXB O\nO XRX O\nOSCXDSO\nOcS SdO\nOOOOOOO" => 34,
        inter_rooms: "OOOOOOOOOOO\nO    O    O\nO RX   XS O\nO XO O OX O\nOSSO   OS O\nOOOOOOOOOOO" => 28,
        corridor_2: "OOOOOOOOOOO\nO S O     O\nO   O X   O\nO     R   O\nO   O X   O\nO S O     O\nOOOOO X   O\nOOOOOO  S O\nOOOOOOOOOOO" => 41,
        garden_2: "OOOOOOOOOOO\nO    R    O\nO OOO OOO O\nO A     B O\nO OOO OOO O\nO  b   a  O\nO OO O OO O\nO         O\nOOOOOOOOOOO" => 73,
        workshop_1: "OOOOOOO\nO   R O\nO OXO O\nO X   O\nOSX   O\nOS    O\nOS    O\nOOOOOOO" => 23,
        classic_1: "OOOOOOO\nO     O\nO OXO O\nO  X  O\nOO X OO\nO  R  O\nO SSS O\nOOOOOOO" => 40,
        theme_kitchen: "OOOOOOOOO\nO R     O\nO  OOO  O\nO X O X O\nO  O    O\nO  O  X O\nO SSS   O\nOOOOOOOOO" => 34,
        adv_rotary: "OOOOOOOOOOO\nOOa  ROOOOO\nOO  OO  bOO\nO A    B  O\nO   OO    O\nOOOOOOOOOOO" => 17,
        adv_four_color: "OOOOOOOOO\nOb     cO\nO       O\nO  C  D O\nO   R   O\nO  A  B O\nO       O\nOd     aO\nOOOOOOOOO" => 43,
        adv_gallery: "OOOOOOOOOO\nO R      O\nO OOOOOO O\nO O    O O\nO X SS X O\nO O    O O\nO OXOOXO O\nO        O\nO   SS   O\nOOOOOOOOOO" => 29,
        box_7x7: "OOOOOOO\nOS   SO\nO  X  O\nO XRXOO\nO  X  O\nOS   SO\nOOOOOOO" => 21,
        sym_diamond: "OOOOOOOOOOO\nOOOOO OOOOO\nOOOO   OOOO\nOOO  S  OOO\nOO  XRX  OO\nO    X    O\nOO   S   OO\nOOO  S  OOO\nOOOO   OOOO\nOOOOO OOOOO\nOOOOOOOOOOO" => 16,
        theme_library: "OOOOOOOOOOO\nOaaO R ObbO\nO  O   O  O\nO  OO OO  O\nO   A B   O\nO  A   B  O\nO         O\nOOOOOOOOOOO" => 45,
        expert_maze: "OOOOOOOOOOOO\nO R  O     O\nOOO  O OOO O\nO X  O O S O\nO OO   O   O\nO O  OOOO  O\nO   XO  X  O\nOOOO OS    O\nO  X    OO O\nO SSS X    O\nOOOOOOOOOOOO" => 65,
        expert_tetris: "OOOOOOOOO\nO   R   O\nO  X X  O\nOOX   XOO\nOO     OO\nOO X X OO\nOOSSSSSOO\nOO  S  OO\nOOOOOOOOO" => 38,

        // Reference supplemental fixtures.
        v2_microban_145: "OOOOOO\nO    O\nO XX O\nO  X O\nO R  O\nOSSSOO\nOOOOOO" => 23,
        v2_microban_146: "OOOOOOO\nOS R SO\nO  XX O\nOO O OO\nO     O\nOO X OO\nOO S OO\nOOOOOOO" => 23,
        v2_caleb_022: "OOOOOOOO\nO R    O\nO  OX  O\nO    X O\nOOX  OOO\nOSS    O\nOSS X OO\nOOOOOOOO" => 45,
        v2_solved_box_must_move: "OOOOOOO\nO     O\nOOS R O\nO X   O\nO X   O\nO S   O\nOOOOOOO" => 14,
        v2_assignment_infeasible: "OOOOOOO\nO   R O\nO X   O\nOOO   O\nO S X O\nO S   O\nOOOOOOO" => 17,
        v2_sealed_corral: "OOOOOOO\nO R   O\nO X X O\nOOO OOO\nO  S  O\nO  S  O\nOOOOOOO" => 19,
        v2_wide_multi_entry: "OOOOOOOOOOOOO\nO    O      O\nOR X   S    O\nO    OOOOO OO\nO  X   S    O\nO    OOOOO OO\nO  X   S    O\nO    O      O\nOOOOOOOOOOOOO" => 25,
        v2_loop_heavy: "OOOOOOOOOOOOO\nO   R       O\nO O O O O O O\nO     X     O\nO O O O O O O\nO           O\nO O O O O O O\nO     X     O\nO O O O O O O\nO  S     S  O\nOOOOOOOOOOOOO" => 32,

        // Reference soundness boards: tunnel-macro and pattern-deadlock
        // regressions. This port does not carry those mechanisms, but the
        // boards must still produce their oracle optima under any pruning it
        // does have.
        es01: "OOOOOOOOO\nOOcOOOOOO\nOOCOOOOOO\nORA    aO\nOXOOOOOOO\nOSOOOOOOO\nO OOOOOOO\nOOOOOOOOO" => 9,
        es01a: "OOOOOOOOOO\nOOO      O\nOOO OOOO O\nOSRX     O\nOOOOOOOOOO" => 16,
        es01c: "OOOOOOOOOO\nOOO      O\nOOO OOOO O\nOSRX     O\nOO OOOOOOO\nOO XSOOOOO\nOO   OOOOO\nOOOOOOOOOO" => 19,
        es01d: "OOOOOOOOOO\nOOO      O\nOOO OOOO O\nOSRX     O\nOO OOOOOOO\nO     OOOO\nO X S OOOO\nO     OOOO\nOOOOOOOOOO" => 22,
        fz_unsolvable: "OOOOOOOO\nO      O\nO OOO aO\nO    AaO\nOOOOOOAO\nOOOOOORO\nOOOOOOOO" => 15,
        fz_corridor: "OOOOO\nO  aO\nO   O\nOOO O\nO O O\nObB O\nOSXAO\nO R O\nOOOOO" => 10,
        pd_false_optimum: "OOOOOOOOOOOOOOOOOOOOOOO\nOO  OOO  OOOOOOOOOOOOOO\nOO                R OOO\nOO OOOO O OOOOOOO O OOO\nOO OOOO O OOOOOOO O OOO\nOO OOOO O OOOOOOO O OOO\nOO OOOO O OOOOOOOXO OOO\nOS        XSO       XSO\nOOOOOOOOOOOOOOOOOOOOOOO" => 63,
        pd_false_unsolvable: "OOOOOOOOOOOOOOOOOOOOOOO\nOOOOOOO  OOOOOOOOOOOOOO\nOOOOOOO           R OOO\nOOOOOOO O OOOOOOO O OOO\nOOOOOOO O OOOOOOO O OOO\nOOOOOOO O OOOOOOO O OOO\nOOOOOOO O OOOOOOOXO OOO\nOS        XSO       XSO\nOOOOOOOOOOOOOOOOOOOOOOO" => 63,

        // B can only be pushed deeper into the one-wide corridor and the robot
        // can never get behind it, so A can never pass B. No box starts on a
        // dead square, so only an exhausted frontier proves it.
        blocked_typed_corridor: "OOOOOOOOO\nOabB    O\nOOOO    O\nOOOO A  O\nOOOO  R O\nOOOOOOOOO" => None,
    }
}
