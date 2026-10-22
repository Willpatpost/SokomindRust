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
//!
//! The build is resumable ([`WalkBuild`]): the two solves stepped a
//! caller-paid budget of pivot units at a time, because they run
//! minutes on the large boards and the search that pays for them must
//! return between batches to answer its deadline. The search defers
//! the term until its own expansions prove the solves can pay (see
//! `room_bound::WALK_TERM_AT`) and reserves the term's bytes before it
//! runs ([`bytes_ceiling`]), so a term attaching mid-search never
//! breaks the memory budget.
use super::*;
use crate::lp_solver;
use sokomind_core::WALL;

use std::path::{Path, PathBuf};

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
/// walks: which row each rhs entry lands on. Defaulted, so a resumable
/// build can hand its map to the term it finishes with.
#[derive(Default)]
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

/// One term as the cache holds it: the two carried dual vectors with
/// the primal objective they were certified against, and the pivots
/// each solve took for the line a hit replays. The file's name covers
/// every input that determines the LP, and its header repeats the LP's
/// dimensions, so a stale entry fails the load's checks rather than
/// the search.
struct CachedTerm {
    objective: f64,
    root: Vec<f64>,
    l1: Vec<f64>,
    primal_pivots: u64,
    l1_pivots: u64,
}

/// FNV-1a over the bytes: the digest's one round.
fn mix(hash: &mut u64, bytes: &[u8]) {
    for &byte in bytes {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// The digest behind the cache's file name: everything that
/// determines the LP hashed together — the fingerprint, the counted
/// cells, the goal count, the start's boxes, the keeper — so any drift
/// in the inputs lands on a different file rather than a stale hit.
fn term_digest(board: &Board, counted: &[bool], goals: usize, state: &State) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    mix(&mut hash, board.fingerprint().as_bytes());
    mix(&mut hash, &(counted.len() as u64).to_le_bytes());
    for &cell in counted {
        mix(&mut hash, &[u8::from(cell)]);
    }
    mix(&mut hash, &(goals as u64).to_le_bytes());
    for &cell in &state.boxes[..goals] {
        mix(&mut hash, &cell.to_le_bytes());
    }
    mix(&mut hash, &state.player.to_le_bytes());
    hash
}

/// The cache file's name: the fingerprint a person reads, the digest
/// the inputs wrote. The fingerprint's `:` is read as a `-` here:
/// Windows takes a `:` in a file name as an NTFS stream, so an entry
/// written under the fingerprint as-is could not be read back there.
/// The fingerprint string itself stays as it is, for the digest that
/// hashes it and the reference it matches.
fn cache_name(board: &Board, counted: &[bool], goals: usize, state: &State) -> String {
    format!(
        "{}.{:016x}.swt",
        board.fingerprint().replace(':', "-"),
        term_digest(board, counted, goals, state)
    )
}

/// One SWT1 record parsed from `bytes`, trusted only when the magic, the
/// LP's dimensions, the vectors' row counts, and the record's exact
/// length all agree.
fn parse_record(bytes: &[u8], m: usize, n: usize) -> Option<CachedTerm> {
    let mut cursor = 0;
    if take(bytes, &mut cursor, 4)? != &b"SWT1"[..] {
        return None;
    }
    let rows = read_u64(bytes, &mut cursor)?;
    let columns = read_u64(bytes, &mut cursor)?;
    if rows != m as u64 || columns != n as u64 {
        return None;
    }
    let objective = read_f64(bytes, &mut cursor)?;
    let root = read_vec(bytes, &mut cursor, m)?;
    let l1 = read_vec(bytes, &mut cursor, m)?;
    let primal_pivots = read_u64(bytes, &mut cursor)?;
    let l1_pivots = read_u64(bytes, &mut cursor)?;
    if cursor != bytes.len() {
        return None;
    }
    Some(CachedTerm {
        objective,
        root,
        l1,
        primal_pivots,
        l1_pivots,
    })
}

/// One little-endian run off the front, advancing the cursor.
fn take<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = cursor.checked_add(len)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(slice)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    Some(u64::from_le_bytes(take(bytes, cursor, 8)?.try_into().ok()?))
}

fn read_f64(bytes: &[u8], cursor: &mut usize) -> Option<f64> {
    Some(f64::from_le_bytes(take(bytes, cursor, 8)?.try_into().ok()?))
}

