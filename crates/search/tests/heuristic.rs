use sokomind_core::{Board, NONE, OPPOSITE, State};
use sokomind_search::Heuristic;
use std::collections::HashSet;

/// Repairs must reproduce the full assignment exactly on every push edge
/// reachable from the start, feasible or not: the single-row augment either
/// restores the true minimum or proves no matching exists.
#[test]
fn incremental_repair_matches_full_assignment() {
    for rows in [
        "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO",
        "OOOOOOOO\nO R    O\nO XXXO O\nO SSSO O\nO      O\nOOOOOOOO",
        "OOOOOOO\nOR    O\nO AB  O\nO     O\nOa b  O\nOOOOOOO",
        "OOOOOOO\nOS   SO\nO  X  O\nO XRXOO\nO  X  O\nOS   SO\nOOOOOOO",
    ] {
        let board = Board::parse(rows).unwrap();
        let heuristic = Heuristic::new(&board);
        let mut start = board.initial;
        board.canonicalize(&mut start);
        let key = |state: &State| (state.player, state.boxes);
        let mut seen = HashSet::from([key(&start)]);
        let mut queue = vec![start];
        let mut edges = 0;
        while let Some(state) = queue.pop() {
            for direction in 0..4 {
                let mut next = state;
                if board.step(&mut next, direction).is_none() {
                    continue;
                }
                board.canonicalize(&mut next);
                if seen.insert(key(&next)) {
                    queue.push(next);
                }
                // Only push transitions exercise the repair path.
                if next.boxes == state.boxes {
                    continue;
                }
                edges += 1;
                let Some(parent) = heuristic.assignment(&state) else {
                    continue;
                };
                let incremental = heuristic.estimate_from(&parent, &next);
                let full = heuristic.estimate(&next);
                assert_eq!(incremental, full, "repair mismatch on {rows}");
            }
        }
        assert!(edges > 0, "no push edges explored on {rows}");
    }
}

/// Nine interchangeable X boxes, past the size where `estimate_from` repairs
/// a group from the parent's duals instead of re-solving it.
const WIDE: &str = concat!(
    "OOOOOOOOOOOO\n",
    "O          O\n",
    "O SSS  SSS O\n",
    "O  XXXXX   O\n",
    "O   R      O\n",
    "O  XXXX  A O\n",
    "O SSS      O\n",
    "O        a O\n",
    "OOOOOOOOOOOO",
);

/// Every legal single push from `state`, in the parent's box order.
fn push_children(board: &Board, state: &State) -> Vec<State> {
    let boxes = &state.boxes[..board.labels.len()];
    let mut reached = vec![false; board.tiles.len()];
    reached[state.player as usize] = true;
    let mut stack = vec![state.player];
    while let Some(cell) = stack.pop() {
        for next in board.neighbors[cell as usize] {
            if next != NONE && !reached[next as usize] && !boxes.contains(&next) {
                reached[next as usize] = true;
                stack.push(next);
            }
        }
    }
    let mut children = Vec::new();
    for &cell in boxes {
        for (direction, &opposite) in OPPOSITE.iter().enumerate() {
            let stand = board.neighbors[cell as usize][opposite];
            if stand == NONE || !reached[stand as usize] {
                continue;
            }
            let mut child = State {
                player: stand,
                ..*state
            };
            if board.step(&mut child, direction).is_some() {
                children.push(child);
            }
        }
    }
    children
}

/// A seeded random walk over feasible states, restarting every 25 pushes,
/// checks the dual repair against a full solve on every child, including
/// pushes that leave a box with no reachable goal.
#[test]
fn dual_repair_matches_full_assignment() {
    let board = Board::parse(WIDE).unwrap();
    let heuristic = Heuristic::new(&board);
    assert!(heuristic.repairs_worthwhile());
    let mut seed = 1u64;
    let mut state = board.initial;
    let (mut feasible, mut infeasible) = (0, 0);
    for step in 0..200 {
        if step % 25 == 0 {
            state = board.initial;
        }
        let parent = heuristic.assignment(&state).unwrap();
        let mut options = Vec::new();
        for mut child in push_children(&board, &state) {
            board.canonicalize(&mut child);
            let full = heuristic.estimate(&child);
            assert_eq!(heuristic.estimate_from(&parent, &child), full, "{child:?}");
            if full.is_some() {
                feasible += 1;
                options.push(child);
            } else {
                infeasible += 1;
            }
        }
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state = if options.is_empty() {
            board.initial
        } else {
            options[(seed >> 33) as usize % options.len()]
        };
    }
    assert!(feasible > 0 && infeasible > 0, "{feasible} {infeasible}");
}
