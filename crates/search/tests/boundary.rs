use sokomind_core::{Board, NONE, StateError};
use sokomind_search::{ExactSearch, Mode, Proof, Search, SearchError, Status, StopReason};

const BOARD: &str = "OOOOOO\nOR   O\nO XX O\nO SS O\nOOOOOO";

#[test]
fn malformed_positions_return_structured_errors_in_every_mode() {
    let board = Board::parse(BOARD).unwrap();
    let start = board.initial();
    let mut cases = Vec::new();
    let mut state = start;
    state.player = NONE;
    cases.push((state, StateError::PlayerOutOfBounds { cell: NONE }));
    state = start;
    state.player = 0;
    cases.push((state, StateError::PlayerOnWall { cell: 0 }));
    state = start;
    state.boxes[0] = NONE;
    cases.push((
        state,
        StateError::BoxOutOfBounds {
            index: 0,
            cell: NONE,
        },
    ));
    state = start;
    state.boxes[0] = 0;
    cases.push((state, StateError::BoxOnWall { index: 0, cell: 0 }));
    state = start;
    state.player = state.boxes[0];
    cases.push((
        state,
        StateError::PlayerOnBox {
            index: 0,
            cell: state.player,
        },
    ));
    state = start;
    state.boxes[1] = state.boxes[0];
    cases.push((
        state,
        StateError::OverlappingBoxes {
            first: 0,
            second: 1,
            cell: state.boxes[0],
        },
    ));
    state = start;
    state.boxes[31] = start.player;
    cases.push((
        state,
        StateError::InactiveBox {
            index: 31,
            cell: start.player,
        },
    ));
    for (state, expected) in cases {
        for mode in [Mode::Fast, Mode::Quality, Mode::Optimal] {
            assert!(matches!(Search::new(board.clone(), state, mode, 100, 4),
                Err(SearchError::InvalidState(error)) if error == expected));
        }
        assert!(matches!(ExactSearch::new(board.clone(), state, 100, 4),
            Err(SearchError::InvalidState(error)) if error == expected));
    }
}

#[test]
fn interruption_cannot_manufacture_a_verdict_in_release() {
    let board = Board::parse("OOOOO\nO R O\nO A O\nO a O\nOOOOO").unwrap();
    for reason in [StopReason::Cancelled, StopReason::TimeLimit] {
        let mut exact = ExactSearch::new(board.clone(), board.initial(), 100, 4).unwrap();
        exact.stop(reason);
        exact.advance(100);
        assert_eq!(exact.proof(), None);
        assert_eq!(exact.lower_bound(), Some(1));
        assert!(matches!(
            exact.status(),
            Status::Cancelled | Status::TimeLimit
        ));
    }
    let mut exact = ExactSearch::new(board.clone(), board.initial(), 100, 4).unwrap();
    exact.advance(100);
    exact.stop(StopReason::Cancelled);
    assert_eq!(exact.status(), Status::Solved);
    assert_eq!(exact.proof(), Some(Proof::Optimal { moves: 1 }));
}

#[test]
fn same_label_box_order_is_normalized_and_stats_account_for_versions() {
    let board = Board::parse(BOARD).unwrap();
    let mut start = board.initial();
    start.boxes.swap(0, 1);
    let mut search = Search::new(board, start, Mode::Optimal, 1000, 4).unwrap();
    while search.status() == Status::Running {
        search.advance(1);
    }
    assert_eq!(search.proof(), Some(Proof::Optimal { moves: 5 }));
    assert_eq!(search.solution().unwrap().unwrap().len(), 5);
    let stats = search.stats();
    assert_eq!(
        stats.unique_states + stats.duplicate_improvements,
        search.generated()
    );
    assert!(stats.reopened_states <= stats.duplicate_improvements);
    assert!(stats.stale_pops <= stats.duplicate_improvements);
    assert!(stats.peak_queue <= search.generated());
}
