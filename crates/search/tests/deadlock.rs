use sokomind_core::{Board, Cell, MAX_BOXES, NONE, State};
use sokomind_search::Deadlock;

/// D is frozen on its goal once pushed down; pushing A left then freezes A
/// against it, while pushing A right solves.
const CHAIN: &str = "O    O\nODO  O\nOd AaO\nOOO RO\nOOOOOO";
/// Two boxes pushed against the wall form a wall/box 2x2 square.
const WALL_PAIR: &str = "OOOOOOO\nOR    O\nO AB  O\nO     O\nOa b  O\nOOOOOOO";

fn at(board: &Board, row: usize, column: usize) -> Cell {
    (row * board.width + column) as Cell
}
fn state(player: Cell, boxes: &[Cell]) -> State {
    let mut next = State {
        player,
        boxes: [NONE; MAX_BOXES],
    };
    next.boxes[..boxes.len()].copy_from_slice(boxes);
    next
}

#[test]
fn freeze_component_and_2x2_branches() {
    let board = Board::parse(CHAIN).unwrap();
    let d = at(&board, 2, 1);
    let a = at(&board, 2, 3);
    let index = board
        .labels
        .iter()
        .position(|&label| label == b'A')
        .unwrap();
    assert_eq!(board.initial.boxes[index], a);
    let mut deadlock = Deadlock::new(&board);
    // Before D reaches its goal, pushing A left is merely a legal retreat.
    let initial = &board.initial.boxes[..board.labels.len()];
    deadlock.refresh(initial);
    assert!(!deadlock.is_dead_after_push(&board, initial, index, a, at(&board, 2, 2)));
    // With D staged on its goal, the same push completes a frozen component.
    let staged = [a, d];
    deadlock.refresh(&staged);
    assert!(deadlock.is_dead_after_push(&board, &staged, index, a, at(&board, 2, 2)));
    // Pushing A down forms a wall/box 2x2 square.
    assert!(deadlock.is_dead_after_push(&board, &staged, index, a, at(&board, 3, 3)));
    // Pushing A up or onto its goal stays legal.
    assert!(!deadlock.is_dead_after_push(&board, &staged, index, a, at(&board, 1, 3)));
    assert!(!deadlock.is_dead_after_push(&board, &staged, index, a, at(&board, 2, 4)));
    // The completed freeze is visible at state level, and the solved state is not dead.
    let frozen = state(board.initial.player, &[at(&board, 2, 2), d]);
    deadlock.refresh(&frozen.boxes[..board.labels.len()]);
    assert!(deadlock.is_dead_state(&board, &frozen.boxes[..board.labels.len()]));
    let solved = state(board.initial.player, &[at(&board, 2, 4), d]);
    deadlock.refresh(&solved.boxes[..board.labels.len()]);
    assert!(!deadlock.is_dead_state(&board, &solved.boxes[..board.labels.len()]));
    assert!(board.solved(&solved));
}

#[test]
fn wall_pair_2x2_deadlock() {
    let board = Board::parse(WALL_PAIR).unwrap();
    let a = at(&board, 2, 2);
    let b = at(&board, 2, 3);
    let a_index = board
        .labels
        .iter()
        .position(|&label| label == b'A')
        .unwrap();
    let b_index = board
        .labels
        .iter()
        .position(|&label| label == b'B')
        .unwrap();
    let mut deadlock = Deadlock::new(&board);
    deadlock.refresh(&[a, b]);
    // One box against the wall can still be pushed away from it.
    assert!(!deadlock.is_dead_after_push(&board, &[a, b], a_index, a, at(&board, 1, 2)));
    // The second box completes a fully blocked wall/box square.
    let raised = [at(&board, 1, 2), b];
    deadlock.refresh(&raised);
    assert!(deadlock.is_dead_after_push(&board, &raised, b_index, b, at(&board, 1, 3)));
    let trapped = [at(&board, 1, 2), at(&board, 1, 3)];
    deadlock.refresh(&trapped);
    assert!(deadlock.is_dead_state(&board, &trapped[..board.labels.len()]));
}

/// Pushing the upper X down lands it after the other X in row-major order,
/// so the canonical child swaps their slots.
const CROSSING_PAIR: &str = "OOOOOO\nOOOROO\nOO XOO\nOOX SO\nOOSOOO\nOOOOOO";

#[test]
fn push_past_a_same_label_box_keeps_parent_order() {
    let board = Board::parse(CROSSING_PAIR).unwrap();
    let upper = at(&board, 2, 3);
    let lower = at(&board, 3, 2);
    let below = at(&board, 3, 3);
    assert_eq!(board.initial.boxes[..2], [upper, lower]);
    let mut deadlock = Deadlock::new(&board);
    deadlock.refresh(&[upper, lower]);
    // The first push of the 4-move solution, checked in parent order.
    assert!(!deadlock.is_dead_after_push(&board, &[below, lower], 0, upper, below));
    let mut child = state(upper, &[below, lower]);
    board.canonicalize(&mut child);
    assert_eq!(child.boxes[..2], [lower, below]);
}
