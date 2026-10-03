//! Two goal-placement prunes for the stage ladder's candidate states. Each
//! `false` comes with a proof that the state has no solution, so both are
//! sound in every mode, but nothing outside staging calls them.
//!
//! `GoalReach` relaxes a state to one box at a time, every other box
//! removed. A box on `x` with the keeper on side `k` of `x`, a `Blocks`
//! side (the part of the keeper's floor component that blocking `x` leaves
//! the keeper in), can be pushed from any neighbor on side `k` onto the
//! floor beyond, and the push leaves the keeper on `x`, so on the side of
//! the new cell that holds `x`. While the box stays put the keeper never
//! leaves its side, and other boxes only take cells away, so every real
//! push of the box is a relaxed one and the goals it reaches alone hold
//! every goal it can end on. `matched` asks for a perfect matching of the
//! boxes onto distinct goals of their own labels inside those sets; with
//! none, no solution exists. A box outside the keeper's floor component
//! never moves, so it reaches only the goal under it, if any.
//!
//! The build runs one breadth-first search per goal backwards over the
//! relaxed pushes, from every side of the goal, so it visits exactly the
//! states that reach the goal: `O(goals * states)` steps, with the goal's
//! mask bit as the visited flag. States are numbered densely, `base[x]`
//! plus the rank of the side's label among the labels of `x`. The sides of
//! `x` are the blocks (biconnected components) that hold it, so a floor
//! component of `n` cells has as many states as its blocks have cells in
//! all, `n - 1 + blocks`, at most `2n - 2` and so below `2 * cells`.
//!
//! A sink line is a wall-to-wall run of at least two floor cells along a
//! row or column with a wall beside every cell across the run. A box on
//! such a cell cannot be pushed across, toward the wall or away from it,
//! so the boxes on a line stay on it for good and in order, and each must
//! end on a goal of its own label on the line, distinct and in the same
//! order. `sink_ok` tests that embedding greedily: walking the line, each
//! box takes the first goal of its label past the previous box's, which
//! finds an embedding whenever one exists.
//!
//! Layout: GoalReach keeps `base` (a `u16` per cell), `masks` (a `u32` per
//! state, reserved at the `2 * cells` bound) and `goal_of` (a `u8` per
//! cell), 11 bytes per cell and 45,056 at `MAX_CELLS`. SinkLines keeps one
//! `u16` head per line, at most one per cell because the lines along an
//! axis are disjoint runs of at least two cells, and the `on_line` set at
//! a bit per cell, 8,704 bytes at `MAX_CELLS`. The build queue, a `u16`
//! per state, is lent by the caller.
use super::Deadlock;
use crate::heuristic::Heuristic;
use crate::reach::Blocks;
use sokomind_core::{Board, Cell, MAX_BOXES, MAX_CELLS, NONE, State, WALL};
use std::{collections::TryReserveError, iter, mem::size_of, ops::Range};

/// `goal_of` on a cell without a goal.
const NO_GOAL: u8 = u8::MAX;
/// A Kuhn owner slot whose goal no box holds yet.
const FREE: u8 = u8::MAX;
/// The bit of a line head holding its axis: 0 for a row, 1 for a column.
const AXIS_SHIFT: u32 = 15;
/// Per axis, rows then columns: the directions back along a line, forward
/// along it, and the two across it.
const AXES: [[usize; 4]; 2] = [[2, 3, 0, 1], [0, 1, 2, 3]];

// A packed build state, a cell shifted past its 2-bit side label, fits `u16`.
const _: () = assert!(MAX_CELLS <= 1 << 14);
// State indices, below `2 * cells`, fit `u16`.
const _: () = assert!(2 * MAX_CELLS <= u16::MAX as usize);
// A line head's cell fits below its axis bit.
const _: () = assert!(MAX_CELLS <= 1 << AXIS_SHIFT);
// A mask has a bit per goal column, and goal and box indices stay below the
// `u8` sentinels.
const _: () = assert!(MAX_BOXES <= u32::BITS as usize);
const _: () = assert!(MAX_BOXES < u8::MAX as usize);

