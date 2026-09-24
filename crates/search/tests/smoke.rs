use sokomind_core::{Board, Game, State};
use sokomind_search::{Mode, Proof, Search, Status};
use std::collections::{HashSet, VecDeque};

const FIRST: &str = "OOOOO\nO R O\nO A O\nO a O\nOOOOO";
const TWO: &str = "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO";

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

#[test]
fn exact_routes_match_primitive_bfs() {
    for rows in [
        FIRST,
        TWO,
        "OOOOOOO\nOR    O\nO XX  O\nO SS  O\nOOOOOOO",
        "OOOOOOO\nORX  SO\nOOOOOOO",
    ] {
        let board = Board::parse(rows).unwrap();
        let expected = bfs(&board);
        let mut search =
            Search::new(board.clone(), board.initial, Mode::Optimal, 20_000, 8).unwrap();
        while search.status() == Status::Running {
            search.advance(32);
        }
        assert_eq!(search.best_moves(), expected);
        let mut game = Game::new(board);
        game.replay(&search.solution().unwrap().unwrap()).unwrap();
        assert!(game.solved());
        assert_eq!(
            search.proof(),
            Some(Proof::Optimal {
                moves: expected.unwrap()
            })
        );
    }
}

#[test]
fn bounded_and_cancelled_searches_do_not_claim_proof() {
    let board = Board::parse(TWO).unwrap();
    let mut search = Search::new(board.clone(), board.initial, Mode::Optimal, 1, 4).unwrap();
    search.advance(32);
    assert_eq!(search.status().as_str(), "state_limit");
    assert_eq!(search.generated(), 1);
    assert_eq!(search.proof(), None);
    let mut search = Search::new(board.clone(), board.initial, Mode::Quality, 20_000, 8).unwrap();
    let mut best = u32::MAX;
    while search.status() == Status::Running {
        search.advance(1);
        let current = search.best_moves().unwrap_or(u32::MAX);
        assert!(current <= best);
        best = current;
    }
    assert!(best < u32::MAX);
    assert_eq!(search.proof(), None);
    assert_eq!(search.lower_bound(), None);
    let mut search = Search::new(board.clone(), board.initial, Mode::Optimal, 20_000, 8).unwrap();
    search.stop(Status::Cancelled);
    search.advance(32);
    assert_eq!(search.generated(), 1);
    assert_eq!(search.proof(), None);
}

#[test]
fn rules_undo_labels_and_atomic_replay() {
    let mut game = Game::new(Board::parse(FIRST).unwrap());
    assert!(!game.step(0));
    assert!(game.step(1));
    assert!(game.solved());
    assert_eq!(game.pushes, 1);
    assert!(!game.step(2)); // Reference sessions stop accepting moves once solved.
    assert!(game.undo());
    assert!(!game.solved());
    assert_eq!(game.moves(), 0);
    assert!(game.replay("DU").is_err());
    assert_eq!(game.moves(), 0);
    assert!(Board::parse("OOOOOO\nOR A O\nO  b O\nOOOOOO").is_err());
    assert!(Board::parse("OOOOOO\nORRXS O\nOOOOOO").is_err());
    let board = Board::parse("OOOOOO\nOR XS\nOO").unwrap();
    assert_eq!(board.tiles[17], 255); // Ragged rows are padded with walls.
}
