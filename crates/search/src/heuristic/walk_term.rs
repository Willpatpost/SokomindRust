//! The dual-transfer walk term: a lower bound, carried per state, on the
//! moves that end off the room tables' counted cells — every push plus
//! every walk between pushes — priced jointly by a linear program. The
//! LP relaxes each box to a flow over the full push graph, folds the goal
//! assignment in per label group, and ships the keeper from each push's
//! end to the next stand over the walk matrix; every route from a state
//! induces a feasible point — its pushes are flows, one same-label goal
//! per box with each goal taken once, its between-push walks a transport
//! — so the optimum never exceeds any route's off-counted cost. The
//! oracle test below checks exactly that, state by state, against exact
//! optima.
//!
//! One solve at the root prices every state: the LP's constraint matrix
//! is fixed and only its rhs — each box's start spot, the goal demand,
//! the keeper's cell — moves with the state, so an optimal dual of the
//! root's LP is dual feasible for every state's, and bounds it by weak
//! duality. The root solve's own duals sit at big-M scale on rows the
//! root leaves unpinned, so a second solve — the L1-minimal dual over
//! the optimal face, the same bound at the root with the least magnitude
//! elsewhere — is carried beside them, and each state takes the larger
//! of the two values. The room bound adds the term's arm, the tables'
//! sum plus the value, to S (docs/room-bound.md).
use super::*;
use crate::lp_solver;
use sokomind_core::WALL;

/// The push graph the term's flows ship over: every reverse-push edge
/// between spots, priced by where the push's move ends. The flow LP's
/// other forms wanted each edge's neighbors and the goal reachability;
/// the dual assembly walks the edge list alone, so those are not kept.
struct PushGraph {
    /// Per edge: the tail spot, the head spot, the cell the keeper ends
    /// the push on (the box's origin), the stand it pushed from, and the
    /// push's cost — 1 when the keeper's move ends off `counted`, 0 on.
    edges: Vec<(Spot, Spot, Cell, Cell, u32)>,
    /// The spot count: cells times keeper sides.
    spots: usize,
}

/// Every reverse-push edge between spots over `blocks`: the same edge
/// relation the heuristic's distances relax — no min-cost filter and no
/// simple-path marking, both of which sample. Real routes revisit spots,
/// and take paths above the flat minimum when the walks between pushes
/// come out cheaper.
fn push_graph(board: &Board, blocks: &Blocks, counted: &[bool]) -> PushGraph {
    let neighbors = board.neighbors();
    let cells = neighbors.len();
    let spots = cells * SIDES;
    let mut edges: Vec<(Spot, Spot, Cell, Cell, u32)> = Vec::new();
    for at in 0..spots {
        let (cell, _side) = (at / SIDES, at % SIDES);
        for d in 0..4 {
            let to = neighbors[cell][d];
            if to == NONE {
                continue;
            }
            // The push moves the box cell -> to, so the edge back from
            // `to` points the other way.
            let back = OPPOSITE[d];
            for side_after in 0..SIDES {
                let after = usize::from(to) * SIDES + side_after;
                let Some((before, stand)) =
                    reverse_push(neighbors, Some(blocks), usize::from(to), side_after, back)
                else {
                    continue;
                };
                if before as usize != at {
                    continue;
                }
                edges.push((
                    at as Spot,
                    after as Spot,
                    cell as Cell,
                    stand,
                    u32::from(!counted[cell]),
                ));
            }
        }
    }
    PushGraph { edges, spots }
}

/// Counted-aware static walks between every pair of cells: per source a
/// 0-1 BFS whose steps end free on counted cells, walls aside. Boxes are
/// ignored, so each entry lower-bounds the keeper's true walk.
fn walk_matrix(board: &Board, counted: &[bool]) -> Vec<u8> {
    let neighbors = board.neighbors();
    let cells = neighbors.len();
    let mut w = vec![0u8; cells * cells];
    let mut steps = vec![u32::MAX; cells];
    for source in 0..cells {
        steps.fill(u32::MAX);
        steps[source] = 0;
        let mut queue = VecDeque::new();
        queue.push_back(source as Cell);
        while let Some(cell) = queue.pop_front() {
            let cell = cell as usize;
            for &next in &neighbors[cell] {
                if next == NONE {
                    continue;
                }
                let free = counted[next as usize];
                let reached = steps[cell] + u32::from(!free);
                if reached < steps[next as usize] {
                    steps[next as usize] = reached;
                    if free {
                        queue.push_front(next);
                    } else {
                        queue.push_back(next);
                    }
                }
            }
        }
        for (cell, &step) in steps.iter().enumerate() {
            w[source * cells + cell] = match step {
                u32::MAX => 0,
                step => step.min(u8::MAX as u32) as u8,
            };
        }
    }
    w
}