/// The goals one box reaches alone from each relaxed state; see the module
/// docs.
pub(crate) struct GoalReach {
    /// Each cell's first state index. A cell of the keeper's component has
    /// one state per side from there on, in label order.
    base: Vec<u16>,
    /// Per state, bit `t` set when the box reaches goal column `t`.
    masks: Vec<u32>,
    /// Each cell's goal column, `NO_GOAL` off goals: all that a box outside
    /// the keeper's component reaches.
    goal_of: Vec<u8>,
}

impl GoalReach {
    /// Heap bytes `build` reserves for a board of `cells` cells.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) const fn bytes_for(cells: usize) -> usize {
        cells * size_of::<u16>() + 2 * cells * size_of::<u32>() + cells * size_of::<u8>()
    }
    /// Numbers the states of the keeper's component from `blocks`, then
    /// searches back from each goal in it. Goal columns are `heuristic`'s.
    /// The three vectors are the only allocations, reserved once at their
    /// bounds; `queue` is scratch of at least `2 * cells` entries, left
    /// holding garbage.
    pub(crate) fn build(
        board: &Board,
        heuristic: &Heuristic,
        blocks: &Blocks,
        queue: &mut [u16],
    ) -> Result<Self, TryReserveError> {
        let (tiles, neighbors) = (board.tiles(), board.neighbors());
        let cells = tiles.len();
        debug_assert!(queue.len() >= 2 * cells);
        let mut base: Vec<u16> = Vec::new();
        base.try_reserve_exact(cells)?;
        let mut masks: Vec<u32> = Vec::new();
        masks.try_reserve_exact(2 * cells)?;
        let mut goal_of: Vec<u8> = Vec::new();
        goal_of.try_reserve_exact(cells)?;
        let mut states: u16 = 0;
        for (x, &tile) in tiles.iter().enumerate() {
            debug_assert!(base.len() < base.capacity());
            base.push(states);
            if tile != WALL && blocks.in_keeper(x as Cell) {
                states += blocks.labels(x as Cell).count_ones() as u16;
            }
        }
        debug_assert!(usize::from(states) <= masks.capacity());
        masks.resize(usize::from(states), 0);
        goal_of.resize(cells, NO_GOAL);
        for (t, &goal) in heuristic.goal_cells().iter().enumerate() {
            goal_of[usize::from(goal)] = t as u8;
        }
        let mut reach = Self {
            base,
            masks,
            goal_of,
        };
        for (t, &goal) in heuristic.goal_cells().iter().enumerate() {
            if blocks.in_keeper(goal) {
                reach.search(neighbors, blocks, goal, 1 << t, queue);
            }
        }
        Ok(reach)
    }
    /// Sets `bit` on every state from which the box reaches `goal`, by a
    /// breadth-first search back from each side of `goal`. State `(y, k)`
    /// came from the box on a neighbor `b` of `y`, pushed onto `y` by the
    /// keeper standing on the cell beyond `b`, when that cell is floor and
    /// the push leaves the keeper on side `k` of `y`, the side holding `b`.
    /// Each state enters the queue at most once, when its bit is set.
    fn search(
        &mut self,
        neighbors: &[[Cell; 4]],
        blocks: &Blocks,
        goal: Cell,
        bit: u32,
        queue: &mut [u16],
    ) {
        let mut tail = 0;
        let labels = blocks.labels(goal);
        for label in (0..4).filter(|&label| (labels >> label) & 1 != 0) {
            let at = self.index(blocks, goal, label);
            self.masks[at] |= bit;
            queue[tail] = pack(goal, label);
            tail += 1;
        }
        let mut head = 0;
        while head < tail {
            let (y, side) = unpack(queue[head]);
            head += 1;
            for (e, &b) in neighbors[usize::from(y)].iter().enumerate() {
                if b == NONE {
                    continue;
                }
                let stand = neighbors[usize::from(b)][e];
                if stand == NONE || blocks.side(y, b) != side {
                    continue;
                }
                let label = blocks.side(b, stand);
                let at = self.index(blocks, b, label);
                if self.masks[at] & bit == 0 {
                    self.masks[at] |= bit;
                    queue[tail] = pack(b, label);
                    tail += 1;
                }
            }
        }
    }
    /// The state index of box cell `x` with the keeper on its side `label`.
    fn index(&self, blocks: &Blocks, x: Cell, label: u8) -> usize {
        let labels = blocks.labels(x);
        debug_assert!((labels >> label) & 1 != 0);
        let below = labels & ((1 << label) - 1);
        usize::from(self.base[usize::from(x)]) + below.count_ones() as usize
    }
    /// The goal columns box cell `x` reaches alone with the keeper on
    /// `player`, a floor cell other than `x`.
    fn reach(&self, blocks: &Blocks, x: Cell, player: Cell) -> u32 {
        if blocks.in_keeper(x) {
            self.masks[self.index(blocks, x, blocks.side(x, player))]
        } else {
            match self.goal_of[usize::from(x)] {
                NO_GOAL => 0,
                t => 1 << t,
            }
        }
    }
    /// Whether each box of `state` can take a distinct goal of its own
    /// label that it reaches alone: Kuhn's augmenting paths over the masks,
    /// each cut to the goal columns of the box's group. `false` proves the
    /// state has no solution. No allocation: the masks and owners are
    /// arrays on the stack.
    pub(crate) fn matched(&self, blocks: &Blocks, heuristic: &Heuristic, state: &State) -> bool {
        let boxes = heuristic.goal_cells().len();
        let mut masks = [0u32; MAX_BOXES];
        for (i, (mask, &cell)) in masks.iter_mut().zip(&state.boxes[..boxes]).enumerate() {
            *mask = self.reach(blocks, cell, state.player) & column_bits(heuristic.group(i));
        }
        let mut owner = [FREE; MAX_BOXES];
        (0..boxes).all(|i| augment(i, &masks[..boxes], &mut owner, &mut 0))
    }
}

