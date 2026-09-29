//! Side-aware reverse push distances, the 5.2 O2 experiment (feature `o2`).
//!
//! A box on `cell` splits the rest of the floor into up to four parts, the
//! cell's sides, and the keeper can only stand in the one it is in. Pushing
//! the box from `previous` onto `cell` needs the keeper on the side of
//! `previous` facing away from `cell`, and leaves it on `previous`, which is
//! on the side of `cell` facing `previous`. A BFS over (cell, side) states
//! therefore counts exactly the pushes a lone box needs from each keeper
//! side, where the plain table lets the keeper stand anywhere.
use sokomind_core::{Board, Cell, MAX_CELLS, NONE, WALL};

/// No neighbour in this direction: a wall or the board edge.
const NO_SIDE: u8 = u8::MAX;
// A (cell, side) state index and a push count both stay below 4 * MAX_CELLS,
// so they fit u16 with NONE left free as the unseen mark.
const _: () = assert!(4 * MAX_CELLS <= NONE as usize);

/// `sides[cell * 4 + direction]` is the side of `cell` its neighbour in
/// `direction` lies on, numbered `0..4` in direction order of first
/// appearance, or `NO_SIDE` without a neighbour. Two neighbours share a side
/// exactly when they stay connected with `cell` removed.
///
/// One iterative Tarjan pass: a neighbour inside the subtree of a DFS child
/// `c` of `cell` is on `c`'s own side when nothing in that subtree reaches
/// above `cell` (`low[c] >= order[cell]`), and on the parent side otherwise.
/// Every other neighbour is an ancestor, so on the parent side too.
fn sides(board: &Board) -> Vec<u8> {
    let neighbors = board.neighbors();
    let cells = neighbors.len();
    // Discovery time from 1 (0 = unseen), the last discovery time inside the
    // cell's subtree, the lowest discovery time one back edge from the
    // subtree reaches, and the DFS parent.
    let mut order = vec![0u32; cells];
    let mut end = vec![0u32; cells];
    let mut low = vec![0u32; cells];
    let mut parent = vec![NONE; cells];
    // (cell, next direction to try)
    let mut stack: Vec<(Cell, u8)> = Vec::new();
    let mut time = 0;
    for root in 0..cells {
        if board.tiles()[root] == WALL || order[root] != 0 {
            continue;
        }
        time += 1;
        order[root] = time;
        low[root] = time;
        stack.push((root as Cell, 0));
        while let Some(top) = stack.last_mut() {
            let cell = top.0 as usize;
            if top.1 < 4 {
                let next = neighbors[cell][top.1 as usize];
                top.1 += 1;
                if next == NONE {
                    continue;
                }
                if order[next as usize] == 0 {
                    time += 1;
                    order[next as usize] = time;
                    low[next as usize] = time;
                    parent[next as usize] = cell as Cell;
                    stack.push((next, 0));
                } else if next != parent[cell] {
                    low[cell] = low[cell].min(order[next as usize]);
                }
            } else {
                stack.pop();
                end[cell] = time;
                let up = parent[cell];
                if up != NONE {
                    low[up as usize] = low[up as usize].min(low[cell]);
                }
            }
        }
    }
    let mut sides = vec![NO_SIDE; cells * 4];
    for (cell, around) in neighbors.iter().enumerate() {
        // The child subtree each side stands for, NONE for the parent side.
        let mut tags = [NONE; 4];
        let mut count = 0;
        for (direction, &next) in around.iter().enumerate() {
            if next == NONE {
                continue;
            }
            let at = order[next as usize];
            let tag = around
                .iter()
                .copied()
                .find(|&child| {
                    child != NONE
                        && parent[child as usize] == cell as Cell
                        && (order[child as usize]..=end[child as usize]).contains(&at)
                })
                .filter(|&child| low[child as usize] >= order[cell])
                .unwrap_or(NONE);
            let side = match tags[..count].iter().position(|&seen| seen == tag) {
                Some(side) => side,
                None => {
                    tags[count] = tag;
                    count += 1;
                    count - 1
                }
            };
            sides[cell * 4 + direction] = side as u8;
        }
    }
    sides
}

