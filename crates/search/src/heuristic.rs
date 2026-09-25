use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, State};
use std::mem::size_of;

const INF: i32 = 1_000_000;
/// Group size from which a child's group cost is repaired from the parent's
/// duals with one augment instead of re-solved. Models put the crossover at
/// 2-4; re-measure it on real hardware.
const REPAIR_CROSSOVER: usize = 3;
// Each label group owns one bit of a cell's dead mask, and there are at most
// as many groups as boxes.
const _: () = assert!(MAX_BOXES <= u32::BITS as usize);

/// Per-goal reverse-push distances plus per-label assignment with duals.
pub(crate) struct Heuristic {
    /// Cell-major push distances: `distances[cell * goals + goal]` is the
    /// distance from `cell` to goal column `goal`, `NONE` when unreachable.
    distances: Vec<u16>,
    goals: usize,
    /// Boxes are grouped by label into contiguous index ranges.
    groups: Vec<Group>,
    /// The index into `groups` of each box's label group.
    group_of: [u8; MAX_BOXES],
    /// Per cell, bit `g` set when no goal of group `g` is reachable from
    /// the cell: a box of that label there makes every estimate `None`.
    dead: Vec<u32>,
}
/// A label group's boxes are indices `start..start + len`. Goal columns
/// follow the same order, so the group's goals are the same column range.
struct Group {
    start: usize,
    len: usize,
}

/// Hungarian working state for one group: 1-based potentials and matching,
/// with row and column 0 as the dummies the augment starts from.
#[derive(Clone, Copy)]
struct Duals {
    u: [i32; MAX_BOXES + 1],
    v: [i32; MAX_BOXES + 1],
    /// Row matched to each column, 0 when the column is free.
    p: [usize; MAX_BOXES + 1],
    /// Previous column on the augmenting path.
    way: [usize; MAX_BOXES + 1],
}

impl Duals {
    const EMPTY: Self = Self {
        u: [0; MAX_BOXES + 1],
        v: [0; MAX_BOXES + 1],
        p: [0; MAX_BOXES + 1],
        way: [0; MAX_BOXES + 1],
    };
}

/// One label group of the node being expanded, solved once for all of its
/// children. Children come box-major and groups are contiguous index runs,
/// so one slot per expansion is enough.
pub(crate) struct ParentGroup {
    /// Index into `groups`; `usize::MAX` until the first child needs it.
    group: usize,
    /// The group's optimal cost in the parent.
    cost: u32,
    /// The parent's optimal duals and matching for this group.
    duals: Duals,
}

impl ParentGroup {
    pub(crate) const EMPTY: Self = Self {
        group: usize::MAX,
        cost: 0,
        duals: Duals::EMPTY,
    };
}

impl Heuristic {
    /// The dead mask. The distance table's `boxes * 2` bytes per cell are
    /// accounted by the arena separately.
    pub(crate) const BYTES_PER_CELL: usize = size_of::<u32>();
    pub(crate) fn new(board: &Board) -> Self {
        let mut groups = Vec::new();
        let mut group_of = [0; MAX_BOXES];
        let mut start = 0;
        for run in board.labels.chunk_by(|a, b| a == b) {
            group_of[start..start + run.len()].fill(groups.len() as u8);
            groups.push(Group {
                start,
                len: run.len(),
            });
            start += run.len();
        }
        // Labels are sorted and every label has as many goals as boxes, so a
        // stable sort by label lays the goal columns out in group order.
        let mut columns = board.goals.clone();
        columns.sort_by_key(|&(_, label)| label);
        let goals = columns.len();
        let mut distances = vec![NONE; board.tiles.len() * goals];
        let mut queue = Vec::with_capacity(board.tiles.len());
        for (column, &(goal, _)) in columns.iter().enumerate() {
            let at = |cell: Cell| cell as usize * goals + column;
            queue.clear();
            queue.push(goal);
            distances[at(goal)] = 0;
            let mut head = 0;
            while head < queue.len() {
                let cell = queue[head];
                head += 1;
                for direction in OPPOSITE {
                    let previous = board.neighbors[cell as usize][direction];
                    if previous == NONE {
                        continue;
                    }
                    let support = board.neighbors[previous as usize][direction];
                    if support != NONE && distances[at(previous)] == NONE {
                        distances[at(previous)] = distances[at(cell)] + 1;
                        queue.push(previous);
                    }
                }
            }
        }
        let dead = (0..board.tiles.len())
            .map(|cell| {
                let row = &distances[cell * goals..(cell + 1) * goals];
                groups
                    .iter()
                    .enumerate()
                    .filter(|(_, group)| {
                        row[group.start..group.start + group.len]
                            .iter()
                            .all(|&distance| distance == NONE)
                    })
                    .fold(0, |mask, (g, _)| mask | (1u32 << g))
            })
            .collect();
        Self {
            distances,
            goals,
            groups,
            group_of,
            dead,
        }
    }