/// A build state, box cell `cell` with the keeper on its side `label`,
/// packed for the queue.
fn pack(cell: Cell, label: u8) -> u16 {
    (cell << 2) | u16::from(label)
}

/// The box cell and side label of a packed build state.
fn unpack(state: u16) -> (Cell, u8) {
    (state >> 2, (state & 3) as u8)
}

/// The goal columns `columns`, a group's nonempty range, as a bit set.
fn column_bits(columns: Range<usize>) -> u32 {
    let len = columns.len() as u32;
    debug_assert!((1..=u32::BITS).contains(&len));
    (u32::MAX >> (u32::BITS - len)) << columns.start
}

/// Kuhn's augmenting path from box `i`: whether it takes a goal from its
/// mask that is free, or whose holder moves on to another, never retrying
/// a goal in `seen` this round. `owner` holds each goal's box or `FREE`.
fn augment(i: usize, masks: &[u32], owner: &mut [u8; MAX_BOXES], seen: &mut u32) -> bool {
    let mut free = masks[i] & !*seen;
    while free != 0 {
        let t = free.trailing_zeros() as usize;
        free &= free - 1;
        if (*seen >> t) & 1 != 0 {
            continue;
        }
        *seen |= 1 << t;
        let holder = owner[t];
        if holder == FREE || augment(usize::from(holder), masks, owner, seen) {
            owner[t] = i as u8;
            return true;
        }
    }
    false
}

/// Every sink line, by its head, and the cells the lines cover; see the
/// module docs.
pub(crate) struct SinkLines {
    /// Each line's first cell, leftmost or topmost, with its axis at bit
    /// `AXIS_SHIFT`: rows first, then columns, each in cell order.
    heads: Vec<u16>,
    /// The `on_line` set: bit `c % 32` of word `c / 32` is set when cell
    /// `c` lies on some line.
    covered: Vec<u32>,
}