/// Cell-major reverse push distances to each goal column, laid out like
/// `Heuristic::distances`: per column, the fewest pushes a lone box needs
/// from each cell, minimized over the keeper's side. At least the plain
/// distance, exact for one box on an otherwise empty board, and so
/// admissible, but not consistent: a push can land the keeper on a far side
/// of a cell whose table entry comes from a near one.
pub(crate) fn push_distances(board: &Board, columns: &[(Cell, u8)]) -> Vec<u16> {
    let neighbors = board.neighbors();
    let sides = sides(board);
    let goals = columns.len();
    let mut distances = vec![NONE; neighbors.len() * goals];
    // Pushes left from state `cell * 4 + side`, NONE while unseen.
    let mut level = vec![NONE; neighbors.len() * 4];
    let mut queue: Vec<u16> = Vec::with_capacity(neighbors.len() * 4);
    for (column, &(goal, _)) in columns.iter().enumerate() {
        let at = |cell: usize| cell * goals + column;
        let goal = goal as usize;
        level.fill(NONE);
        queue.clear();
        distances[at(goal)] = 0;
        for &side in &sides[goal * 4..goal * 4 + 4] {
            if side != NO_SIDE && level[goal * 4 + side as usize] == NONE {
                level[goal * 4 + side as usize] = 0;
                queue.push((goal * 4 + side as usize) as u16);
            }
        }
        let mut head = 0;
        while head < queue.len() {
            let state = queue[head] as usize;
            head += 1;
            let (cell, side) = (state / 4, (state % 4) as u8);
            let pushes = level[state] + 1;
            // The push that left the keeper on `previous`, facing `side`.
            for direction in 0..4 {
                if sides[cell * 4 + direction] != side {
                    continue;
                }
                let previous = neighbors[cell][direction] as usize;
                if neighbors[previous][direction] == NONE {
                    continue;
                }
                let before = previous * 4 + sides[previous * 4 + direction] as usize;
                if level[before] == NONE {
                    level[before] = pushes;
                    queue.push(before as u16);
                    // States leave the queue by level, so a cell's first
                    // reached side is its nearest.
                    if distances[at(previous)] == NONE {
                        distances[at(previous)] = pushes;
                    }
                }
            }
        }
    }
    distances
}

#[cfg(test)]
mod tests {
    use super::{NO_SIDE, push_distances, sides};
    use crate::heuristic::{Heuristic, ParentGroup, plain_distances};
    use crate::{Mode, Proof, Search, Status};
    use sokomind_core::{Board, Cell, NONE, WALL};
    use std::collections::VecDeque;