    /// Whether box `i` on `cell` has no reachable goal of its label. Then
    /// the box's group has no finite matching, so `estimate` and
    /// `child_estimate` would return `None`, and callers may prune first.
    pub(crate) fn dead(&self, i: usize, cell: Cell) -> bool {
        (self.dead[cell as usize] >> self.group_of[i]) & 1 != 0
    }

    /// Distances from `cell` to each of `group`'s goals, in column order.
    fn goal_distances(&self, group: &Group, cell: Cell) -> &[u16] {
        let at = cell as usize * self.goals + group.start;
        &self.distances[at..at + group.len]
    }

    /// Minimum-cost label-compatible matching over relaxed push distances.
    /// Admissible for total moves: the relaxation removes every other box.
    /// It is the sum of independent per-group optima, `None` when some group
    /// has no perfect matching over reachable goals.
    pub(crate) fn estimate(&self, state: &State) -> Option<u32> {
        let mut total = 0u32;
        for group in &self.groups {
            total += self
                .solve(group, &state.boxes[group.start..group.start + group.len])?
                .0;
        }
        Some(total)
    }

    /// `estimate` of the child where box `i` of `parent` moved to `to`,
    /// given the parent's own estimate `parent_h`. A push changes only box
    /// `i`'s group, so the child's total is the parent's with that group's
    /// cost swapped: equal to a fresh `estimate` of the child, because the
    /// optimal cost is unique. `cache` holds the parent's solution of the
    /// last group asked for and is re-solved only when the group changes.
    pub(crate) fn child_estimate(
        &self,
        parent_h: u32,
        cache: &mut ParentGroup,
        parent: &State,
        i: usize,
        to: Cell,
    ) -> Option<u32> {
        let g = self.group_of[i] as usize;
        let group = &self.groups[g];
        let range = group.start..group.start + group.len;
        if cache.group != g {
            // Never `None`: every group of a queued state is feasible.
            let (cost, duals) = self.solve(group, &parent.boxes[range.clone()])?;
            *cache = ParentGroup {
                group: g,
                cost,
                duals,
            };
        }
        let mut boxes = parent.boxes;
        boxes[i] = to;
        let cells = &boxes[range];
        let cost = if group.len < REPAIR_CROSSOVER {
            self.solve(group, cells)?.0
        } else {
            self.repair(group, cells, i - group.start, &cache.duals)?
        };
        Some(parent_h - cache.cost + cost)
    }

    /// Deterministic Hungarian over one label group: its optimal cost with
    /// the final duals, `None` when no perfect matching exists.
    fn solve(&self, group: &Group, cells: &[Cell]) -> Option<(u32, Duals)> {
        // A one-box group needs no Hungarian: one distance is the assignment.
        if group.len == 1 {
            let cost = cost(self.goal_distances(group, cells[0])[0]);
            return (cost < INF).then_some((cost as u32, Duals::EMPTY));
        }
        let mut duals = Duals::EMPTY;
        for row in 1..=group.len {
            if !self.augment(group, cells, row, &mut duals) {
                return None;
            }
        }
        Some((self.matched_cost(group, cells, &duals)?, duals))
    }

