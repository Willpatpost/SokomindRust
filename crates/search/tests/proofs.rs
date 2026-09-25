use sokomind_core::{Board, Game, State};
use sokomind_search::{ExactSearch, Mode, Proof, Search, Status};
use std::collections::{HashSet, VecDeque};

const FIRST: &str = "OOOOO\nO R O\nO A O\nO a O\nOOOOO";
const TWO: &str = "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO";
/// The box is frozen against the top-right wall and can never reach its goal.
const CORNERED: &str = "OOOOO\nOR XO\nOS  O\nOOOOO";

// Independent primitive-move BFS oracle (not a second push-search implementation).
fn bfs(board: &Board) -> Option<u32> {
    let key = |s: State| (s.player, s.boxes);
    let mut queue = VecDeque::from([(board.initial, 0)]);
    let mut seen = HashSet::from([key(board.initial)]);
    while let Some((state, moves)) = queue.pop_front() {
        if board.solved(&state) {
            return Some(moves);
        }
        for direction in 0..4 {
            let mut next = state;
            if board.step(&mut next, direction).is_some() && seen.insert(key(next)) {
                queue.push_back((next, moves + 1));
            }
        }
    }
    None
}

fn run(board: &Board, mode: Mode) -> Search {
    let mut search = Search::new(board.clone(), board.initial, mode, 20_000, 8).unwrap();
    while search.status() == Status::Running {
        search.advance(64);
    }
    search
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
    for max_states in 1..=40 {
        let mut search =
            Search::new(board.clone(), board.initial, Mode::Fast, max_states, 8).unwrap();
        while search.status() == Status::Running {
            search.advance(16);
        }
        if let Some(moves) = search.best_moves() {
            let route = search.solution().unwrap().unwrap();
            assert_eq!(route.len(), moves as usize, "at {max_states} states");
            let mut game = Game::new(board.clone());
            game.replay(&route).unwrap();
            assert!(game.solved(), "at {max_states} states");
        }
    }
}
