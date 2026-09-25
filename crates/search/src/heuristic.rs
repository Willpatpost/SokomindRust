use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, State};

const INF: i32 = 1_000_000;
/// A parent's duals amortize across a node's children, so the repair path
/// only pays once a label group is large enough for a full solve to dominate
/// child generation. Below this, per-child full solves of one group win.
const REPAIR_CROSSOVER: usize = 8;

/// Per-goal reverse-push distances plus per-label assignment with duals.
pub struct Heuristic {
    /// Cell-major push distances: `distances[cell * goals + goal]` is the
    /// distance from `cell` to goal column `goal`, `NONE` when unreachable.
    distances: Vec<u16>,
    goals: usize,
    /// Boxes are grouped by label into contiguous index ranges.
    groups: Vec<Group>,
}
/// A label group's boxes are indices `start..start + len`. Goal columns
/// follow the same order, so the group's goals are the same column range.
struct Group {
    start: usize,
    len: usize,
}

/// Hungarian working state for one group: 1-based potentials and matching,
/// with row and column 0 as the dummies the augment starts from.
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

/// An optimal box/goal assignment per label group, carrying the dual
/// potentials that `estimate_from` repairs after a single box move.
pub struct Assignment {
    total: u32,
    /// The cells this assignment solved for, sorted within each group.
    cells: [Cell; MAX_BOXES],
    groups: Vec<GroupSolution>,
}
struct GroupSolution {
    cost: u32,
    /// Matched local goal index per local row, -1 when unmatched.
    columns: [i32; MAX_BOXES],
    /// Dual potentials per local row and per local goal.
    u: [i32; MAX_BOXES],
    v: [i32; MAX_BOXES],
}
enum Repair {
    Cost(u32),
    /// The single-row augment proved no perfect matching exists.
    Infeasible,
    /// Not a single-cell change; the caller falls back to a full solve.
    Diff,
}