/// The LP's row map, the state-independent half the per-state evaluation
/// walks: which row each rhs entry lands on.
struct RowMaps {
    /// Per box, the conservation row of each spot; `usize::MAX` on the
    /// box's own goal cells, whose spots the attribution rows own.
    cons_row: Vec<Vec<usize>>,
    /// Per box, the attribution row of each of its group's columns.
    attr_row: Vec<Vec<usize>>,
    /// The assignment row of each goal column.
    goal_row: Vec<usize>,
    /// The player row of each cell; `usize::MAX` off the floor.
    player_row: Vec<usize>,
    /// The sink's demand row.
    dummy_row: usize,
}

/// The full-matrix walk LP at `state`'s rhs (the root's: the duals the
/// term carries come from one solve there), in sparse triplet form.
/// Column order: every edge for every box, the absorption variables,
/// then the transport arcs over the fixed source entries — the end cells
/// the graph can produce, plus one player row per floor cell. Row order:
/// conservation, attribution, assignment, end sources, player rows,
/// demands.
fn assemble_lp(
    hall: &Heuristic,
    board: &Board,
    state: &State,
    graph: &PushGraph,
    w: &[u8],
) -> (lp_solver::Lp, RowMaps) {
    let goals = hall.goals;
    let cells = board.tiles().len();
    let spot_count = graph.spots;
    let edges = graph.edges.len();
    let mut starts = [0; MAX_BOXES];
    for (spot, &cell) in starts.iter_mut().zip(&state.boxes[..goals]) {
        *spot = hall.spot(cell, state.player);
    }
    let mut x_at = vec![0usize; goals + 1];
    for b in 0..goals {
        x_at[b + 1] = x_at[b] + edges;
    }
    let mut a_at = vec![0usize; goals + 1];
    a_at[0] = x_at[goals];
    for b in 0..goals {
        a_at[b + 1] = a_at[b] + hall.groups[hall.group_of[b] as usize].len;
    }
    let mut end_cells: Vec<Cell> = graph.edges.iter().map(|&(_, _, end, ..)| end).collect();
    end_cells.sort();
    end_cells.dedup();
    let mut targets: Vec<Option<Cell>> = graph
        .edges
        .iter()
        .map(|&(_, _, _, stand, _)| Some(stand))
        .collect();
    targets.push(None);
    targets.sort();
    targets.dedup();
    let floor: Vec<usize> = (0..cells)
        .filter(|&cell| board.tiles()[cell] != WALL)
        .collect();
    let ns = end_cells.len() + floor.len();
    let nt = targets.len();
    let n = a_at[goals] + ns * nt;
    let mut cons_row: Vec<Vec<usize>> = Vec::with_capacity(goals);
    let mut attr_row: Vec<Vec<usize>> = Vec::with_capacity(goals);
    let mut row = 0usize;
    for b in 0..goals {
        let group = &hall.groups[hall.group_of[b] as usize];
        let mut per_spot = vec![usize::MAX; spot_count];
        for (spot, slot) in per_spot.iter_mut().enumerate() {
            let on_own_goal = (0..group.len)
                .any(|k| usize::from(hall.goal_cells[group.start + k]) == spot / SIDES);
            if !on_own_goal {
                *slot = row;
                row += 1;
            }
        }
        cons_row.push(per_spot);
        let mut per_k = Vec::with_capacity(group.len);
        for _ in 0..group.len {
            per_k.push(row);
            row += 1;
        }
        attr_row.push(per_k);
    }
    let mut goal_row = Vec::with_capacity(goals);
    for _ in 0..goals {
        goal_row.push(row);
        row += 1;
    }
    let mut end_row = vec![usize::MAX; cells];
    for &cell in &end_cells {
        end_row[usize::from(cell)] = row;
        row += 1;
    }
    let mut player_row = vec![usize::MAX; cells];
    for &cell in &floor {
        player_row[cell] = row;
        row += 1;
    }
    let mut demand_row = vec![usize::MAX; cells];
    let mut dummy_row = usize::MAX;
    for target in &targets {
        match target {
            None => dummy_row = row,
            Some(cell) => demand_row[usize::from(*cell)] = row,
        }
        row += 1;
    }
    let mut triplets: Vec<(usize, usize, f64)> = Vec::new();
    let mut rhs = vec![0.0; row];
    let mut c = vec![0.0; n];
    for b in 0..goals {
        let group = &hall.groups[hall.group_of[b] as usize];
        for (edge, &(from, to, end, stand, cost)) in graph.edges.iter().enumerate() {
            let col = x_at[b] + edge;
            match cons_row[b][from as usize] {
                usize::MAX => {}
                r => triplets.push((r, col, 1.0)),
            }
            match cons_row[b][to as usize] {
                usize::MAX => {}
                r => triplets.push((r, col, -1.0)),
            }
            for (k, &goal_cell) in hall.goal_cells[group.start..group.start + group.len]
                .iter()
                .enumerate()
            {
                let goal = usize::from(goal_cell);
                if from as usize / SIDES == goal {
                    triplets.push((attr_row[b][k], col, 1.0));
                }
                if to as usize / SIDES == goal {
                    triplets.push((attr_row[b][k], col, -1.0));
                }
            }
            triplets.push((end_row[usize::from(end)], col, -1.0));
            triplets.push((demand_row[usize::from(stand)], col, -1.0));
            c[col] = f64::from(cost);
        }
        for k in 0..group.len {
            let col = a_at[b] + k;
            triplets.push((attr_row[b][k], col, 1.0));
            triplets.push((goal_row[group.start + k], col, 1.0));
        }
    }
    let mut y = 0usize;
    for &cell in &end_cells {
        let source_row = end_row[usize::from(cell)];
        for &target in &targets {
            let col = a_at[goals] + y;
            triplets.push((source_row, col, 1.0));
            match target {
                None => {
                    triplets.push((dummy_row, col, 1.0));
                }
                Some(t) => {
                    c[col] = f64::from(w[usize::from(cell) * cells + usize::from(t)]);
                    triplets.push((demand_row[usize::from(t)], col, 1.0));
                }
            }
            y += 1;
        }
    }
    for &cell in &floor {
        let source_row = player_row[cell];
        for &target in &targets {
            let col = a_at[goals] + y;
            triplets.push((source_row, col, 1.0));
            match target {
                None => {
                    // The keeper reaches a stand before any push; only a
                    // push's end, the route's last, feeds the sink.
                    c[col] = 1e6;
                    triplets.push((dummy_row, col, 1.0));
                }
                Some(t) => {
                    c[col] = f64::from(w[cell * cells + usize::from(t)]);
                    triplets.push((demand_row[usize::from(t)], col, 1.0));
                }
            }
            y += 1;
        }
    }
    for b in 0..goals {
        let group = &hall.groups[hall.group_of[b] as usize];
        for (spot, &r) in cons_row[b].iter().enumerate() {
            if r != usize::MAX {
                rhs[r] = f64::from(u32::from(spot == starts[b] as usize));
            }
        }
        for k in 0..group.len {
            let goal = usize::from(hall.goal_cells[group.start + k]);
            rhs[attr_row[b][k]] = f64::from(u32::from(starts[b] as usize / SIDES == goal));
        }
    }
    for column in 0..goals {
        rhs[goal_row[column]] = 1.0;
    }
    for &cell in &floor {
        rhs[player_row[cell]] = f64::from(u32::from(cell == usize::from(state.player)));
    }
    rhs[dummy_row] = 1.0;
    (
        lp_from_triplets(row, n, triplets, rhs, c),
        RowMaps {
            cons_row,
            attr_row,
            goal_row,
            player_row,
            dummy_row,
        },
    )
}