    /// One-row repair of a parent's optimal `duals` after only 0-based `row`
    /// moved, to `cells[row]`. Freeing the row's column and zeroing its
    /// potential keeps the duals feasible (every `v <= 0 <= cost`), so one
    /// augmenting path from that row restores an optimal matching; a missing
    /// finite column proves no matching exists.
    fn repair(&self, group: &Group, cells: &[Cell], row: usize, duals: &Duals) -> Option<u32> {
        let mut state = *duals;
        let row = row + 1;
        state.u[row] = 0;
        for p in &mut state.p[1..=group.len] {
            if *p == row {
                *p = 0;
            }
        }
        if !self.augment(group, cells, row, &mut state) {
            return None;
        }
        self.matched_cost(group, cells, &state)
    }

    /// One Hungarian augmenting path from 1-based `row`, which must be
    /// unmatched, over feasible potentials in `state`. Returns false when no
    /// finite column is reachable: then no perfect matching exists.
    fn augment(&self, group: &Group, cells: &[Cell], row: usize, state: &mut Duals) -> bool {
        let n = group.len;
        let Duals { u, v, p, way } = state;
        p[0] = row;
        let mut minv = [INF; MAX_BOXES + 1];
        let mut used = [false; MAX_BOXES + 1];
        let mut j0 = 0;
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let distances = self.goal_distances(group, cells[i0 - 1]);
            let mut delta = INF;
            let mut j1 = 0;
            for j in 1..=n {
                if used[j] {
                    continue;
                }
                let reduced = cost(distances[j - 1]) - u[i0] - v[j];
                if reduced < minv[j] {
                    minv[j] = reduced;
                    way[j] = j0;
                }
                if minv[j] < delta {
                    delta = minv[j];
                    j1 = j;
                }
            }
            if delta >= INF {
                return false;
            }
            for j in 0..=n {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
        true
    }

    /// Total cost of the complete matching in `state`, `None` when it has to
    /// use an unreachable goal.
    fn matched_cost(&self, group: &Group, cells: &[Cell], state: &Duals) -> Option<u32> {
        let mut total = 0i32;
        for j in 1..=group.len {
            total += cost(self.goal_distances(group, cells[state.p[j] - 1])[j - 1]);
        }
        (total < INF).then_some(total as u32)
    }
}

/// A push distance as a Hungarian cost, `INF` when the goal is unreachable.
fn cost(distance: u16) -> i32 {
    if distance == NONE {
        INF
    } else {
        distance as i32
    }
}

#[cfg(test)]
mod tests {
    use super::{Heuristic, ParentGroup, REPAIR_CROSSOVER};
    use sokomind_core::{Board, Cell, NONE};

    /// Nine interchangeable X boxes and a lone A: the widest group here.
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

    /// Fixed-seed LCG, so every run sees the same boards and walks.
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

    /// An open 11x9 room with `x` X boxes, one A box, their goals and the
    /// robot on distinct cells at least two steps from the wall. A box pushed
    /// next to a wall can never leave that line, which holds no goal, so
    /// walks meet infeasible children.
    fn room(x: usize, rng: &mut Lcg) -> Board {
        let (width, height) = (11, 9);
        let mut rows = vec![vec![b'O'; width]; height];
        for row in &mut rows[1..height - 1] {
            row[1..width - 1].fill(b' ');
        }
        let mut symbols = vec![b'R', b'A', b'a'];
        symbols.extend(std::iter::repeat_n(b'X', x));
        symbols.extend(std::iter::repeat_n(b'S', x));
        for symbol in symbols {
            loop {
                let (column, row) = (2 + rng.below(width - 4), 2 + rng.below(height - 4));
                if rows[row][column] == b' ' {
                    rows[row][column] = symbol;
                    break;
                }
            }
        }
        let text: Vec<&str> = rows
            .iter()
            .map(|row| std::str::from_utf8(row).unwrap())
            .collect();
        Board::parse(&text.join("\n")).unwrap()
    }