impl SinkLines {
    /// Heap bytes `build` reserves for a board of `cells` cells: at most a
    /// head per cell and one bit per cell.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) const fn bytes_for(cells: usize) -> usize {
        cells * size_of::<u16>() + cells.div_ceil(32) * size_of::<u32>()
    }
    /// Finds every line, rows then columns, in one pass over the cells per
    /// axis: a line starts at a cell with a wall behind it and floor ahead,
    /// and each start walks its run once to test it and, when every cell
    /// has a wall across, once more to cover it. The two vectors are the
    /// only allocations, reserved once at their bounds.
    pub(crate) fn build(board: &Board) -> Result<Self, TryReserveError> {
        let neighbors = board.neighbors();
        let cells = neighbors.len();
        let mut heads: Vec<u16> = Vec::new();
        heads.try_reserve_exact(cells)?;
        let mut covered: Vec<u32> = Vec::new();
        covered.try_reserve_exact(cells.div_ceil(32))?;
        covered.resize(cells.div_ceil(32), 0);
        for (axis, &[back, forward, a, b]) in AXES.iter().enumerate() {
            for (c, around) in neighbors.iter().enumerate() {
                // A wall has no neighbors, so it never starts a line.
                if around[back] != NONE || around[forward] == NONE {
                    continue;
                }
                let head = c as Cell;
                let frozen = walk(neighbors, head, forward).all(|x| {
                    let across = neighbors[usize::from(x)];
                    across[a] == NONE || across[b] == NONE
                });
                if frozen {
                    debug_assert!(heads.len() < heads.capacity());
                    heads.push(head | ((axis as u16) << AXIS_SHIFT));
                    for x in walk(neighbors, head, forward) {
                        covered[usize::from(x) / 32] |= 1 << (x % 32);
                    }
                }
            }
        }
        Ok(Self { heads, covered })
    }
    /// Whether every line's boxes, as `deadlock` was last refreshed, embed
    /// in order into the line's goals of their labels: walking the line,
    /// each box takes the first goal of its own label past the previous
    /// box's. `false` proves the state has no solution. `O(cells)` per
    /// call, each line walked twice, with no allocation.
    pub(crate) fn sink_ok(&self, board: &Board, deadlock: &Deadlock) -> bool {
        let neighbors = board.neighbors();
        for &head in &self.heads {
            let (cell, forward) = unpack_head(head);
            let mut goals = walk(neighbors, cell, forward);
            for c in walk(neighbors, cell, forward) {
                if let Some(i) = deadlock.at(c)
                    && !goals.any(|g| board.on_goal(i, g))
                {
                    return false;
                }
            }
        }
        true
    }
    /// Whether cell `c` lies on some line.
    pub(crate) fn on_line(&self, c: Cell) -> bool {
        (self.covered[usize::from(c) / 32] >> (c % 32)) & 1 != 0
    }
}

/// The cells from `head` on in direction `forward`, up to the wall.
fn walk(neighbors: &[[Cell; 4]], head: Cell, forward: usize) -> impl Iterator<Item = Cell> {
    iter::successors(Some(head), move |&x| {
        let next = neighbors[usize::from(x)][forward];
        (next != NONE).then_some(next)
    })
}

/// A line head's first cell and the direction its line runs.
fn unpack_head(head: u16) -> (Cell, usize) {
    let axis = usize::from(head >> AXIS_SHIFT);
    (head & ((1 << AXIS_SHIFT) - 1), AXES[axis][1])
}

#[cfg(test)]
mod tests {
    use crate::deadlock::{Deadlock, GoalReach, SinkLines};
    use crate::heuristic::Heuristic;
    use crate::reach::Blocks;
    use crate::testkit::{Lcg, catalog, components, probe_groups, random_room};
    use sokomind_core::{Board, Cell, MAX_CELLS, NONE, OPPOSITE, State, WALL};
    use std::mem::size_of;

    /// The keeper's row holds the `A` box, a `b` goal and the `a` goal.
    /// Below a wall row, where the keeper never goes, lie both `B` boxes
    /// and the other `b` goal.
    const SPLIT: &str = "R Ab a\nOOOOOO\nB  B b";

    /// Every catalog board, `SPLIT` and 100 rooms drawn from `rng`.
    fn boards(rng: &mut Lcg) -> Vec<(String, Board)> {
        let mut boards = catalog();
        boards.push(("split".to_owned(), Board::parse(SPLIT).unwrap()));
        for n in 0..100 {
            let board = Board::parse(&random_room(rng)).unwrap();
            boards.push((format!("room {n}"), board));
        }
        boards
    }

