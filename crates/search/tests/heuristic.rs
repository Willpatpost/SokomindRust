use sokomind_core::{Board, State};
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
