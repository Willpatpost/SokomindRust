//! What the stage tests compare against and run on:
//! - [`Geo`]: a port of the probe's geometry, with the probe's floods,
//!   keeper walk, push search, path, occupancy and fixture builders.
//! - the probe's three huge routes and their push states;
//! - random roots;
//! - [`Owned`]: every component a stage unit borrows, built for one board.
//!
//! The probe takes its dead cells from `pull`. [`Geo`] takes them from
//! `heuristic.dead`, which `dead_matches_pull` in heuristic.rs pins to the
//! probe's.
use super::{Facts, Scratch, Tools};
use crate::{
    corral::Corral,
    deadlock::Deadlock,
    heuristic::Heuristic,
    reach::Reach,
    testkit::{self, Lcg},
};
use sokomind_core::{Board, Cell, NONE, OPPOSITE, State, Step, WALL, decode_direction};
use std::collections::VecDeque;

/// The distance of a cell a [`Geo`] flood or push search did not reach.
pub(super) const FAR: u32 = u32::MAX;

/// The probe's three solution routes of huge, each with its move count:
/// 626, 893 and 503 moves, of 248, 278 and 236 pushes. Each file ends in a
/// newline, so the route helpers trim it.
pub(super) const ROUTES: [(usize, &str); 3] = [
    (626, include_str!("../../testdata/huge-626.txt")),
    (893, include_str!("../../testdata/huge-893.txt")),
    (503, include_str!("../../testdata/huge-503.txt")),
];

/// The probe's board geometry, rebuilt from the board and its
/// [`Heuristic`].
pub(super) struct Geo<'a> {
    /// The board.
    pub(super) board: &'a Board,
    /// Each cell's neighbors in `U D L R` order, `NONE` off the board.
    pub(super) nb: &'a [[Cell; 4]],
    /// The number of boxes.
    pub(super) slots: usize,
    /// Each slot's group: one group per run of equal labels.
    pub(super) group: Vec<usize>,
    /// Each cell's goal group, `usize::MAX` off goals.
    pub(super) goal_group: Vec<usize>,
    /// Each group's label.
    pub(super) groups: Vec<u8>,
    /// Each goal's cell and group, in board order.
    pub(super) goals: Vec<(Cell, usize)>,
    /// `dead[g][c]`: floor cell `c` is dead for group `g`.
    pub(super) dead: Vec<Vec<bool>>,
}

impl<'a> Geo<'a> {
    /// The geometry of `board`, with dead cells from `heuristic`.
    pub(super) fn new(board: &'a Board, heuristic: &Heuristic) -> Self {
        let (group, goal_group) = testkit::probe_groups(board);
        let slots = group.len();
        let groups: Vec<u8> = board
            .labels()
            .chunk_by(|a, b| a == b)
            .map(|run| run[0])
            .collect();
        let goals: Vec<(Cell, usize)> = board
            .goals()
            .iter()
            .map(|&(q, _)| (q, goal_group[usize::from(q)]))
            .collect();
        let tiles = board.tiles();
        let mut dead = vec![vec![false; tiles.len()]; groups.len()];
        for (i, &g) in group.iter().enumerate() {
            for (c, &tile) in tiles.iter().enumerate() {
                dead[g][c] = tile != WALL && heuristic.dead(i, c as Cell);
            }
        }
        Self {
            board,
            nb: board.neighbors(),
            slots,
            group,
            goal_group,
            groups,
            goals,
            dead,
        }
    }

    /// The probe's `flood`: keeper steps from `start` to each cell around
    /// the `blocked` cells, `FAR` where unreached, and everywhere when
    /// `start` is blocked.
    pub(super) fn flood(&self, start: Cell, blocked: &[bool]) -> Vec<u32> {
        let mut dist = vec![FAR; self.nb.len()];
        if blocked[usize::from(start)] {
            return dist;
        }
        dist[usize::from(start)] = 0;
        let mut queue = VecDeque::from([start]);
        while let Some(x) = queue.pop_front() {
            let here = dist[usize::from(x)];
            for y in self.nb[usize::from(x)] {
                if y == NONE || blocked[usize::from(y)] || dist[usize::from(y)] != FAR {
                    continue;
                }
                dist[usize::from(y)] = here + 1;
                queue.push_back(y);
            }
        }
        dist
    }