    /// Every floor cell of `board`.
    fn floor_cells(board: &Board) -> Vec<Cell> {
        let tiles = board.tiles();
        (0..tiles.len() as Cell)
            .filter(|&c| tiles[usize::from(c)] != WALL)
            .collect()
    }

    /// Blocks from the start keeper and GoalReach over them, both built on
    /// junk scratch.
    fn build(board: &Board, heuristic: &Heuristic) -> (Blocks, GoalReach) {
        let mut stack = vec![0xbeef; MAX_CELLS];
        let mut low = vec![0xbeef; MAX_CELLS];
        let mut next = vec![0xee; MAX_CELLS];
        let player = board.initial().player;
        let built = Blocks::build(board, player, &mut stack, &mut low, &mut next);
        let blocks = built.unwrap();
        let mut queue = vec![0xbeef; 2 * MAX_CELLS];
        let table = GoalReach::build(board, heuristic, &blocks, &mut queue).unwrap();
        (blocks, table)
    }

    /// A port of the stage probe's `reaches` (slurm/probes/p4b.rs 291, not
    /// tracked) on its own floods: one forward search per state over the
    /// whole board. `comp[x]` numbers the floor components left once `x` is
    /// blocked, and state `(x, k)`, at `base[x] + k`, has in `reach` the
    /// goal columns one box from `x` reaches alone with the keeper in `k`.
    struct Probe {
        comp: Vec<Vec<usize>>,
        base: Vec<usize>,
        reach: Vec<u32>,
    }

    impl Probe {
        fn new(board: &Board, heuristic: &Heuristic) -> Self {
            let (tiles, neighbors) = (board.tiles(), board.neighbors());
            let cells = tiles.len();
            let comp: Vec<Vec<usize>> = (0..cells)
                .map(|x| match tiles[x] {
                    WALL => vec![usize::MAX; cells],
                    _ => components(board, Some(x)),
                })
                .collect();
            let mut base = Vec::new();
            let mut cell_of = Vec::new();
            for (x, row) in comp.iter().enumerate() {
                base.push(cell_of.len());
                let ids = row.iter().filter(|&&k| k != usize::MAX);
                let count = ids.max().map_or(0, |&k| k + 1);
                cell_of.resize(cell_of.len() + count, x);
            }
            let mut goal_bit = vec![0u32; cells];
            for (t, &goal) in heuristic.goal_cells().iter().enumerate() {
                goal_bit[usize::from(goal)] |= 1 << t;
            }
            let mut reach = vec![0; cell_of.len()];
            let mut seen = vec![usize::MAX; cell_of.len()];
            let mut stack = Vec::new();
            for start in 0..cell_of.len() {
                seen[start] = start;
                stack.push(start);
                while let Some(state) = stack.pop() {
                    let x = cell_of[state];
                    let k = state - base[x];
                    reach[start] |= goal_bit[x];
                    for (d, &y) in neighbors[x].iter().enumerate() {
                        let stand = neighbors[x][OPPOSITE[d]];
                        if y == NONE || stand == NONE || comp[x][usize::from(stand)] != k {
                            continue;
                        }
                        let y = usize::from(y);
                        let next = base[y] + comp[y][x];
                        if seen[next] != start {
                            seen[next] = start;
                            stack.push(next);
                        }
                    }
                }
            }
            Self { comp, base, reach }
        }

        /// The probe's goal columns for box cell `x`, keeper on `player`.
        fn mask(&self, x: Cell, player: Cell) -> u32 {
            let k = self.comp[usize::from(x)][usize::from(player)];
            self.reach[self.base[usize::from(x)] + k]
        }
    }

    /// Whether the boxes take distinct goal columns from their masks: box
    /// `i` extends every set of `i` columns the earlier boxes can take, a
    /// DP over column sets in `O(2^columns * columns)`.
    fn perfect(masks: &[u32]) -> bool {
        let sets = 1usize << masks.len();
        let mut ok = vec![false; sets];
        ok[0] = true;
        for set in 0..sets {
            if !ok[set] {
                continue;
            }
            let i = set.count_ones() as usize;
            if i == masks.len() {
                return true;
            }
            for t in 0..masks.len() {
                if (masks[i] >> t) & 1 != 0 && (set >> t) & 1 == 0 {
                    ok[set | (1 << t)] = true;
                }
            }
        }
        false
    }