/// Triplets, sorted by column then row, as the solver's CSC: each
/// column's entries land contiguous and row-ordered, the order a
/// row-major scan of the same matrix produces.
fn lp_from_triplets(
    m: usize,
    n: usize,
    mut triplets: Vec<(usize, usize, f64)>,
    b: Vec<f64>,
    c: Vec<f64>,
) -> lp_solver::Lp {
    // The triplets are (row, column, value); the sort is by column then
    // row, the (column, row) pair unique so the value needs no order.
    triplets.sort_unstable_by_key(|&(row, col, _)| (col, row));
    let mut starts = vec![0usize; n + 1];
    for &(_, col, _) in triplets.iter() {
        starts[col + 1] += 1;
    }
    for j in 0..n {
        starts[j + 1] += starts[j];
    }
    let mut index = Vec::with_capacity(triplets.len());
    let mut value = Vec::with_capacity(triplets.len());
    for &(row, _, v) in triplets.iter() {
        index.push(row);
        value.push(v);
    }
    lp_solver::Lp {
        m,
        n,
        starts,
        index,
        value,
        b,
        c,
    }
}

/// The L1-regularization LP over a solved LP's dual face: minimize
/// Σ|u| (as split variables p, n ≥ 0) subject to the transposed column
/// constraints (slacks turn the inequalities into rows) and the
/// optimal-value pin, set a hair below the objective so the face stays
/// feasible under the perturbed solve.
fn l1_regularization_lp(lp: &lp_solver::Lp, pin: f64) -> lp_solver::Lp {
    let m = lp.m;
    let n = lp.n;
    let rows = n + 1;
    let columns = 2 * m + n;
    let mut p_entries: Vec<Vec<(usize, f64)>> = vec![Vec::new(); m];
    let mut n_entries: Vec<Vec<(usize, f64)>> = vec![Vec::new(); m];
    for j in 0..n {
        for position in lp.starts[j]..lp.starts[j + 1] {
            let row = lp.index[position];
            let value = lp.value[position];
            p_entries[row].push((j, value));
            n_entries[row].push((j, -value));
        }
    }
    for i in 0..m {
        p_entries[i].push((n, lp.b[i]));
        n_entries[i].push((n, -lp.b[i]));
    }
    let mut starts = vec![0usize; columns + 1];
    for i in 0..m {
        starts[i + 1] = starts[i] + p_entries[i].len();
    }
    for i in 0..m {
        starts[m + i + 1] = starts[m + i] + n_entries[i].len();
    }
    for j in 0..n {
        starts[2 * m + j + 1] = starts[2 * m + j] + 1;
    }
    let mut index = Vec::with_capacity(starts[columns]);
    let mut value = Vec::with_capacity(starts[columns]);
    for entries in [&p_entries, &n_entries] {
        for column in entries.iter() {
            for &(row, v) in column {
                index.push(row);
                value.push(v);
            }
        }
    }
    for j in 0..n {
        index.push(j);
        value.push(1.0);
    }
    let mut rhs = Vec::with_capacity(rows);
    rhs.extend(lp.c.iter().copied());
    rhs.push(pin);
    let mut costs = vec![0.0; columns];
    for slot in &mut costs[..2 * m] {
        *slot = 1.0;
    }
    lp_solver::Lp {
        m: rows,
        n: columns,
        starts,
        index,
        value,
        b: rhs,
        c: costs,
    }
}

