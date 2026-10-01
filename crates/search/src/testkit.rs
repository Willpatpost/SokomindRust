//! Test-only helpers shared by the unit tests: the catalog, a seeded random
//! source and the rooms it generates, and an exhaustive oracle that gives
//! the exact remaining moves from every primitive state of a small board.
use sokomind_core::{Board, State, Step};
use std::{
    collections::{HashMap, VecDeque},
    sync::LazyLock,
};

/// Primitive states per board at most; see `MIN_EXPLORED`.
const CAP: usize = 20_000;
/// How many catalog boards fit under `CAP` today. `explored_catalog`
/// checks it once, so a catalog edit that drops one fails there, in one
/// place.
const MIN_EXPLORED: usize = 8;
/// Successor state and, for a push, the box index and direction.
pub(crate) type Edge = (usize, Option<(usize, usize)>);
/// A catalog board that fits under `CAP`: its id, the board, every
/// primitive state reachable from its start without expanding solved ones
/// (the start first) and each state's edges.
pub(crate) type Explored = (String, Board, Vec<State>, Vec<Vec<Edge>>);

/// The fitting catalog boards, explored on first use, which panics when
/// fewer than `MIN_EXPLORED` fit.
static EXPLORED: LazyLock<Vec<Explored>> = LazyLock::new(|| {
    let explored: Vec<Explored> = catalog()
        .into_iter()
        .filter_map(|(id, board)| {
            let (states, edges) = explore(&board)?;
            Some((id, board, states, edges))
        })
        .collect();
    let ids: Vec<&str> = explored.iter().map(|(id, ..)| id.as_str()).collect();
    assert!(explored.len() >= MIN_EXPLORED, "{ids:?}");
    explored
});

/// Every catalog board with its id.
pub(crate) fn catalog() -> Vec<(String, Board)> {
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

/// The catalog boards that fit under `CAP`, explored once per test binary.
pub(crate) fn explored_catalog() -> &'static [Explored] {
    &EXPLORED
}

/// 64-bit LCG with Knuth's MMIX multiplier and increment. Tests seed it with
/// a literal, so every run sees the same generated rooms and walks. A test's
/// seed and order of draws pin its rooms; changing either silently swaps the
/// boards it checks.
pub(crate) struct Lcg(pub(crate) u64);
impl Lcg {
    /// A draw in `0..n`, from the top 31 bits of the next state.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as usize % n
    }
}

/// A walled room with a 4x4 floor holding one box set with its goals,
/// the robot and up to two inner walls, all on distinct cells. Its draws
/// pin the rooms of `flagged_pushes_leave_no_solution` in the deadlock
/// tests, from `Lcg(0x5eed)`, and of `pruning_never_changes_a_live_run`
/// in the engine tests, from `Lcg(0xd1ff)`; changing them swaps those
/// boards and invalidates the Python counts both cite.
pub(crate) fn random_room(rng: &mut Lcg) -> String {
    const SETS: [&[u8]; 4] = [b"AaBb", b"AaBbCc", b"XSXSAa", b"XXXSSS"];
    let set = SETS[rng.below(SETS.len())];
    let walls = &b"OO"[..rng.below(3)];
    let mut floor = [b' '; 16];
    let mut free: Vec<usize> = (0..floor.len()).collect();
    for &symbol in set.iter().chain(b"R").chain(walls) {
        floor[free.swap_remove(rng.below(free.len()))] = symbol;
    }
    let mut rows = vec!["OOOOOO".to_string()];
    for row in floor.chunks(4) {
        rows.push(format!("O{}O", std::str::from_utf8(row).unwrap()));
    }
    rows.push("OOOOOO".to_string());
    rows.join("\n")
}

/// Every primitive state reachable from the start, without expanding solved
/// ones, or `None` past `CAP` states. Box order is the board's: estimates
/// and walks ignore order inside a group.
fn explore(board: &Board) -> Option<(Vec<State>, Vec<Vec<Edge>>)> {
    let key = |state: &State| (state.player, state.boxes);
    let mut states = vec![board.initial()];
    let mut index = HashMap::from([(key(&board.initial()), 0)]);
    let mut edges = Vec::new();
    while edges.len() < states.len() {
        let state = states[edges.len()];
        let mut out = Vec::new();
        if !board.solved(&state) {
            for direction in 0..4 {
                let mut next = state;
                let Some(step) = board.step(&mut next, direction) else {
                    continue;
                };
                let pushed = match step {
                    Step::Walk => None,
                    Step::Push(i) => Some((i, direction)),
                };
                let id = *index.entry(key(&next)).or_insert_with(|| {
                    states.push(next);
                    states.len() - 1
                });
                out.push((id, pushed));
            }
        }
        edges.push(out);
        if states.len() > CAP {
            return None;
        }
    }
    Some((states, edges))
}

/// Exact moves to a solved state, `u32::MAX` without one.
pub(crate) fn remaining(board: &Board, states: &[State], edges: &[Vec<Edge>]) -> Vec<u32> {
    let mut reverse = vec![Vec::new(); states.len()];
    for (from, out) in edges.iter().enumerate() {
        for &(to, _) in out {
            reverse[to].push(from);
        }
    }
    let mut exact = vec![u32::MAX; states.len()];
    let mut queue = VecDeque::new();
    for (id, state) in states.iter().enumerate() {
        if board.solved(state) {
            exact[id] = 0;
            queue.push_back(id);
        }
    }
    while let Some(to) = queue.pop_front() {
        for &from in &reverse[to] {
            if exact[from] == u32::MAX {
                exact[from] = exact[to] + 1;
                queue.push_back(from);
            }
        }
    }
    exact
}
