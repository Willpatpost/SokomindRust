use sokomind_core::{Board, Cell, Game, MAX_ROUTE, decode_direction};

/// The robot can pace left and right forever without touching the box.
const CORRIDOR: &str = "OOOOOOO\nO R XSO\nOOOOOOO";
/// Pushing the first box down carries it past its same-label partner, so the
/// live box order stops matching the canonical sorted order. Solved by DLDR.
const CROSSING_PAIR: &str = "OOOOOO\nOOOROO\nOO XOO\nOOX SO\nOOSOOO\nOOOOOO";

#[test]
fn routes_stop_at_max_route_moves() {
    assert_eq!(MAX_ROUTE, 100_000);
    let mut game = Game::new(Board::parse(CORRIDOR).unwrap());
    let full = "LR".repeat(MAX_ROUTE / 2);
    game.replay(&full).unwrap();
    assert_eq!(game.moves() as usize, MAX_ROUTE);
    assert_eq!(game.pushes(), 0);
    assert_eq!(game.state(), game.board().initial);
    // The budget, not the board, refuses the next legal step.
    assert!(!game.step(2));
    assert!(game.undo());
    assert!(game.step(3));
    assert!(!game.step(2));
    assert_eq!(game.moves() as usize, MAX_ROUTE);
    // One move over is refused atomically.
    let error = game.replay(&format!("{full}L")).unwrap_err();
    assert!(error.contains("too long"), "{error}");
    assert!(error.contains(&MAX_ROUTE.to_string()), "{error}");
    assert_eq!(game.moves() as usize, MAX_ROUTE);
    assert_eq!(game.actions(), full);
}

#[test]
fn rules_undo_and_atomic_replay() {
    let board = Board::parse(CROSSING_PAIR).unwrap();
    let at = |row: usize, column: usize| (row * board.width + column) as Cell;
    let snapshot = |game: &Game| {
        (
            game.state(),
            game.moves(),
            game.pushes(),
            game.actions().to_owned(),
        )
    };
    let mut game = Game::new(board.clone());
    let mut trail = vec![snapshot(&game)];
    // The wall above the robot refuses the step and changes nothing.
    assert!(!game.step(0));
    assert_eq!(snapshot(&game), trail[0]);
    for action in "DLDR".bytes() {
        assert!(game.step(decode_direction(action).unwrap()));
        trail.push(snapshot(&game));
    }
    assert!(game.solved());
    assert_eq!((game.moves(), game.pushes()), (4, 3));
    // Reference sessions stop accepting moves once solved, even legal ones.
    assert!(!game.step(0));
    assert_eq!(&snapshot(&game), trail.last().unwrap());
    // Undo records box indices into this live order, never a sorted one.
    assert_eq!(trail[1].0.boxes[..2], [at(3, 3), at(3, 2)]);
    for expected in trail.iter().rev().skip(1) {
        assert!(game.undo());
        assert_eq!(&snapshot(&game), expected);
    }
    assert!(!game.undo());
    assert_eq!(game.state(), board.initial);
    // The restored game replays the same route to the same result.
    game.replay("DLDR").unwrap();
    assert_eq!(&snapshot(&game), trail.last().unwrap());
    // A route blocked by a wall, by the solved position, or by a bad action
    // is refused as a whole and leaves the finished game untouched.
    for route in ["DLU", "DLDRU", "DX"] {
        assert!(game.replay(route).is_err(), "{route}");
        assert_eq!(&snapshot(&game), trail.last().unwrap());
    }
}