impl Heuristic {
    pub fn new(board: &Board) -> Self {
        let mut groups = Vec::new();
        let mut start = 0;
        for run in board.labels.chunk_by(|a, b| a == b) {
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
        Self {
            distances,
            goals,
            groups,
        }
    }

    /// Distances from `cell` to each of `group`'s goals, in column order.
    fn goal_distances(&self, group: &Group, cell: Cell) -> &[u16] {
        let at = cell as usize * self.goals + group.start;
        &self.distances[at..at + group.len]
    }

    /// Whether any label group is large enough for dual repair to amortize.
    pub fn repairs_worthwhile(&self) -> bool {
        self.groups
            .iter()
            .any(|group| group.len >= REPAIR_CROSSOVER)
    }

    fn sorted_cells(&self, state: &State) -> [Cell; MAX_BOXES] {
        let mut cells = state.boxes;
        for group in &self.groups {
            cells[group.start..group.start + group.len].sort_unstable();
        }
        cells
    }

    /// Minimum-cost label-compatible matching over relaxed push distances.
    /// Admissible for total moves: the relaxation removes every other box.
    /// Lean path: totals only, no dual extraction or cell sorting.
    pub fn estimate(&self, state: &State) -> Option<u32> {
        let mut total = 0u32;
        for group in &self.groups {
            total += self
                .solve_group(
                    group,
                    &state.boxes[group.start..group.start + group.len],
                    false,
                )?
                .0;
        }
        Some(total)
    }

    /// Incremental estimate after one box moved: the changed label group is
    /// repaired from the parent's duals; other groups reuse the parent cost.
    pub fn estimate_from(&self, parent: &Assignment, child: &State) -> Option<u32> {
        let cells = self.sorted_cells(child);
        let mut total = parent.total;
        for (g, group) in self.groups.iter().enumerate() {
            let (start, end) = (group.start, group.start + group.len);
            if cells[start..end] == parent.cells[start..end] {
                continue;
            }
            total -= parent.groups[g].cost;
            if group.len >= REPAIR_CROSSOVER {
                match self.repair_group(
                    group,
                    &parent.cells[start..end],
                    &cells[start..end],
                    &parent.groups[g],
                ) {
                    Repair::Cost(cost) => {
                        total += cost;
                        continue;
                    }
                    Repair::Infeasible => return None,
                    Repair::Diff => {}
                }
            }
            total += self.solve_group(group, &cells[start..end], false)?.0;
        }
        Some(total)
    }

    pub fn assignment(&self, state: &State) -> Option<Assignment> {
        let cells = self.sorted_cells(state);
        let mut total = 0u32;
        let mut solutions = Vec::with_capacity(self.groups.len());
        for group in &self.groups {
            let (cost, solution) =
                self.solve_group(group, &cells[group.start..group.start + group.len], true)?;
            total += cost;
            if let Some(solution) = solution {
                solutions.push(solution);
            }
        }
        Some(Assignment {
            total,
            cells,
            groups: solutions,
        })
    }

    /// Deterministic Hungarian over one label group. `duals` also extracts
    /// the potentials later repairs need; the lean path skips that cost.
    fn solve_group(
        &self,
        group: &Group,
        cells: &[Cell],
        duals: bool,
    ) -> Option<(u32, Option<GroupSolution>)> {
        let n = group.len;
        // A one-box group needs no Hungarian: one distance is the assignment.
        if n == 1 {
            let cost = cost(self.goal_distances(group, cells[0])[0]);
            if cost >= INF {
                return None;
            }
            if !duals {
                return Some((cost as u32, None));
            }
            let mut v = [0i32; MAX_BOXES];
            v[0] = cost;
            return Some((
                cost as u32,
                Some(GroupSolution {
                    cost: cost as u32,
                    columns: [0i32; MAX_BOXES],
                    u: [0i32; MAX_BOXES],
                    v,
                }),
            ));
        }
        let mut state = Duals::EMPTY;
        for row in 1..=n {
            if !self.augment(group, cells, row, &mut state) {
                return None;
            }
        }
        let cost = self.matched_cost(group, cells, &state)?;
        if !duals {
            return Some((cost, None));
        }
        let mut columns = [-1i32; MAX_BOXES];
        for j in 1..=n {
            columns[state.p[j] - 1] = (j - 1) as i32;
        }
        let mut u = [0i32; MAX_BOXES];
        let mut v = [0i32; MAX_BOXES];
        u[..n].copy_from_slice(&state.u[1..=n]);
        v[..n].copy_from_slice(&state.v[1..=n]);
        Some((
            cost,
            Some(GroupSolution {
                cost,
                columns,
                u,
                v,
            }),
        ))
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

    /// One-row repair of a parent's optimal assignment: remap the unchanged
    /// rows onto the child's cell order, then run a single augmenting path
    /// from the changed row. With optimal parent duals, one augment restores
    /// optimality; a missing finite column proves no matching exists.
    fn repair_group(
        &self,
        group: &Group,
        parent_cells: &[Cell],
        child_cells: &[Cell],
        previous: &GroupSolution,
    ) -> Repair {
        let n = group.len;
        // Require exactly one removed and one added cell.
        let (mut pi, mut ci) = (0, 0);
        let (mut removed, mut added) = (-1i32, -1i32);
        while pi < n && ci < n {
            if parent_cells[pi] == child_cells[ci] {
                pi += 1;
                ci += 1;
            } else if parent_cells[pi] < child_cells[ci] {
                if removed >= 0 {
                    return Repair::Diff;
                }
                removed = parent_cells[pi] as i32;
                pi += 1;
            } else {
                if added >= 0 {
                    return Repair::Diff;
                }
                added = child_cells[ci] as i32;
                ci += 1;
            }
        }
        while pi < n {
            if removed >= 0 {
                return Repair::Diff;
            }
            removed = parent_cells[pi] as i32;
            pi += 1;
        }
        while ci < n {
            if added >= 0 {
                return Repair::Diff;
            }
            added = child_cells[ci] as i32;
            ci += 1;
        }
        if removed < 0 || added < 0 {
            return Repair::Diff;
        }
        let mut columns = [-1i32; MAX_BOXES];
        let mut u = [0i32; MAX_BOXES];
        let mut changed = usize::MAX;
        for (ci, &cell) in child_cells.iter().enumerate() {
            if let Some(pi) = parent_cells.iter().position(|&c| c == cell) {
                columns[ci] = previous.columns[pi];
                u[ci] = previous.u[pi];
            } else {
                if changed != usize::MAX {
                    return Repair::Diff;
                }
                changed = ci;
            }
        }
        let Some(changed) = (changed != usize::MAX).then_some(changed) else {
            return Repair::Diff;
        };
        let mut state = Duals::EMPTY;
        state.u[1..=n].copy_from_slice(&u[..n]);
        state.v[1..=n].copy_from_slice(&previous.v[..n]);
        for r in 0..n {
            if columns[r] >= 0 {
                state.p[columns[r] as usize + 1] = r + 1;
            } else if r != changed {
                return Repair::Diff;
            }
        }
        if !self.augment(group, child_cells, changed + 1, &mut state) {
            return Repair::Infeasible;
        }
        match self.matched_cost(group, child_cells, &state) {
            Some(cost) => Repair::Cost(cost),
            None => Repair::Infeasible,
        }
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
