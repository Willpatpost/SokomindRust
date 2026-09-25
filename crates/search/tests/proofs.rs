use sokomind_core::{Board, Game, State};
use sokomind_search::{ExactSearch, Mode, Proof, Search, Status};
use std::collections::{HashSet, VecDeque};

const FIRST: &str = "OOOOO\nO R O\nO A O\nO a O\nOOOOO";
const TWO: &str = "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO";
/// The box is frozen against the top-right wall and can never reach its goal.
const CORNERED: &str = "OOOOO\nOR XO\nOS  O\nOOOOO";
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

// Independent primitive-move BFS oracle (not a second push-search implementation).
fn bfs(board: &Board) -> Option<u32> {
    bfs_within(board, usize::MAX).unwrap()
}

/// `bfs`, or `None` once it has seen more than `cap` states.
fn bfs_within(board: &Board, cap: usize) -> Option<Option<u32>> {
    let key = |s: State| (s.player, s.boxes);
    let mut queue = VecDeque::from([(board.initial, 0)]);
    let mut seen = HashSet::from([key(board.initial)]);
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

fn run(board: &Board, mode: Mode) -> Search {
    let mut search = Search::new(board.clone(), board.initial, mode, 20_000, 8).unwrap();
    while search.status() == Status::Running {
        search.advance(64);
    }
    search
}

/// Runs a search in small slices, checking the live lower bound after each.
fn drive(
    board: &Board,
    mode: Mode,
    max_states: usize,
    optimum: Option<u32>,
    context: &str,
) -> Search {
    let mut search = Search::new(board.clone(), board.initial, mode, max_states, 8).unwrap();
    while search.status() == Status::Running {
        search.advance(4);
        if let (Some(bound), Some(optimum)) = (search.lower_bound(), optimum) {
            assert!(
                bound <= optimum,
                "live bound {bound} > {optimum}: {context}"
            );
        }
    }
    search
}

/// Everything a finished search claims must agree with the BFS optimum
/// (`None` when unsolvable): its bound, its certificate, and its route.
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
        return;
    };
    let optimum = optimum.unwrap_or_else(|| panic!("route on an unsolvable board: {context}"));
    assert!(best >= optimum, "{context}");
    let route = search.solution().unwrap().unwrap();
    assert_eq!(route.len(), best as usize, "{context}");
    let mut game = Game::new(board.clone());
    game.replay(&route).unwrap();
    assert!(game.solved(), "{context}");
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

#[test]
fn exact_kernel_can_be_driven_directly() {
    let board = Board::parse(FIRST).unwrap();
    let expected = bfs(&board).unwrap();
    let mut engine = ExactSearch::new(board.clone(), board.initial, 20_000, 8).unwrap();
    while engine.status() == Status::Running {
        engine.advance(8);
    }
    assert_eq!(engine.proof(), Some(Proof::Optimal { moves: expected }));
    let mut game = Game::new(board);
    game.replay(&engine.solution().unwrap().unwrap()).unwrap();
    assert!(game.solved());
}

#[test]
fn drained_frontier_proves_unsolvable() {
    let board = Board::parse(CORNERED).unwrap();
    assert_eq!(bfs(&board), None);
    let mut search = run(&board, Mode::Optimal);
    assert_eq!(search.status(), Status::Exhausted);
    assert_eq!(search.proof(), Some(Proof::Unsolvable));
    assert_eq!(search.solution().unwrap(), None);
    // The bounded engine must not inherit the unsolvable claim.
    let bounded = run(&board, Mode::Fast);
    assert_eq!(bounded.status(), Status::Exhausted);
    assert_eq!(bounded.proof(), None);
}

#[test]
fn interrupted_optimal_keeps_a_sound_gap() {
    let board = Board::parse(TWO).unwrap();
    let mut search = Search::new(board.clone(), board.initial, Mode::Optimal, 20_000, 8).unwrap();
    // The first incumbent always appears when the solved child is generated,
    // at least one expansion before it can pop, so the stop lands mid-run.
    while search.status() == Status::Running && search.best_moves().is_none() {
        search.advance(1);
    }
    assert!(search.best_moves().is_some());
    search.stop(Status::TimeLimit);
    assert_eq!(search.status(), Status::TimeLimit);
    let best = search.best_moves().unwrap();
    match search.proof() {
        Some(Proof::Bounded {
            lower_bound,
            upper_bound,
        }) => {
            assert_eq!(upper_bound, best);
            assert!(lower_bound <= upper_bound);
        }
        Some(Proof::Optimal { moves }) => assert_eq!(moves, best),
        other => panic!("expected a certificate, got {other:?}"),
    }
    assert!(search.lower_bound().is_some_and(|bound| bound <= best));
    let route = search.solution().unwrap().unwrap();
    let mut game = Game::new(board);
    game.replay(&route).unwrap();
    assert!(game.solved());
    assert_eq!(route.len(), best as usize);
}