    /// Root estimates of every catalog board as (id, plain, o2), from the
    /// Python mirror of this module (specs/o2/o2_table.py at d866657).
    const ROOTS: [(&str, u32, u32); 57] = [
        ("ultra-tiny", 1, 1),
        ("tiny", 5, 5),
        ("tutorial-push", 1, 1),
        ("tutorial-around", 1, 1),
        ("beginner-three", 3, 3),
        ("beginner-detour", 10, 10),
        ("beginner-typed-line", 15, 15),
        ("box-5x5-a", 3, 3),
        ("medium", 18, 18),
        ("garden-2", 14, 14),
        ("workshop-1", 10, 10),
        ("classic-1", 11, 11),
        ("theme-kitchen", 12, 12),
        ("large", 42, 44),
        ("adv-gallery", 10, 10),
        ("theme-parking", 34, 34),
        ("open-field", 66, 66),
        ("huge", 200, 208),
        ("expert-maze", 24, 24),
        ("gen-v2-310116-594a231c", 12, 12),
        ("gen-v2-310014-7ebd2f54", 12, 12),
        ("gen-v2-310089-a702de93", 13, 13),
        ("gen-v2-310032-ceaf7565", 8, 8),
        ("gen-v2-310056-2f55f9f8", 17, 17),
        ("gen-v2-310118-e9172c97", 17, 17),
        ("gen-v2-310145-228a1c30", 16, 16),
        ("gen-v2-310136-dd37455b", 18, 18),
        ("gen-v2-310081-a2088508", 27, 27),
        ("gen-v2-310183-c9e1ef62", 30, 30),
        ("gen-v2-320093-09d278d1", 30, 30),
        ("gen-v2-320165-dcd465b6", 50, 50),
        ("gen-v2-320118-f55e1396", 40, 42),
        ("gen-v2-320050-78319f43", 49, 49),
        ("gen-v2-320050-ece6c64b", 46, 46),
        ("gen-v2-320115-e2982270", 49, 49),
        ("gen-v2-320041-027a9b04", 46, 46),
        ("gen-v2-320050-4eb88a2d", 46, 46),
        ("gen-v2-320081-b59cab95", 46, 46),
        ("gen-v2-320041-e16f5a47", 44, 44),
        ("gen-v2-340114-a8b9d02a", 68, 68),
        ("gen-v2-340053-6d913910", 42, 42),
        ("gen-v2-340187-cd08a50a", 57, 67),
        ("gen-v2-340053-466f0008", 49, 49),
        ("gen-v2-330146-44c41505", 54, 62),
        ("gen-v2-340106-b3558338", 78, 78),
        ("gen-v2-340048-b5d312a6", 58, 58),
        ("gen-v2-340046-c9f5ef3b", 70, 70),
        ("gen-v2-350108-80cb0432", 93, 93),
        ("gen-v2-340272-fd819d0e", 103, 105),
        ("gen-v2-350250-5b361f8b", 102, 102),
        ("gen-v2-350070-6be05c02", 82, 92),
        ("gen-v2-350108-b13869bc", 108, 110),
        ("gen-v2-360313-16158b3b", 128, 128),
        ("gen-v2-370002-8ea0b852", 148, 148),
        ("gen-v2-370193-88873f15", 117, 117),
        ("gen-v2-350001-a996dcbc", 138, 140),
        ("gen-v2-360372-b5687375", 112, 118),
    ];

    /// The smallest one-box board the pre-validation found where the plain
    /// table is finite at the box and the side-aware one is not: the only
    /// first push lands the box on a cell whose last push needs the keeper
    /// in the dead end behind it.
    const SIDE_TRAP: &str = "OOOOO\nO R O\nO AOO\nOa  O\nOOOOO";