    /// Walks `steps` random feasible states of `board`, restarting every 25.
    /// Moves every box onto every free neighbor cell, box-major with one
    /// cache per parent as the engine does, and checks the incremental child
    /// estimate against a fresh one. Tallies `[feasible, infeasible]` per
    /// path: singleton, re-solve, dual repair.
    fn walk(board: &Board, steps: usize, rng: &mut Lcg, counts: &mut [[u32; 2]; 3]) {
        let heuristic = Heuristic::new(board);
        let boxes = board.labels.len();
        let mut state = board.initial;
        for step in 0..steps {
            if step.is_multiple_of(25) {
                state = board.initial;
            }
            let parent_h = heuristic.estimate(&state).unwrap();
            let mut cache = ParentGroup::EMPTY;
            let mut options = Vec::new();
            for i in 0..boxes {
                let len = heuristic.groups[heuristic.group_of[i] as usize].len;
                let path = if len == 1 {
                    0
                } else if len < REPAIR_CROSSOVER {
                    1
                } else {
                    2
                };
                for to in board.neighbors[state.boxes[i] as usize] {
                    if to == NONE || state.boxes[..boxes].contains(&to) {
                        continue;
                    }
                    let mut child = state;
                    child.boxes[i] = to;
                    board.canonicalize(&mut child);
                    let fresh = heuristic.estimate(&child);
                    let incremental = heuristic.child_estimate(parent_h, &mut cache, &state, i, to);
                    assert_eq!(incremental, fresh, "{state:?} box {i} to {to}");
                    counts[path][fresh.is_none() as usize] += 1;
                    if fresh.is_some() {
                        options.push(child);
                    }
                }
            }
            state = if options.is_empty() {
                board.initial
            } else {
                options[rng.below(options.len())]
            };
        }
    }

    /// A box on the top or bottom row can only slide along it, and one in a
    /// corner cannot move at all, so which cells are dead depends on where
    /// each label's goals are.
    #[test]
    fn dead_marks_cells_without_a_reachable_goal_of_the_label() {
        let board = Board::parse(concat!(
            "OOOOOOO\n",
            "O  a  O\n",
            "O A B O\n",
            "O  b  O\n",
            "O R   O\n",
            "OOOOOOO",
        ))
        .unwrap();
        assert_eq!(board.labels, [b'A', b'B']);
        let heuristic = Heuristic::new(&board);
        let at = |x: usize, y: usize| (y * board.width + x) as Cell;
        let (a, b) = (0, 1);
        // A corner is dead for every label.
        assert!(heuristic.dead(a, at(1, 1)) && heuristic.dead(b, at(1, 1)));
        // The top row reaches `a` by a push along it, but never `b` below.
        assert!(!heuristic.dead(a, at(2, 1)) && heuristic.dead(b, at(2, 1)));
        // In the middle both goals are one push away.
        assert!(!heuristic.dead(a, at(3, 2)) && !heuristic.dead(b, at(3, 2)));
        // Nothing pushes a box up off the bottom row, which holds no goal.
        assert!(heuristic.dead(a, at(3, 4)) && heuristic.dead(b, at(3, 4)));
    }

    /// The incremental child estimate equals a fresh estimate for label
    /// groups of 1 through 8 boxes and WIDE's 9, on every path, including
    /// children with no perfect matching.
    #[test]
    fn child_estimate_matches_fresh_estimate() {
        let mut rng = Lcg(1);
        let mut counts = [[0; 2]; 3];
        for x in 1..=8 {
            for _ in 0..3 {
                walk(&room(x, &mut rng), 50, &mut rng, &mut counts);
            }
        }
        walk(&Board::parse(WIDE).unwrap(), 200, &mut rng, &mut counts);
        assert!(counts.iter().flatten().all(|&n| n > 0), "{counts:?}");
    }
}