/// One dual vector, its row count pinned to the LP's.
fn read_vec(bytes: &[u8], cursor: &mut usize, m: usize) -> Option<Vec<f64>> {
    let len = read_u64(bytes, cursor)? as usize;
    if len != m {
        return None;
    }
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        values.push(read_f64(bytes, cursor)?);
    }
    Some(values)
}

// The record a binding provided for this thread's next walk-term build:
// the cache name it was generated under and its SWT1 bytes. wasm has no
// environment or filesystem, so a record a web worker fetched is the
// browser's only dual cache; the build takes it through the same name,
// dimension and y·b checks a directory entry passes, and a mismatched
// record is dropped, never trusted. One record serves one build: the
// take is consuming, and a build that never arms leaves it for the next
// one to refuse by name.
thread_local! {
    static INJECTED: std::cell::RefCell<Option<(String, Vec<u8>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Provides a walk-term dual-cache record for this thread's next search:
/// the manifest's `name` and the record's `bytes` (see [`INJECTED`]).
pub(crate) fn inject(name: String, bytes: Vec<u8>) {
    INJECTED.with(|cell| *cell.borrow_mut() = Some((name, bytes)));
}

/// Takes the provided record, if one stands.
fn take_injected() -> Option<(String, Vec<u8>)> {
    INJECTED.with(|cell| cell.borrow_mut().take())
}

/// The native halves of the term's iteration support: the dual cache
/// and the build's log line, both off unless their environment
/// variables name them. The cache is best effort by construction — a
/// directory that cannot be written, or an entry that cannot be
/// parsed or does not pass its checks, leaves the build solving as it
/// always did.
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::CachedTerm;
    use std::path::{Path, PathBuf};

    /// The cache directory, when `SOKOMIND_DUAL_CACHE` names one.
    pub(super) fn cache_dir() -> Option<PathBuf> {
        std::env::var_os("SOKOMIND_DUAL_CACHE").map(PathBuf::from)
    }

    /// Whether `SOKOMIND_WALK_TERM_LOG` asks for the build's line.
    pub(super) fn log_enabled() -> bool {
        std::env::var_os("SOKOMIND_WALK_TERM_LOG").is_some()
    }

    /// Reads one entry, trusting it only as [`super::parse_record`] does.
    pub(super) fn load(dir: &Path, name: &str, m: usize, n: usize) -> Option<CachedTerm> {
        let bytes = std::fs::read(dir.join(name)).ok()?;
        super::parse_record(&bytes, m, n)
    }

    /// One dual vector's bytes, for the record `store` writes.
    fn push_vec(bytes: &mut Vec<u8>, values: &[f64]) {
        bytes.extend_from_slice(&(values.len() as u64).to_le_bytes());
        for &value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// Writes one entry through a temporary and a rename, so a reader
    /// never sees a torn file and a write that cannot happen never
    /// fails the search.
    pub(super) fn store(dir: &Path, name: &str, m: usize, n: usize, term: &CachedTerm) {
        let _ = std::fs::create_dir_all(dir);
        let mut bytes = Vec::with_capacity(56 + 16 * (term.root.len() + term.l1.len()));
        bytes.extend_from_slice(b"SWT1");
        bytes.extend_from_slice(&(m as u64).to_le_bytes());
        bytes.extend_from_slice(&(n as u64).to_le_bytes());
        bytes.extend_from_slice(&term.objective.to_le_bytes());
        push_vec(&mut bytes, &term.root);
        push_vec(&mut bytes, &term.l1);
        bytes.extend_from_slice(&term.primal_pivots.to_le_bytes());
        bytes.extend_from_slice(&term.l1_pivots.to_le_bytes());
        let temp = dir.join(format!("{name}.{}.tmp", std::process::id()));
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(&temp, dir.join(name));
        }
    }
}

/// The wasm twins: no environment to read and no filesystem to write,
/// so no cache and no log — the build behaves exactly as it did before
/// either existed.
#[cfg(target_arch = "wasm32")]
mod native {
    use super::CachedTerm;
    use std::path::{Path, PathBuf};

    pub(super) fn cache_dir() -> Option<PathBuf> {
        None
    }

    pub(super) fn log_enabled() -> bool {
        false
    }

    pub(super) fn load(_dir: &Path, _name: &str, _m: usize, _n: usize) -> Option<CachedTerm> {
        None
    }

    pub(super) fn store(_dir: &Path, _name: &str, _m: usize, _n: usize, _term: &CachedTerm) {}
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
    /// bound no stronger than the hall's own arm, never wrong. The
    /// environment's `SOKOMIND_DUAL_CACHE` carries the solves between
    /// builds and `SOKOMIND_WALK_TERM_LOG` reports each one's pivots.
    pub(crate) fn build(
        board: &Board,
        hall: &Heuristic,
        counted: &[bool],
        start: &State,
    ) -> Option<Self> {
        Self::build_with(board, hall, counted, start, native::cache_dir().as_deref())
            .map(|(term, _)| term)
    }

    /// The build against an explicit cache directory, returning the
    /// term and whether an entry supplied its duals — the pair a test
    /// watches while the product path above only ever asks the
    /// environment. The assembly always runs: its row map is the term's
    /// own evaluation structure, and its rhs re-prices a stored root
    /// dual against the stored objective — the solver's own `y·b`
    /// guard — before the file is trusted, so the two solves, and on
    /// Grand Hall the better part of two hours, are the only work a
    /// hit skips.
    pub(crate) fn build_with(
        board: &Board,
        hall: &Heuristic,
        counted: &[bool],
        start: &State,
        cache: Option<&Path>,
    ) -> Option<(Self, bool)> {
        let mut build = WalkBuild::new(board, hall, counted, start, cache)?;
        let cached = matches!(build.stage.as_ref(), Some(WalkStage::Cached { .. }));
        let mut units = usize::MAX;
        match build.step(&mut units) {
            Some(Some(term)) => Some((term, cached)),
            _ => None,
        }
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

/// The most heap bytes a walk term on `board` with `goals` boxes can
/// hold: the row map's per-box spot rows and group rows, the fixed
/// rows, and the two dual vectors at the LP's largest row count. The
/// LP is not assembled when this is asked — the search reserves the
/// ceiling before it runs, so a term attaching mid-search never breaks
/// the memory budget (see `engine.rs`) — so the row count is bounded
/// from the board's shape rather than counted: a conservation row per
/// box and spot, an attribution row per box and goal, and the goal,
/// end, player and demand rows over the cells.
pub(crate) fn bytes_ceiling(board: &Board, goals: usize) -> u64 {
    let cells = board.tiles().len() as u64;
    let boxes = goals as u64;
    let spots = cells * SIDES as u64;
    let rows = boxes * (spots + boxes) + boxes + 3 * cells + 1;
    let maps = (boxes * (spots + boxes) + boxes + cells) * size_of::<usize>() as u64;
    maps + 2 * rows * size_of::<f64>() as u64
}

/// The term's resumable build: the LP assembled once, then its two
/// solves stepped a caller-paid budget of pivot units at a time, so
/// the search that pays for them can return between batches and answer
/// its deadline — a solve that runs minutes never blocks one `advance`
/// call (see `engine.rs`). The assembly and the solves' transient
/// buffers ride above the memory budget as the one-shot build's always
/// did; the built term's bytes land against the ceiling the search
/// reserved before it ran (see `bytes_ceiling`). A cache entry the
/// `y·b` guard passes starts the build done.
pub(crate) struct WalkBuild {
    /// The walk LP at the start's rhs, its row map beside it.
    lp: lp_solver::Lp,
    maps: RowMaps,
    /// The cache's file name for the term, the directory it lives in,
    /// and the fingerprint its log lines name.
    name: String,
    cache: Option<PathBuf>,
    fingerprint: String,
    /// The solve in progress; `None` once the term left.
    stage: Option<WalkStage>,
}

/// Where the build is: the root solve, the L1 solve it pinned, or
/// duals a cache entry supplied.
enum WalkStage {
    /// The root solve at the start's rhs.
    Root(lp_solver::SolveBuild),
    /// The L1-regularized solve over the root's pinned optimal face,
    /// carrying the root solve's duals, pivots and objective for the
    /// term and its log line.
    L1 {
        solve: lp_solver::SolveBuild,
        lp: lp_solver::Lp,
        root: Vec<f64>,
        root_pivots: u64,
        objective: f64,
    },
    /// Duals a cache entry supplied; the term is assembled with the
    /// build's row map on handover.
    Cached { root: Vec<f64>, l1: Vec<f64> },
}

impl WalkBuild {
    /// The build the search starts, its solves carried by the
    /// environment's dual cache where one is armed (see
    /// `SOKOMIND_DUAL_CACHE`).
    pub(crate) fn start(
        board: &Board,
        hall: &Heuristic,
        counted: &[bool],
        start: &State,
    ) -> Option<Self> {
        Self::new(board, hall, counted, start, native::cache_dir().as_deref())
    }

    /// Assembles the LP over `hall` and `counted` at `start`'s rhs and
    /// starts its root solve, or holds the duals a passing cache entry
    /// supplied. `None` when the root solve cannot start — the caller
    /// runs without the term and never tries again.
    pub(crate) fn new(
        board: &Board,
        hall: &Heuristic,
        counted: &[bool],
        start: &State,
        cache: Option<&Path>,
    ) -> Option<Self> {
        let blocks = &hall
            .keeper
            .as_ref()
            .expect("the hall heuristic keeps keeper sides")
            .blocks;
        let graph = push_graph(board, blocks, counted);
        let w = walk_matrix(board, counted);
        let (lp, maps) = assemble_lp(hall, board, start, &graph, &w);
        let name = cache_name(board, counted, hall.goals, start);
        let cache = cache.map(Path::to_path_buf);
        // A provided record comes first: it names this very build, where a
        // directory's entry only might. A mismatched record falls through
        // to the solves, never trusted.
        let entry = take_injected()
            .filter(|(provided, _)| provided == &name)
            .and_then(|(_, bytes)| parse_record(&bytes, lp.m, lp.n))
            .or_else(|| native::load(cache.as_deref()?, &name, lp.m, lp.n));
        if let Some(entry) = entry {
            let value: f64 = entry.root.iter().zip(&lp.b).map(|(&u, &b)| u * b).sum();
            if (value - entry.objective).abs() <= 1e-4 * (1.0 + entry.objective.abs()) {
                if native::log_enabled() {
                    eprintln!(
                        "walk term {}: cache hit, {} primal and {} l1 pivots, objective {:.3}",
                        board.fingerprint(),
                        entry.primal_pivots,
                        entry.l1_pivots,
                        entry.objective
                    );
                }
                return Some(Self {
                    stage: Some(WalkStage::Cached {
                        root: entry.root,
                        l1: entry.l1,
                    }),
                    lp,
                    maps,
                    name,
                    cache,
                    fingerprint: board.fingerprint().to_owned(),
                });
            }
        }
        let solve = lp_solver::SolveBuild::new(&lp).ok()?;
        Some(Self {
            stage: Some(WalkStage::Root(solve)),
            lp,
            maps,
            name,
            cache,
            fingerprint: board.fingerprint().to_owned(),
        })
    }

    /// Runs up to `units` pivot units: `None` while the build has more
    /// to do, then the term — `Some(None)` when a solve failed, the
    /// bound standing on the tables and the hall part alone, never
    /// wrong. Done, it must not be stepped again.
    pub(crate) fn step(&mut self, units: &mut usize) -> Option<Option<WalkTerm>> {
        match self.stage.take() {
            None => Some(None),
            Some(WalkStage::Cached { root, l1 }) => Some(Some(WalkTerm {
                maps: std::mem::take(&mut self.maps),
                root,
                l1,
            })),
            Some(WalkStage::Root(mut solve)) => {
                let done = match solve.step(&self.lp, units) {
                    Ok(done) => done,
                    Err(_) => return Some(None),
                };
                if !done {
                    self.stage = Some(WalkStage::Root(solve));
                    return None;
                }
                let Ok(solved) = solve.finish(&self.lp) else {
                    return Some(None);
                };
                let pin = (solved.objective - 1e-4).max(0.0);
                let lp = l1_regularization_lp(&self.lp, pin);
                let Some(solve) = lp_solver::SolveBuild::new(&lp).ok() else {
                    return Some(None);
                };
                self.stage = Some(WalkStage::L1 {
                    solve,
                    lp,
                    root: solved.duals,
                    root_pivots: solved.pivots,
                    objective: solved.objective,
                });
                // The units the root solve left go on to the L1 solve.
                self.step(units)
            }
            Some(WalkStage::L1 {
                mut solve,
                lp,
                root,
                root_pivots,
                objective,
            }) => {
                let done = match solve.step(&lp, units) {
                    Ok(done) => done,
                    Err(_) => return Some(None),
                };
                if !done {
                    self.stage = Some(WalkStage::L1 {
                        solve,
                        lp,
                        root,
                        root_pivots,
                        objective,
                    });
                    return None;
                }
                let Ok(solved) = solve.finish(&lp) else {
                    return Some(None);
                };
                let l1: Vec<f64> = (0..self.lp.m)
                    .map(|i| solved.x[i] - solved.x[self.lp.m + i])
                    .collect();
                if native::log_enabled() {
                    eprintln!(
                        "walk term {}: {} primal and {} l1 pivots, objective {:.3}",
                        self.fingerprint, root_pivots, solved.pivots, objective
                    );
                }
                if let Some(dir) = &self.cache {
                    native::store(
                        dir,
                        &self.name,
                        self.lp.m,
                        self.lp.n,
                        &CachedTerm {
                            objective,
                            root: root.clone(),
                            l1: l1.clone(),
                            primal_pivots: root_pivots,
                            l1_pivots: solved.pivots,
                        },
                    );
                }
                Some(Some(WalkTerm {
                    maps: std::mem::take(&mut self.maps),
                    root,
                    l1,
                }))
            }
        }
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

    /// The cache round-trip: a second build over the same scratch
    /// directory must hit — both solves skipped — and price every
    /// state exactly as the fresh build did, the file holding the
    /// same vectors the deterministic solver produced the first time.
    #[test]
    fn the_second_build_hits_the_cache() {
        let (_id, board, states, _edges) = explored_catalog()
            .iter()
            .next()
            .expect("the catalog holds a board");
        let mut rng = Lcg(7);
        let counted: Vec<bool> = (0..board.tiles().len()).map(|_| rng.below(4) < 2).collect();
        let hall = Heuristic::hall(board, board.initial().player, &counted);
        let dir = std::env::temp_dir().join(format!("sokomind-walk-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (fresh, hit) =
            WalkTerm::build_with(board, &hall, &counted, &board.initial(), Some(&dir))
                .expect("the fresh build solves");
        assert!(!hit, "an empty directory holds no entry");
        let (cached, hit) =
            WalkTerm::build_with(board, &hall, &counted, &board.initial(), Some(&dir))
                .expect("the cached build loads");
        assert!(hit, "the stored entry must be read back");
        for state in states {
            assert!(
                (fresh.value(&hall, state) - cached.value(&hall, state)).abs() < 1e-9,
                "the cached duals price {state:?} differently"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A record provided through the binding's channel behaves as a
    /// directory entry does: the build whose name it carries takes it
    /// without solving, and one whose name it does not refuses it, the
    /// solves running fresh. The browser's only dual cache rides this
    /// channel, wasm having no environment or filesystem to read.
    #[test]
    fn a_provided_record_hits_only_its_own_name() {
        let (_id, board, states, _edges) = explored_catalog()
            .iter()
            .next()
            .expect("the catalog holds a board");
        let mut rng = Lcg(11);
        let counted: Vec<bool> = (0..board.tiles().len()).map(|_| rng.below(4) < 2).collect();
        let hall = Heuristic::hall(board, board.initial().player, &counted);
        let dir =
            std::env::temp_dir().join(format!("sokomind-walk-provided-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The fresh build stores its record; a file is the one shape the
        // store writes, so the channel's bytes come from it.
        let (fresh, _) = WalkTerm::build_with(board, &hall, &counted, &board.initial(), Some(&dir))
            .expect("the fresh build solves");
        let name = std::fs::read_dir(&dir)
            .expect("the store wrote its record")
            .next()
            .and_then(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .expect("one record file");
        let bytes = std::fs::read(dir.join(&name)).expect("the record reads back");
        let _ = std::fs::remove_dir_all(&dir);
        // A record under another name is refused: the build solves fresh.
        inject(
            "another-board.0000000000000000.swt".to_owned(),
            bytes.clone(),
        );
        let (_, hit) = WalkTerm::build_with(board, &hall, &counted, &board.initial(), None)
            .expect("the mismatched build solves");
        assert!(!hit, "a record under another name must be refused");
        // The matching name takes it: the duals price identically.
        inject(name, bytes);
        let (provided, hit) = WalkTerm::build_with(board, &hall, &counted, &board.initial(), None)
            .expect("the provided build loads");
        assert!(hit, "the record the name matches must be taken");
        for state in states {
            assert!(
                (fresh.value(&hall, state) - provided.value(&hall, state)).abs() < 1e-9,
                "the provided duals price {state:?} differently"
            );
        }
    }
}