    /// The probe's `walk_path`: one shortest keeper walk from `from` to `to`
    /// on the empty board, both ends included, each step to the first
    /// neighbor in `U D L R` order that is one step closer. Empty when
    /// `from` cannot reach `to`.
    pub(super) fn walk_path(&self, from: Cell, to: Cell) -> Vec<Cell> {
        let open = vec![false; self.nb.len()];
        let dist = self.flood(to, &open);
        if dist[usize::from(from)] == FAR {
            return Vec::new();
        }
        let mut path = vec![from];
        let mut x = from;
        while x != to {
            let step = dist[usize::from(x)] - 1;
            x = self.nb[usize::from(x)]
                .into_iter()
                .filter(|&y| y != NONE)
                .find(|&y| dist[usize::from(y)] == step)
                .expect("a shortest walk steps closer");
            path.push(x);
        }
        path
    }

    /// The probe's `push` with nothing blocked: the one-box push search from
    /// `from`, with the keeper and the other boxes ignored and the `dead`
    /// cells skipped. A push needs the cell ahead and the stand behind on
    /// the board. Gives each cell's pushes, `FAR` where unreached, and the
    /// direction of the push that entered it, `u8::MAX` for `from` and
    /// where unreached.
    pub(super) fn push(&self, from: Cell, dead: &[bool]) -> (Vec<u32>, Vec<u8>) {
        let mut dist = vec![FAR; self.nb.len()];
        let mut via = vec![u8::MAX; self.nb.len()];
        dist[usize::from(from)] = 0;
        let mut queue = VecDeque::from([from]);
        while let Some(x) = queue.pop_front() {
            let around = self.nb[usize::from(x)];
            let here = dist[usize::from(x)];
            for (d, &y) in around.iter().enumerate() {
                let s = around[OPPOSITE[d]];
                if y == NONE || s == NONE || dead[usize::from(y)] || dist[usize::from(y)] != FAR {
                    continue;
                }
                dist[usize::from(y)] = here + 1;
                via[usize::from(y)] = d as u8;
                queue.push_back(y);
            }
        }
        (dist, via)
    }

    /// The probe's `path`: the cells the box enters and the stand of each
    /// push, in push order, along `via` from a [`Geo::push`] from `from`.
    /// `to` must be reached.
    pub(super) fn path(&self, from: Cell, to: Cell, via: &[u8]) -> (Vec<Cell>, Vec<Cell>) {
        let mut cells = Vec::new();
        let mut stands = Vec::new();
        let mut x = to;
        while x != from {
            let back = OPPOSITE[usize::from(via[usize::from(x)])];
            let prev = self.nb[usize::from(x)][back];
            cells.push(x);
            stands.push(self.nb[usize::from(prev)][back]);
            x = prev;
        }
        cells.reverse();
        stands.reverse();
        (cells, stands)
    }

    /// The probe's `occ`: each cell's box slot plus 1, or 0 for no box.
    pub(super) fn occ(&self, state: &State) -> Vec<usize> {
        let mut occ = vec![0; self.nb.len()];
        for (i, &c) in state.boxes[..self.slots].iter().enumerate() {
            occ[usize::from(c)] = i + 1;
        }
        occ
    }

    /// The probe's `settled`: whether each cell holds a box on a goal of its
    /// own label.
    pub(super) fn settled(&self, state: &State) -> Vec<bool> {
        let mut settled = vec![false; self.nb.len()];
        for (i, &c) in state.boxes[..self.slots].iter().enumerate() {
            if self.board.on_goal(i, c) {
                settled[usize::from(c)] = true;
            }
        }
        settled
    }