    /// Every catalog board with its id.
    fn catalog() -> Vec<(String, Board)> {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../../data/puzzles.json")).unwrap();
        catalog
            .as_array()
            .unwrap()
            .iter()
            .map(|puzzle| {
                let rows: Vec<&str> = puzzle["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row.as_str().unwrap())
                    .collect();
                let board = Board::parse(&rows.join("\n")).unwrap();
                (puzzle["id"].as_str().unwrap().to_owned(), board)
            })
            .collect()
    }

    /// Goal columns in group order, as `Heuristic::new` lays them out.
    fn columns(board: &Board) -> Vec<(Cell, u8)> {
        let mut columns = board.goals().to_vec();
        columns.sort_by_key(|&(_, label)| label);
        columns
    }

    /// Fixed-seed LCG, so every run sees the same boards.
    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as usize % n
        }
    }

    /// A walled 3..=6 x 3..=6 room, about 28% walls, with one A box, its goal
    /// and the robot on distinct cells. Many boxes are stuck; some sit where
    /// only the plain table sees a way out.
    fn one_box_board(rng: &mut Lcg) -> Board {
        let (width, height) = (3 + rng.below(4), 3 + rng.below(4));
        let mut cells = vec![b' '; width * height];
        let mut free: Vec<usize> = (0..cells.len()).collect();
        for symbol in [b'A', b'a', b'R'] {
            let cell = free.swap_remove(rng.below(free.len()));
            cells[cell] = symbol;
        }
        for cell in free {
            if rng.below(100) < 28 {
                cells[cell] = b'O';
            }
        }
        let wall = "O".repeat(width + 2);
        let mut rows = vec![wall.clone()];
        for row in cells.chunks(width) {
            rows.push(format!("O{}O", std::str::from_utf8(row).unwrap()));
        }
        rows.push(wall);
        Board::parse(&rows.join("\n")).unwrap()
    }

    /// Independent sides: flood `floor - {cell}` from each neighbour and
    /// number the parts in direction order of first appearance.
    fn flood_sides(board: &Board) -> Vec<u8> {
        let neighbors = board.neighbors();
        let mut sides = vec![NO_SIDE; neighbors.len() * 4];
        for (cell, around) in neighbors.iter().enumerate() {
            for (direction, &start) in around.iter().enumerate() {
                if start == NONE || sides[cell * 4 + direction] != NO_SIDE {
                    continue;
                }
                // One past the largest id so far: ids count up from 0.
                let side = sides[cell * 4..cell * 4 + 4]
                    .iter()
                    .filter(|&&id| id != NO_SIDE)
                    .max()
                    .map_or(0, |&last| last + 1);
                let mut seen = vec![false; neighbors.len()];
                seen[start as usize] = true;
                let mut queue = VecDeque::from([start]);
                while let Some(at) = queue.pop_front() {
                    for next in neighbors[at as usize] {
                        if next != NONE && next as usize != cell && !seen[next as usize] {
                            seen[next as usize] = true;
                            queue.push_back(next);
                        }
                    }
                }
                for (other, &next) in around.iter().enumerate() {
                    if next != NONE && seen[next as usize] {
                        sides[cell * 4 + other] = side;
                    }
                }
            }
        }
        sides
    }

    /// The exact fewest pushes a lone box on each cell needs to reach `goal`,
    /// minimized over every keeper cell: a reverse 0-1 BFS over (box, keeper)
    /// from every (goal, keeper). Reversed walks cost 0; un-pushing box `box`
    /// from keeper `keeper` puts the box on `keeper` and the keeper one step
    /// further back, for 1. `u32::MAX` when no keeper cell gets it there.
    fn true_pushes(board: &Board, goal: Cell) -> Vec<u32> {
        let neighbors = board.neighbors();
        let cells = neighbors.len();
        let floor = |cell: usize| board.tiles()[cell] != WALL;
        let mut pushes = vec![u32::MAX; cells * cells];
        let mut queue = VecDeque::new();
        for keeper in (0..cells).filter(|&k| floor(k) && k != goal as usize) {
            pushes[goal as usize * cells + keeper] = 0;
            queue.push_back((goal as usize, keeper));
        }
        while let Some((at, keeper)) = queue.pop_front() {
            let cost = pushes[at * cells + keeper];
            for direction in 0..4 {
                let next = neighbors[keeper][direction];
                if next != NONE && next as usize != at && cost < pushes[at * cells + next as usize]
                {
                    pushes[at * cells + next as usize] = cost;
                    queue.push_front((at, next as usize));
                }
                // The keeper pushed the box from `keeper` toward `direction`.
                let back = neighbors[keeper][direction ^ 1];
                if next as usize == at
                    && back != NONE
                    && cost + 1 < pushes[keeper * cells + back as usize]
                {
                    pushes[keeper * cells + back as usize] = cost + 1;
                    queue.push_back((keeper, back as usize));
                }
            }
        }
        (0..cells)
            .map(|at| {
                if at == goal as usize {
                    return 0;
                }
                (0..cells)
                    .filter(|&k| k != at)
                    .map(|k| pushes[at * cells + k])
                    .min()
                    .unwrap_or(u32::MAX)
            })
            .collect()
    }

    #[test]
    fn tarjan_sides_match_a_flood_fill() {
        let mut rng = Lcg(5);
        let boards = catalog()
            .into_iter()
            .map(|(_, board)| board)
            .chain((0..400).map(|_| one_box_board(&mut rng)));
        for board in boards {
            assert_eq!(
                sides(&board),
                flood_sides(&board),
                "{}",
                board.fingerprint()
            );
        }
    }

    /// Plain <= side-aware <= the BFS push count, on small one-box boards;
    /// the second is an equality because the (cell, side) BFS is exact for a
    /// lone box. Some box cells must be strictly stronger, some newly dead.
    #[test]
    fn side_distances_lie_between_plain_and_true_pushes() {
        let mut rng = Lcg(7);
        let (mut stronger, mut dead) = (0, 0);
        for _ in 0..400 {
            let board = one_box_board(&mut rng);
            let columns = columns(&board);
            let plain = plain_distances(&board, &columns);
            let side = push_distances(&board, &columns);
            let exact = true_pushes(&board, columns[0].0);
            for cell in (0..board.tiles().len()).filter(|&c| board.tiles()[c] != WALL) {
                let widen = |d: u16| if d == NONE { u32::MAX } else { u32::from(d) };
                let (plain, side) = (widen(plain[cell]), widen(side[cell]));
                let context = format!("{} cell {cell}", board.fingerprint());
                assert!(plain <= side, "{context}: {plain} > {side}");
                assert_eq!(side, exact[cell], "{context}");
                stronger += usize::from(plain < side);
                dead += usize::from(plain < side && side == u32::MAX);
            }
        }
        assert!(stronger > 0 && dead > 0, "{stronger} stronger, {dead} dead");
    }

    /// Root h never falls below the plain table's, and equals the Python
    /// mirror's on every catalog board (10 rise, by 2 to 10).
    #[test]
    fn o2_root_estimates_match_prevalidation() {
        let catalog = catalog();
        assert_eq!(catalog.len(), ROOTS.len());
        for ((id, board), (expected, plain, o2)) in catalog.iter().zip(ROOTS) {
            assert_eq!(id, expected);
            let start = board.initial();
            let old = Heuristic::with_table(board, plain_distances).estimate(&start);
            let new = Heuristic::new(board).estimate(&start);
            assert_eq!((old, new), (Some(plain), Some(o2)), "{id}");
            assert!(o2 >= plain, "{id}");
        }
    }

    /// The engine's incremental child estimate, including the dual repair of
    /// groups of 3 or more, equals a fresh estimate on every catalog board
    /// whose table O2 changes: the repair needs nonnegative costs, not a
    /// consistent table. Boxes step onto any free neighbour, as in the
    /// heuristic module's walk; estimates ignore the order inside a group.
    #[test]
    fn child_estimates_match_fresh_on_side_aware_tables() {
        let mut rng = Lcg(11);
        let mut checked = 0;
        for (id, board) in catalog() {
            let columns = columns(&board);
            if push_distances(&board, &columns) == plain_distances(&board, &columns) {
                continue;
            }
            let heuristic = Heuristic::new(&board);
            let boxes = board.labels().len();
            let mut state = board.initial();
            for step in 0..60_usize {
                if step.is_multiple_of(20) {
                    state = board.initial();
                }
                let parent_h = heuristic.estimate(&state).unwrap();
                let mut cache = ParentGroup::EMPTY;
                let mut options = Vec::new();
                for i in 0..boxes {
                    for to in board.neighbors()[state.boxes[i] as usize] {
                        if to == NONE || state.boxes[..boxes].contains(&to) {
                            continue;
                        }
                        let mut child = state;
                        child.boxes[i] = to;
                        let fresh = heuristic.estimate(&child);
                        let incremental =
                            heuristic.child_estimate(parent_h, &mut cache, &state, i, to);
                        assert_eq!(incremental, fresh, "{id} step {step} box {i} to {to}");
                        checked += 1;
                        if fresh.is_some() {
                            options.push(child);
                        }
                    }
                }
                if !options.is_empty() {
                    state = options[rng.below(options.len())];
                }
            }
        }
        assert!(checked > 0);
    }

    /// The plain table needs a full search to prove SIDE_TRAP unsolvable; the
    /// side-aware one has no finite root estimate, so every mode stops at
    /// the root and Optimal claims Unsolvable without expanding it.
    #[test]
    fn side_trap_is_unsolvable_at_the_root() {
        let board = Board::parse(SIDE_TRAP).unwrap();
        let start = board.initial();
        let plain = Heuristic::with_table(&board, plain_distances);
        assert_eq!(plain.estimate(&start), Some(2));
        assert_eq!(Heuristic::new(&board).estimate(&start), None);
        for mode in [Mode::Optimal, Mode::Fast, Mode::Quality] {
            let mut search = Search::new(board.clone(), start, mode, 1_000, 8).unwrap();
            search.advance(32);
            assert_eq!(search.status(), Status::Exhausted, "{mode:?}");
            assert_eq!((search.expanded(), search.generated()), (0, 1), "{mode:?}");
            let proof = (mode == Mode::Optimal).then_some(Proof::Unsolvable);
            assert_eq!(search.proof(), proof, "{mode:?}");
            assert_eq!(search.solution().unwrap(), None, "{mode:?}");
        }
    }
}
