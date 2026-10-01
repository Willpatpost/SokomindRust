//! Game rules through the public API: route limits, prefix replay, board
//! steps, state validation, undo, reset, and replay errors.

use sokomind_core::{
    Board, Cell, Game, MAX_ROUTE, NONE, ParseError, ReplayError, State, StateError, Step,
    decode_direction,
};

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
    assert_eq!(game.state(), game.board().initial());
    // The budget, not the board, refuses the next legal step.
    assert!(!game.step(2));
    assert!(game.undo());
    assert!(game.step(3));
    assert!(!game.step(2));
    assert_eq!(game.moves() as usize, MAX_ROUTE);
    // One move over is refused atomically.
    assert_eq!(game.replay(&format!("{full}L")), Err(ReplayError::TooLong));
    assert_eq!(game.moves() as usize, MAX_ROUTE);
    assert_eq!(game.actions(), full);
}

#[test]
fn at_replays_a_prefix_and_refuses_a_full_unsolved_one() {
    let refused = |rows: &str, actions: &str| match Game::at(rows, actions) {
        Ok(_) => panic!("{} actions should be refused", actions.len()),
        Err(error) => error,
    };
    // A board error carries `Board::parse`'s error; replay errors keep
    // the index of the refused action.
    assert_eq!(
        refused("", ""),
        ReplayError::InvalidBoard(ParseError::EmptyRow)
    );
    assert_eq!(
        refused(CROSSING_PAIR, "U"),
        ReplayError::Blocked { index: 0 }
    );
    assert_eq!(
        refused(CROSSING_PAIR, "DX"),
        ReplayError::InvalidAction { index: 1 }
    );
    // A played prefix leaves the game where its actions left it.
    let game = Game::at(CROSSING_PAIR, "DL").unwrap();
    assert_eq!((game.moves(), game.pushes(), game.actions()), (2, 1, "DL"));
    assert!(!game.solved());
    // Unsolved at full length, the position can only be extended past the
    // limit; one move shorter it still has room for exactly one more.
    let pacing = "LR".repeat(MAX_ROUTE / 2);
    assert_eq!(refused(CORRIDOR, &pacing), ReplayError::PastLimit);
    let short = Game::at(CORRIDOR, &pacing[..MAX_ROUTE - 1]).unwrap();
    assert_eq!(short.moves() as usize, MAX_ROUTE - 1);
    assert!(short.check_extension(1).is_ok());
    assert_eq!(short.check_extension(2), Err(ReplayError::PastLimit));
    // A position solved at full length needs no extension.
    let solved = Game::at(CORRIDOR, &format!("{}RR", &pacing[2..])).unwrap();
    assert!(solved.solved());
    assert_eq!(solved.moves() as usize, MAX_ROUTE);
    assert!(solved.check_extension(0).is_ok());
    assert_eq!(solved.check_extension(1), Err(ReplayError::PastLimit));
    // A fresh game has room for MAX_ROUTE moves, and a huge count cannot overflow.
    let fresh = Game::at(CORRIDOR, "").unwrap();
    assert!(fresh.check_extension(MAX_ROUTE as u32).is_ok());
    assert_eq!(
        fresh.check_extension(MAX_ROUTE as u32 + 1),
        Err(ReplayError::PastLimit)
    );
    assert_eq!(fresh.check_extension(u32::MAX), Err(ReplayError::PastLimit));
}