    /// The probe's `filled`: whether goal `q` holds a box of group `g`, with
    /// `occ` from [`Geo::occ`].
    pub(super) fn filled(&self, q: Cell, g: usize, occ: &[usize]) -> bool {
        let o = occ[usize::from(q)];
        o != 0 && self.group[o - 1] == g
    }

    /// Whether `c` is floor that holds no box of `state`.
    fn free(&self, state: &State, c: Cell) -> bool {
        self.board.tiles()[usize::from(c)] != WALL && !state.boxes[..self.slots].contains(&c)
    }

    /// The probe's `place`: `base` with each `(from, to)` box move applied
    /// in order and the keeper on `player`. `None` when a `from` holds no
    /// box, or when a `to` or `player` is a wall or holds a box.
    pub(super) fn place(
        &self,
        base: &State,
        moves: &[(Cell, Cell)],
        player: Cell,
    ) -> Option<State> {
        let mut state = *base;
        for &(from, to) in moves {
            let i = state.boxes[..self.slots].iter().position(|&c| c == from)?;
            if !self.free(&state, to) {
                return None;
            }
            state.boxes[i] = to;
        }
        state.player = player;
        self.free(&state, player).then_some(state)
    }

    /// The probe's `box_of`: the cell of the first box labelled `label`.
    pub(super) fn box_of(&self, state: &State, label: u8) -> Option<Cell> {
        let i = self.board.labels().iter().position(|&l| l == label)?;
        Some(state.boxes[i])
    }

    /// The probe's `parse_show`: the state that a probe picture, its rows
    /// from the board's top row and without their indent, draws. Each
    /// group's letter is a box of that group, filling the group's slots in
    /// row-major order, and '@' is the keeper. `None` when a group's box
    /// count or the keeper does not match.
    pub(super) fn parse_show(&self, rows: &[&str]) -> Option<State> {
        let mut state = self.board.initial();
        let mut next = vec![self.slots; self.groups.len()];
        for (i, &g) in self.group.iter().enumerate().rev() {
            next[g] = i;
        }
        let mut player = None;
        for (r, row) in rows.iter().enumerate() {
            for (c, ch) in row.bytes().enumerate() {
                if ch == b'@' {
                    player = Some(at(self.board, r, c));
                    continue;
                }
                let Some(g) = self.groups.iter().position(|&l| l == ch) else {
                    continue;
                };
                let i = next[g];
                if i >= self.slots || self.group[i] != g {
                    return None;
                }
                state.boxes[i] = at(self.board, r, c);
                next[g] += 1;
            }
        }
        for (g, &i) in next.iter().enumerate() {
            if i < self.slots && self.group[i] == g {
                return None;
            }
        }
        state.player = player?;
        Some(state)
    }
}

/// The cell in row `r` and column `c` of `board`.
pub(super) fn at(board: &Board, r: usize, c: usize) -> Cell {
    (r * board.width() + c) as Cell
}

/// The probe's `macro_ends`: along `route`, trimmed, the state after the
/// last push of each run of pushes of one box. `None` when an action is
/// unknown or illegal.
pub(super) fn macro_ends(board: &Board, route: &str) -> Option<Vec<State>> {
    let mut state = board.initial();
    let mut ends = Vec::new();
    let mut current: Option<usize> = None;
    let mut last = state;
    for action in route.trim().bytes() {
        if let Step::Push(slot) = board.step(&mut state, decode_direction(action)?)? {
            if current.is_some_and(|s| s != slot) {
                ends.push(last);
            }
            current = Some(slot);
            last = state;
        }
    }
    ends.push(last);
    Some(ends)
}

/// The state after each push of `route`, trimmed, in push order. Panics on
/// an unknown or illegal action.
pub(super) fn route_pushes(board: &Board, route: &str) -> Vec<State> {
    let mut state = board.initial();
    let mut pushes = Vec::new();
    for action in route.trim().bytes() {
        let direction = decode_direction(action).expect("a route action");
        if let Step::Push(_) = board.step(&mut state, direction).expect("a legal move") {
            pushes.push(state);
        }
    }
    pushes
}