#[test]
fn bounded_modes_report_no_proof() {
    let board = Board::parse(TWO).unwrap();
    for mode in [Mode::Fast, Mode::Quality] {
        let mut search = run(&board, mode);
        assert_eq!(search.status(), Status::Solved);
        assert_eq!(search.proof(), None);
        assert_eq!(search.lower_bound(), None);
        assert!(search.solution().unwrap().is_some());
    }
}

#[test]
fn bounded_certificates_reject_inverted_bounds() {
    assert!(Proof::bounded(5, 3).is_err());
    assert_eq!(
        Proof::bounded(3, 5).unwrap(),
        Proof::Bounded {
            lower_bound: 3,
            upper_bound: 5
        }
    );
}

#[test]
fn solutions_survive_exact_limit_boundaries() {
    // Sweep the state limit across the boundary where a solution child is
    // discovered at the exact moment the arena fills.
    let board = Board::parse(TWO).unwrap();
    let mut kept = 0;
    for max_states in 1..=40 {
        let mut search =
            Search::new(board.clone(), board.initial, Mode::Fast, max_states, 8).unwrap();
        while search.status() == Status::Running {
            search.advance(16);
        }
        let Some(moves) = search.best_moves() else {
            assert_eq!(
                search.status(),
                Status::StateLimit,
                "at {max_states} states"
            );
            continue;
        };
        if search.status() == Status::StateLimit {
            kept += 1;
        }
        let route = search.solution().unwrap().unwrap();
        assert_eq!(route.len(), moves as usize, "at {max_states} states");
        let mut game = Game::new(board.clone());
        game.replay(&route).unwrap();
        assert!(game.solved(), "at {max_states} states");
    }
    assert!(kept > 0, "no state limit kept the solution it found");
}

#[test]
fn same_label_reordering_keeps_proofs_sound() {
    for (rows, moves) in REORDERED {
        let board = Board::parse(rows).unwrap();
        assert_eq!(bfs(&board), Some(moves), "{rows:?}");
        for mode in [Mode::Optimal, Mode::Fast, Mode::Quality] {
            let mut search = run(&board, mode);
            assert_eq!(search.status(), Status::Solved, "{rows:?} {mode:?}");
            let best = search.best_moves().unwrap();
            if mode == Mode::Optimal {
                assert_eq!(search.proof(), Some(Proof::Optimal { moves }), "{rows:?}");
            } else {
                assert!(best >= moves, "{rows:?} {mode:?}");
            }
            let route = search.solution().unwrap().unwrap();
            assert_eq!(route.len(), best as usize, "{rows:?} {mode:?}");
            let mut game = Game::new(board.clone());
            game.replay(&route).unwrap();
            assert!(game.solved(), "{rows:?} {mode:?}");
        }
    }
}

/// Sweeps every state limit up to the unlimited run's node count. A limit
/// always cuts an expansion short, so the frontier must count that node's
/// unpushed children (`interrupted_g`); some limits land exactly as the solved
/// child needs the arena's spare node (`push_final`).
#[test]
fn limited_optimal_bounds_never_pass_the_optimum() {
    let (mut kept, mut spare) = (0, 0);
    for (rows, optimum) in [(CROSSING, 9), (TWO, 20)].into_iter().chain(REORDERED) {
        let board = Board::parse(rows).unwrap();
        assert_eq!(bfs(&board), Some(optimum), "{rows:?}");
        let full = run(&board, Mode::Optimal).generated() as usize;
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
            let Some(best) = search.best_moves() else {
                assert_eq!(search.proof(), None, "{context}");
                continue;
            };
            kept += 1;
            // Only the spare node can take the arena past its limit.
            if search.generated() as usize == max_states + 1 {
                spare += 1;
            }
            match search.proof() {
                Some(Proof::Optimal { moves }) => assert_eq!(moves, optimum, "{context}"),
                Some(Proof::Bounded {
                    lower_bound,
                    upper_bound,
                }) => {
                    assert_eq!(upper_bound, best, "{context}");
                    assert!(
                        lower_bound <= optimum && optimum <= upper_bound,
                        "{context}"
                    );
                }
                other => panic!("{other:?} with an incumbent: {context}"),
            }
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
        let mut full = 1;
        for mode in [Mode::Optimal, Mode::Fast, Mode::Quality] {
            let context = format!("{mode:?} on {rows:?}");
            let mut search = drive(&board, mode, 20_000, optimum, &context);
            assert_consistent(&board, &mut search, optimum, &context);
            if mode == Mode::Optimal {
                full = search.generated() as usize;
            } else {
                assert_eq!(search.proof(), None, "{context}");
                assert_eq!(search.lower_bound(), None, "{context}");
            }
            // A run cut short by a limit only has to stay consistent.
            if matches!(search.status(), Status::StateLimit | Status::MemoryLimit) {
                continue;
            }
            let finished = if optimum.is_some() {
                Status::Solved
            } else {
                Status::Exhausted
            };
            assert_eq!(search.status(), finished, "{context}");
            if mode == Mode::Optimal {
                let proof = optimum.map_or(Proof::Unsolvable, |moves| Proof::Optimal { moves });
                assert_eq!(search.proof(), Some(proof), "{context}");
            }
        }
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