/// The server and the web app show these texts, so the typed variants keep
/// the wording the string errors had.
#[test]
fn replay_errors_keep_their_messages() {
    let cases = [
        (
            ReplayError::InvalidBoard(ParseError::EmptyRow),
            "Board rows cannot be empty",
        ),
        (
            ReplayError::TooLong,
            "Route is too long: the limit is 100000 moves",
        ),
        (
            ReplayError::InvalidAction { index: 3 },
            "Routes must use only U/D/L/R",
        ),
        (
            ReplayError::Blocked { index: 7 },
            "Blocked action at index 7",
        ),
        (
            ReplayError::PastLimit,
            "Position and route together exceed the 100000-move replay limit",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn board_step_reports_walks_and_pushes() {
    let board = Board::parse(CROSSING_PAIR).unwrap();
    let at = |row: usize, column: usize| (row * board.width() + column) as Cell;
    let start = board.initial();
    assert_eq!(start.boxes[..2], [at(2, 3), at(3, 2)]);
    // The wall above the robot and an out-of-range direction are refused,
    // leaving the state untouched.
    for direction in [0, 4] {
        let mut state = start;
        assert_eq!(board.step(&mut state, direction), None);
        assert_eq!(state, start);
    }
    // D pushes the upper box, which keeps its slot.
    let mut state = start;
    assert_eq!(board.step(&mut state, 1), Some(Step::Push(0)));
    let pushed = state;
    assert_eq!(pushed.player, at(2, 3));
    assert_eq!(pushed.boxes[..2], [at(3, 3), at(3, 2)]);
    // Pushing it again would put it in the wall.
    assert_eq!(board.step(&mut state, 1), None);
    assert_eq!(state, pushed);
    // A box cannot be pushed into another box either.
    let jammed = State {
        player: at(3, 4),
        ..pushed
    };
    let mut state = jammed;
    assert_eq!(board.step(&mut state, 2), None);
    assert_eq!(state, jammed);
    // The rest of DLDR: a walk, then a push of each box onto a goal.
    let mut state = pushed;
    assert_eq!(board.step(&mut state, 2), Some(Step::Walk));
    assert_eq!((state.player, state.boxes), (at(2, 2), pushed.boxes));
    assert_eq!(board.step(&mut state, 1), Some(Step::Push(1)));
    assert_eq!(board.step(&mut state, 3), Some(Step::Push(0)));
    assert_eq!(state.boxes[..2], [at(3, 4), at(4, 2)]);
    assert!(board.solved(&state));
}

/// Search checks every caller-built start state here, and a rejected one
/// reaches the server log only as this message, so each names its rule.
#[test]
fn validate_state_names_the_broken_rule() {
    let board = Board::parse(CROSSING_PAIR).unwrap();
    let start = board.initial();
    assert_eq!(board.validate_state(&start), Ok(()));
    // The robot starts on cell 9 and the boxes on 15 and 20; cell 0 is a wall.
    let player_at = |player: Cell| State { player, ..start };
    let box_at = |slot: usize, cell: Cell| {
        let mut state = start;
        state.boxes[slot] = cell;
        state
    };
    let cases = [
        (
            player_at(NONE),
            StateError::PlayerOutOfBounds { cell: NONE },
            "Invalid state: player is off the board at cell 65535",
        ),
        (
            player_at(0),
            StateError::PlayerOnWall { cell: 0 },
            "Invalid state: player is on a wall at cell 0",
        ),
        (
            box_at(1, NONE),
            StateError::BoxOutOfBounds {
                index: 1,
                cell: NONE,
            },
            "Invalid state: box 1 is off the board at cell 65535",
        ),
        (
            box_at(1, 0),
            StateError::BoxOnWall { index: 1, cell: 0 },
            "Invalid state: box 1 is on a wall at cell 0",
        ),
        (
            player_at(15),
            StateError::PlayerOnBox { index: 0, cell: 15 },
            "Invalid state: player is on box 0 at cell 15",
        ),
        (
            box_at(1, 15),
            StateError::OverlappingBoxes {
                first: 0,
                second: 1,
                cell: 15,
            },
            "Invalid state: boxes 0 and 1 share cell 15",
        ),
        (
            box_at(2, 9),
            StateError::InactiveBox { index: 2, cell: 9 },
            "Invalid state: unused box slot 2 holds cell 9",
        ),
    ];
    for (state, error, message) in cases {
        assert_eq!(board.validate_state(&state), Err(error), "{message}");
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn rules_undo_and_atomic_replay() {
    let board = Board::parse(CROSSING_PAIR).unwrap();
    let at = |row: usize, column: usize| (row * board.width() + column) as Cell;
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
    assert_eq!(decode_direction(b'X'), None);
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
    assert_eq!(game.state(), board.initial());
    // The restored game replays the same route to the same result.
    game.replay("DLDR").unwrap();
    assert_eq!(&snapshot(&game), trail.last().unwrap());
    // A route blocked by a wall, by the solved position, or by a bad action
    // is refused as a whole and leaves the finished game untouched. Each
    // replays from the start position, not the solved one, so DL is legal.
    for (route, error) in [
        ("DLU", ReplayError::Blocked { index: 2 }),
        ("DLDRU", ReplayError::Blocked { index: 4 }),
        ("DX", ReplayError::InvalidAction { index: 1 }),
    ] {
        assert_eq!(game.replay(route), Err(error), "{route}");
        assert_eq!(&snapshot(&game), trail.last().unwrap());
    }
}

/// The web app's Restart button and Replay best both start from `reset`,
/// which must forget the position, history, route and pushes together,
/// whether the game was mid-route or solved.
#[test]
fn reset_forgets_every_move() {
    let board = Board::parse(CROSSING_PAIR).unwrap();
    for route in ["DL", "DLDR"] {
        let mut game = Game::new(board.clone());
        game.replay(route).unwrap();
        assert!(game.pushes() > 0, "{route}");
        game.reset();
        assert_eq!(game.state(), board.initial(), "{route}");
        assert_eq!(
            (game.moves(), game.pushes(), game.actions()),
            (0, 0, ""),
            "{route}"
        );
        assert!(!game.solved(), "{route}");
        assert!(!game.undo(), "{route}");
        // Play resumes from the start with fresh counters.
        assert!(game.step(decode_direction(b'D').unwrap()), "{route}");
        assert_eq!(
            (game.moves(), game.pushes(), game.actions()),
            (1, 1, "D"),
            "{route}"
        );
    }
}