/// The walk term the room bound carries: the row map of the LP its
/// duals price, and the two dual vectors — the root solve's own, and
/// the L1-minimal dual over its optimal face. Sound per state by weak
/// duality: the dual feasible region never involves the rhs, so both
/// vectors bound every state's LP, which bounds its off-counted
/// remaining moves.
pub(crate) struct WalkTerm {
    maps: RowMaps,
    /// The root solve's duals.
    root: Vec<f64>,
    /// The L1-minimal dual over the root's optimal face.
    l1: Vec<f64>,
}

impl WalkTerm {
    /// Builds the term for `hall` over `counted`: the push graph, the
    /// walk matrix, the LP at `start`'s rhs, its primal solve, and the
    /// L1-regularized dual over the face that solve pinned. `None` when
    /// either solve fails — the search then runs without the term, its
    /// bound no stronger than the hall's own arm, never wrong.
    pub(crate) fn build(
        board: &Board,
        hall: &Heuristic,
        counted: &[bool],
        start: &State,
    ) -> Option<Self> {
        let blocks = &hall
            .keeper
            .as_ref()
            .expect("the hall heuristic keeps keeper sides")
            .blocks;
        let graph = push_graph(board, blocks, counted);
        let w = walk_matrix(board, counted);
        let (lp, maps) = assemble_lp(hall, board, start, &graph, &w);
        let solved = lp_solver::solve(&lp).ok()?;
        let pin = (solved.objective - 1e-4).max(0.0);
        let regularized = lp_solver::solve(&l1_regularization_lp(&lp, pin)).ok()?;
        let l1 = (0..lp.m)
            .map(|i| regularized.x[i] - regularized.x[lp.m + i])
            .collect();
        Some(Self {
            maps,
            root: solved.duals,
            l1,
        })
    }

