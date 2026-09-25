use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, State};

const INF: i32 = 1_000_000;
/// A parent's duals amortize across a node's children, so the repair path
/// only pays once a label group is large enough for a full solve to dominate
/// child generation. Below this, per-child full solves of one group win.
const REPAIR_CROSSOVER: usize = 8;

/// Per-goal reverse-push distances plus per-label assignment with duals.
pub struct Heuristic {
    /// Parallel to `board.goals`: push distance from the goal to each cell.
    distances: Vec<Vec<u16>>,
    /// Boxes are grouped by label into contiguous index ranges.
    groups: Vec<Group>,
}
struct Group {
    start: usize,
    len: usize,
    /// Column offset of this group's goals in `distances`.
    base: usize,
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
        let mut queue = Vec::with_capacity(board.tiles.len());
        let mut reverse = |goal: Cell| {
            let mut dist = vec![NONE; board.tiles.len()];
            queue.clear();
            queue.push(goal);
            dist[goal as usize] = 0;
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
                    if support != NONE && dist[previous as usize] == NONE {
                        dist[previous as usize] = dist[cell as usize] + 1;
                        queue.push(previous);
                    }
                }
            }
            dist
        };
        // Distance columns are stored in group order so a group's columns are
        // contiguous: the Hungarian inner loop stays single-indirection.
        let mut distances = Vec::with_capacity(board.goals.len());
        let mut groups = Vec::new();
        let mut start = 0;
        while start < board.labels.len() {
            let label = board.labels[start];
            let mut end = start + 1;
            while end < board.labels.len() && board.labels[end] == label {
                end += 1;
            }
            let base = distances.len();
            for &(goal, goal_label) in &board.goals {
                if goal_label == label {
                    distances.push(reverse(goal));
                }
            }
            groups.push(Group {
                start,
                len: end - start,
                base,
            });
            start = end;
        }
        Self {
            distances,
            groups,
        }
    }

    fn cost(&self, goal: usize, cell: Cell) -> i32 {
        let distance = self.distances[goal][cell as usize];
        if distance == NONE {
            INF
        } else {
            distance as i32
        }
    }

    /// Whether any label group is large enough for dual repair to amortize.
    pub fn repairs_worthwhile(&self) -> bool {
        self.groups.iter().any(|group| group.len >= REPAIR_CROSSOVER)
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
            total += self.solve_group(group, &state.boxes[group.start..group.start + group.len], false)?.0;
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
            let cost = self.cost(group.base, cells[0]);
            if cost >= INF {
                return None;
            }
            if !duals {
                return Some((cost as u32, None));
            }
            let mut columns = [0i32; MAX_BOXES];
            let mut v = [0i32; MAX_BOXES];
            v[0] = cost;
            return Some((
                cost as u32,
                Some(GroupSolution {
                    cost: cost as u32,
                    columns,
                    u: [0i32; MAX_BOXES],
                    v,
                }),
            ));
        }
        let mut u = [0i32; MAX_BOXES + 1];
        let mut v = [0i32; MAX_BOXES + 1];
        let mut p = [0usize; MAX_BOXES + 1];
        let mut way = [0usize; MAX_BOXES + 1];
        for row in 1..=n {
            p[0] = row;
            let mut minv = [INF; MAX_BOXES + 1];
            let mut used = [false; MAX_BOXES + 1];
            let mut j0 = 0;
            loop {
                used[j0] = true;
                let i0 = p[j0];
                let mut delta = INF;
                let mut j1 = 0;
                for j in 1..=n {
                    if used[j] {
                        continue;
                    }
                    let reduced =
                        self.cost(group.base + j - 1, cells[i0 - 1]) - u[i0] - v[j];
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
                    return None;
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
        }
        let mut columns = [-1i32; MAX_BOXES];
        let mut cost = 0i32;
        for j in 1..=n {
            let row = p[j];
            columns[row - 1] = (j - 1) as i32;
            cost += self.cost(group.base + j - 1, cells[row - 1]);
        }
        if cost >= INF {
            return None;
        }
        if !duals {
            return Some((cost as u32, None));
        }
        let mut out_u = [0i32; MAX_BOXES];
        let mut out_v = [0i32; MAX_BOXES];
        out_u[..n].copy_from_slice(&u[1..=n]);
        out_v[..n].copy_from_slice(&v[1..=n]);
        Some((
            cost as u32,
            Some(GroupSolution {
                cost: cost as u32,
                columns,
                u: out_u,
                v: out_v,
            }),
        ))
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
        let mut row_potential = [0i32; MAX_BOXES + 1];
        let mut column_potential = [0i32; MAX_BOXES + 1];
        let mut p = [0usize; MAX_BOXES + 1];
        for i in 0..n {
            row_potential[i + 1] = u[i];
        }
        for j in 0..n {
            column_potential[j + 1] = previous.v[j];
        }
        for r in 0..n {
            if columns[r] >= 0 {
                p[columns[r] as usize + 1] = r + 1;
            } else if r != changed {
                return Repair::Diff;
            }
        }
        p[0] = changed + 1;
        let mut way = [0usize; MAX_BOXES + 1];
        let mut minv = [INF; MAX_BOXES + 1];
        let mut used = [false; MAX_BOXES + 1];
        let mut j0 = 0usize;
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = INF;
            let mut j1 = 0usize;
            for j in 1..=n {
                if used[j] {
                    continue;
                }
                let reduced = self.cost(group.base + j - 1, child_cells[i0 - 1])
                    - row_potential[i0]
                    - column_potential[j];
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
                return Repair::Infeasible;
            }
            for j in 0..=n {
                if used[j] {
                    row_potential[p[j]] += delta;
                    column_potential[j] -= delta;
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
        let mut cost = 0i32;
        for j in 1..=n {
            let row = p[j];
            cost += self.cost(group.base + j - 1, child_cells[row - 1]);
        }
        if cost >= INF {
            return Repair::Infeasible;
        }
        Repair::Cost(cost as u32)
    }
}