    /// A random state: the keeper on a cell of `keeper` and each box, half
    /// the time, on a free goal of its label while one is left, else on a
    /// random free cell of `floor`.
    fn place(
        rng: &mut Lcg,
        board: &Board,
        heuristic: &Heuristic,
        keeper: &[Cell],
        floor: &[Cell],
    ) -> State {
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

    /// A port of the stage probe's `sink_lines`: each wall-to-wall row,
    /// then column, of at least two cells, every one with a wall on one
    /// side across it.
    fn probe_lines(board: &Board) -> Vec<Vec<Cell>> {
        let neighbors = board.neighbors();
        let mut lines = Vec::new();
        for (back, forward, a, b) in [(2, 3, 0, 1), (0, 1, 2, 3)] {
            for (c, (&tile, around)) in board.tiles().iter().zip(neighbors).enumerate() {
                if tile == WALL || around[back] != NONE {
                    continue;
                }
                let mut run = Vec::new();
                let mut frozen = true;
                let mut x = c as Cell;
                loop {
                    let adjacent = neighbors[usize::from(x)];
                    frozen &= adjacent[a] == NONE || adjacent[b] == NONE;
                    run.push(x);
                    if adjacent[forward] == NONE {
                        break;
                    }
                    x = adjacent[forward];
                }
                if frozen && run.len() >= 2 {
                    lines.push(run);
                }
            }
        }
        lines
    }

    /// A port of the stage probe's `sink_ok`, with `occupant` holding one
    /// plus the box slot on each cell, 0 on cells without a box.
    fn probe_sink_ok(
        lines: &[Vec<Cell>],
        occupant: &[usize],
        group: &[usize],
        goal_group: &[usize],
    ) -> bool {
        lines.iter().all(|line| {
            let mut next = 0;
            line.iter().all(|&c| {
                let o = occupant[usize::from(c)];
                if o == 0 {
                    return true;
                }
                let g = group[o - 1];
                let fits = |&k: &usize| goal_group[usize::from(line[k])] == g;
                match (next..line.len()).find(fits) {
                    Some(k) => {
                        next = k + 1;
                        true
                    }
                    None => false,
                }
            })
        })
    }

    /// GoalReach against the probe's forward search on every catalog
    /// board, `SPLIT` and 100 rooms. Each build reserves exactly
    /// `bytes_for` and numbers one state per side of each keeper cell.
    /// Every state is checked, with the keeper on each floor neighbor of
    /// the box (every side touches it) and on the start cell, and every
    /// floor cell outside the keeper's component with the start keeper;
    /// `SPLIT` makes sure there are some.
    #[test]
    fn goal_reach_matches_forward_bfs() {
        let mut rng = Lcg(0x60a1);
        let mut outside = 0;
        for (id, board) in boards(&mut rng) {
            let heuristic = Heuristic::new(&board);
            let (blocks, table) = build(&board, &heuristic);
            let bytes = table.base.capacity() * size_of::<u16>()
                + table.masks.capacity() * size_of::<u32>()
                + table.goal_of.capacity() * size_of::<u8>();
            assert_eq!(bytes, GoalReach::bytes_for(board.tiles().len()), "{id}");
            let probe = Probe::new(&board, &heuristic);
            let player = board.initial().player;
            let floor = floor_cells(&board);
            let (keeper, other): (Vec<Cell>, Vec<Cell>) =
                floor.into_iter().partition(|&c| blocks.in_keeper(c));
            let mut states = 0;
            for &x in &keeper {
                let around = board.neighbors()[usize::from(x)];
                let mut sides = Vec::new();
                for c in around.into_iter().filter(|&c| c != NONE).chain([player]) {
                    if c != x {
                        let got = table.reach(&blocks, x, c);
                        assert_eq!(got, probe.mask(x, c), "{id}: box {x}, keeper {c}");
                        sides.push(probe.comp[usize::from(x)][usize::from(c)]);
                    }
                }
                sides.sort_unstable();
                sides.dedup();
                states += sides.len();
            }
            assert_eq!(table.masks.len(), states, "{id}");
            for &x in &other {
                let got = table.reach(&blocks, x, player);
                assert_eq!(got, probe.mask(x, player), "{id}: box {x} outside");
            }
            outside += other.len();
        }
        assert!(outside > 0);
    }

    /// `matched` against a brute-force matching over the probe's masks on
    /// 50 random states of every catalog board with at most 10 boxes,
    /// `SPLIT` and 100 rooms. Placing boxes on free goals of their labels
    /// half the time makes both verdicts come up.
    #[test]
    fn matched_matches_brute_force() {
        let mut rng = Lcg(0x3a7c);
        let mut verdicts = [0; 2];
        for (id, board) in boards(&mut rng) {
            if board.labels().len() > 10 {
                continue;
            }
            let heuristic = Heuristic::new(&board);
            let (blocks, table) = build(&board, &heuristic);
            let probe = Probe::new(&board, &heuristic);
            let floor = floor_cells(&board);
            let mut keeper = floor.clone();
            keeper.retain(|&c| blocks.in_keeper(c));
            for _ in 0..50 {
                let state = place(&mut rng, &board, &heuristic, &keeper, &floor);
                let masks: Vec<u32> = (0..board.labels().len())
                    .map(|i| {
                        let columns = heuristic.group(i).fold(0u32, |m, t| m | (1 << t));
                        probe.mask(state.boxes[i], state.player) & columns
                    })
                    .collect();
                let expected = perfect(&masks);
                let matched = table.matched(&blocks, &heuristic, &state);
                assert_eq!(matched, expected, "{id}: {state:?}");
                verdicts[usize::from(matched)] += 1;
            }
        }
        assert!(verdicts.iter().all(|&n| n > 0), "{verdicts:?}");
    }

    /// SinkLines against the ports of the probe's `sink_lines` and
    /// `sink_ok` on every catalog board, `SPLIT` and 100 rooms: a head per
    /// line, `on_line` on exactly the lines' cells, and `sink_ok` agreeing
    /// on 50 random box placements per board. Each box lands on a line
    /// cell a quarter of the time, so lines often hold several boxes and
    /// both verdicts come up.
    #[test]
    fn sink_matches_probe() {
        let mut rng = Lcg(0x51ec);
        let mut verdicts = [0; 2];
        for (id, board) in boards(&mut rng) {
            let lines = probe_lines(&board);
            let (group, goal_group) = probe_groups(&board);
            let sink = SinkLines::build(&board).unwrap();
            let bytes = sink.heads.capacity() * size_of::<u16>()
                + sink.covered.capacity() * size_of::<u32>();
            assert_eq!(bytes, SinkLines::bytes_for(board.tiles().len()), "{id}");
            assert_eq!(sink.heads.len(), lines.len(), "{id}");
            let line_cells: Vec<Cell> = lines.concat();
            for c in 0..board.tiles().len() as Cell {
                assert_eq!(sink.on_line(c), line_cells.contains(&c), "{id}: {c}");
            }
            let floor = floor_cells(&board);
            let boxes = board.labels().len();
            let mut deadlock = Deadlock::new(&board);
            for _ in 0..50 {
                let mut placed: Vec<Cell> = Vec::new();
                while placed.len() < boxes {
                    let on_line = !line_cells.is_empty() && rng.below(4) == 0;
                    let pool = if on_line { &line_cells } else { &floor };
                    let c = pool[rng.below(pool.len())];
                    if !placed.contains(&c) {
                        placed.push(c);
                    }
                }
                deadlock.refresh(&placed);
                let mut occupant = vec![0; board.tiles().len()];
                for (i, &c) in placed.iter().enumerate() {
                    occupant[usize::from(c)] = i + 1;
                }
                let want = probe_sink_ok(&lines, &occupant, &group, &goal_group);
                let got = sink.sink_ok(&board, &deadlock);
                assert_eq!(got, want, "{id}: {placed:?}");
                verdicts[usize::from(want)] += 1;
            }
        }
        assert!(verdicts.iter().all(|&n| n > 0), "{verdicts:?}");
    }
}