    /// The term's value at `state`: the larger of the two carried duals'
    /// rhs dot products, `u·b(state)` — O(boxes) row lookups, each box's
    /// spot and the keeper's cell against the map. Zero where every box
    /// sits placed: the sink has no event to feed and the hyperplane is
    /// vacuous there.
    pub(crate) fn value(&self, hall: &Heuristic, state: &State) -> f64 {
        self.dual_value(&self.root, hall, state)
            .max(self.dual_value(&self.l1, hall, state))
    }

    /// One dual vector's value at `state`.
    fn dual_value(&self, u: &[f64], hall: &Heuristic, state: &State) -> f64 {
        let maps = &self.maps;
        let mut value = u[maps.dummy_row];
        for column in 0..hall.goals {
            value += u[maps.goal_row[column]];
        }
        let mut placed = true;
        for b in 0..hall.goals {
            let group = &hall.groups[hall.group_of[b] as usize];
            let spot = hall.spot(state.boxes[b], state.player) as usize;
            let cell = spot / SIDES;
            let mut on_goal = false;
            for k in 0..group.len {
                if usize::from(hall.goal_cells[group.start + k]) == cell {
                    on_goal = true;
                    value += u[maps.attr_row[b][k]];
                }
            }
            if on_goal {
                continue;
            }
            placed = false;
            let r = maps.cons_row[b][spot];
            if r != usize::MAX {
                value += u[r];
            }
        }
        if placed {
            return 0.0;
        }
        let player = maps.player_row[usize::from(state.player)];
        if player != usize::MAX {
            value += u[player];
        }
        value
    }

    /// The term's arm of S at `state`: the tables' sum plus the value,
    /// ceilinged. The oracle bounds the value within a millionth of the
    /// true off-counted remaining, and the total remaining is an
    /// integer, so the ceiling of the sum less that tolerance never
    /// passes the truth.
    pub(crate) fn arm(&self, hall: &Heuristic, state: &State, tables: u32) -> u32 {
        (f64::from(tables) + self.value(hall, state) - 1e-6).ceil() as u32
    }

    /// The heap bytes the maps and dual vectors hold.
    pub(crate) fn bytes(&self) -> u64 {
        let maps = &self.maps;
        let maps_bytes = |vs: &[Vec<usize>]| {
            vs.iter()
                .map(|v| v.capacity() * size_of::<usize>())
                .sum::<usize>()
        };
        (maps_bytes(&maps.cons_row)
            + maps_bytes(&maps.attr_row)
            + maps.goal_row.capacity() * size_of::<usize>()
            + maps.player_row.capacity() * size_of::<usize>()
            + (self.root.capacity() + self.l1.capacity()) * size_of::<f64>()) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{Lcg, explored_catalog, remaining_off};

    /// The admissibility oracle: per board and counted set, the term
    /// built in-process — graph, matrix, primal, L1 — must never claim
    /// more than the exact off-counted remaining at any state, the same
    /// bar the out-of-process sweep met before the solver existed. The
    /// hall here is the charged one the room bound keeps; the term reads
    /// only its structure — spots, groups, blocks — which the charge
    /// never touches.
    #[test]
    fn transfer_bounds_every_oracle_state() {
        let mut rng = Lcg(5);
        let mut checked = 0u64;
        for (id, board, states, edges) in explored_catalog() {
            for quarters in 1..4 {
                let counted: Vec<bool> = (0..board.tiles().len())
                    .map(|_| rng.below(4) < quarters)
                    .collect();
                let exact = remaining_off(board, states, edges, Some(&counted));
                let hall = Heuristic::hall(board, board.initial().player, &counted);
                let Some(term) = WalkTerm::build(board, &hall, &counted, &board.initial()) else {
                    panic!("{id} q{quarters}: the term failed to build");
                };
                for (from, state) in states.iter().enumerate() {
                    if exact[from] == u32::MAX {
                        continue;
                    }
                    let d = term.value(&hall, state);
                    assert!(
                        d <= f64::from(exact[from]) + 1e-6,
                        "{id}: dual {d:.3} > {} q{quarters} at {state:?}",
                        exact[from]
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 10_000, "the oracle checked only {checked} states");
    }
}
