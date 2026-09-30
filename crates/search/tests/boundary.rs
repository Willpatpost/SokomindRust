//! The search API's edges: malformed positions, the exported limits, error
//! messages, interruption, and the stats and proof wire names.

use sokomind_core::{Board, NONE, StateError};
use sokomind_search::{
    MAX_STATES, MAX_STATES_RANGE, MEMORY_MIB_RANGE, Mode, Proof, Search, SearchError, SearchStats,
    SolutionError, Status, StopReason,
};

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
        for mode in Mode::ALL {
            assert!(matches!(Search::new(board.clone(), state, mode, 100, 4),
                Err(SearchError::InvalidState(error)) if error == expected));
        }
    }
}

#[test]
fn limits_follow_the_exported_ranges() {
    assert_eq!(*MAX_STATES_RANGE.end(), MAX_STATES);
    let board = Board::parse(BOARD).unwrap();
    let (states, memory) = (MAX_STATES_RANGE, MEMORY_MIB_RANGE);
    for mode in Mode::ALL {
        let search = |max_states: usize, memory_mib: usize| {
            Search::new(board.clone(), board.initial(), mode, max_states, memory_mib)
        };
        assert!(search(*states.end(), *memory.start()).is_ok());
        assert!(search(*states.start(), *memory.end()).is_ok());
        for (max_states, memory_mib) in [
            (states.end() + 1, *memory.start()),
            (states.start() - 1, *memory.start()),
            (100, memory.start() - 1),
            (100, memory.end() + 1),
        ] {
            assert_eq!(
                search(max_states, memory_mib).err(),
                Some(SearchError::Limits),
                "{mode:?}: {max_states} states, {memory_mib} MiB"
            );
        }
    }
}

/// The web app shows these texts when the WASM search refuses to start or
/// to hand over a route, so the typed variants keep the old wording.
#[test]
fn search_errors_keep_their_messages() {
    let cases = [
        (
            SearchError::Limits.to_string(),
            "Use 1..60000000 states and 4..256 MiB",
        ),
        (
            SearchError::BudgetTooSmall.to_string(),
            "Memory budget is too small",
        ),
        (
            SearchError::Allocation("node arena").to_string(),
            "Cannot reserve node arena",
        ),
        (
            SolutionError::TooLong.to_string(),
            "Solution exceeds the 100000-move replay limit",
        ),
        (
            SolutionError::Replay.to_string(),
            "Internal route replay failed",
        ),
        (
            SolutionError::Counters.to_string(),
            "Internal solution counters failed",
        ),
    ];
    for (message, expected) in cases {
        assert_eq!(message, expected);
    }
}

#[test]
fn interruption_cannot_manufacture_a_verdict_in_release() {
    let board = Board::parse("OOOOO\nO R O\nO A O\nO a O\nOOOOO").unwrap();
    for reason in [StopReason::Cancelled, StopReason::TimeLimit] {
        let mut exact = Search::new(board.clone(), board.initial(), Mode::Optimal, 100, 4).unwrap();
        exact.stop(reason);
        exact.advance(100);
        assert_eq!(exact.proof(), None);
        assert_eq!(exact.lower_bound(), Some(1));
        assert!(matches!(
            exact.status(),
            Status::Cancelled | Status::TimeLimit
        ));
    }
    let mut exact = Search::new(board.clone(), board.initial(), Mode::Optimal, 100, 4).unwrap();
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

#[test]
fn stats_values_follow_field_order_and_proofs_name_their_wire_kind() {
    // The wire names the server, the WASM diagnostics and the corpus share.
    assert_eq!(
        SearchStats::FIELDS,
        [
            "unique_states",
            "duplicate_improvements",
            "reopened_states",
            "stale_pops",
            "peak_queue",
            "pruned_dead_cells",
            "pruned_deadlocks",
            "pruned_duplicates",
            "pruned_assignment",
            "pruned_bound",
        ]
    );
    let stats = SearchStats {
        unique_states: 1,
        duplicate_improvements: 2,
        reopened_states: 3,
        stale_pops: 4,
        peak_queue: u32::MAX,
        pruned_dead_cells: 6,
        pruned_deadlocks: 7,
        pruned_duplicates: 8,
        pruned_assignment: 9,
        pruned_bound: u64::MAX,
    };
    assert_eq!(
        stats.values(),
        [1, 2, 3, 4, u64::from(u32::MAX), 6, 7, 8, 9, u64::MAX]
    );
    assert_eq!(SearchStats::default().values(), [0; 10]);
    let proofs = [
        Proof::Bounded {
            lower_bound: 1,
            upper_bound: 2,
        },
        Proof::Optimal { moves: 2 },
        Proof::Unsolvable,
    ];
    assert_eq!(
        proofs.map(Proof::kind),
        ["bounded", "optimal", "unsolvable"]
    );
}