/// Huge's extra roots, with `geo` built on huge: the roots of the checks
/// fixtures F2 and m11, then every `step`th push state of each probe route.
pub(super) fn huge_roots(geo: &Geo<'_>, step: usize) -> Vec<State> {
    let board = geo.board;
    let start = board.initial();
    let cell = |r, c| at(board, r, c);
    let f2 = geo.place(&start, &[(cell(3, 1), cell(2, 1))], cell(3, 1));
    let ends = macro_ends(board, ROUTES[1].1).expect("route 893");
    let mut roots = vec![f2.expect("F2's root"), ends[10]];
    for (_, route) in ROUTES {
        roots.extend(route_pushes(board, route).into_iter().step_by(step));
    }
    roots
}

/// A random root, drawn as goals.rs's tests draw states: the keeper on a
/// random floor cell of the start keeper's component, then each box, half
/// the time, on a free goal column of its group while one is left, else on
/// a random free floor cell.
pub(super) fn random_state(rng: &mut Lcg, board: &Board, heuristic: &Heuristic) -> State {
    let component = testkit::components(board, None);
    let home = component[usize::from(board.initial().player)];
    let tiles = board.tiles();
    let floor: Vec<Cell> = (0..tiles.len() as Cell)
        .filter(|&c| tiles[usize::from(c)] != WALL)
        .collect();
    let keeper: Vec<Cell> = floor
        .iter()
        .copied()
        .filter(|&c| component[usize::from(c)] == home)
        .collect();
    let mut state = board.initial();
    state.player = keeper[rng.below(keeper.len())];
    let mut used = vec![state.player];
    for i in 0..board.labels().len() {
        let mut free = heuristic.goal_cells()[heuristic.group(i)].to_vec();
        free.retain(|c| !used.contains(c));
        let cell = if !free.is_empty() && rng.below(2) == 0 {
            free[rng.below(free.len())]
        } else {
            loop {
                let c = floor[rng.below(floor.len())];
                if !used.contains(&c) {
                    break c;
                }
            }
        };
        state.boxes[i] = cell;
        used.push(cell);
    }
    state
}

/// Every component a stage unit borrows, owned and built for one board.
/// The board stays with the caller, which passes it to [`Owned::parts`].
pub(super) struct Owned {
    /// The board's heuristic.
    pub(super) heuristic: Heuristic,
    /// The keeper flood.
    pub(super) reach: Reach,
    /// Deadlock's occupancy.
    pub(super) deadlock: Deadlock,
    /// Corral's stamps and queue.
    pub(super) corral: Corral,
    /// The stage's working buffers.
    pub(super) scratch: Scratch,
    /// The start position's facts.
    pub(super) facts: Facts,
}

impl Owned {
    /// Every component for `board`, with its facts built.
    pub(super) fn new(board: &Board) -> Self {
        let cells = board.tiles().len();
        let heuristic = Heuristic::new(board);
        let mut scratch = Scratch::new(cells).expect("stage scratch");
        let player = board.initial().player;
        let facts = Facts::build(board, &heuristic, player, &mut scratch).expect("stage facts");
        Self {
            heuristic,
            reach: Reach::new(cells),
            deadlock: Deadlock::new(board),
            corral: Corral::new(board),
            scratch,
            facts,
        }
    }

    /// The units' three borrows of these components, for `board`, the board
    /// they were built for.
    pub(super) fn parts<'a>(
        &'a mut self,
        board: &'a Board,
    ) -> (Tools<'a>, &'a Facts, &'a mut Scratch) {
        let tools = Tools {
            board,
            heuristic: &self.heuristic,
            reach: &mut self.reach,
            deadlock: &mut self.deadlock,
            corral: &mut self.corral,
        };
        (tools, &self.facts, &mut self.scratch)
    }
}
