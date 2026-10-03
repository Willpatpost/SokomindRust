//! The boxes a stage may move and the cells its plan needs: ports the
//! probe's `movers`, `walk_blockers` and `walk_path` (p4b.rs 1004-1123 and
//! 235-251) and the need marks of a stage start (1758-1767).
//!
//! The focus is the first `FOCUS[rung]` distinct boxes of the ranked
//! candidates, and the used candidates are those taken until the focus is
//! full: always a prefix of the list. The movers start as the focus. Boxes
//! are then appended in the probe's order, each only on its first
//! occurrence:
//! 1. B1: the boxes on each used candidate's path cells, then on its
//!    stands.
//! 2. The walk blockers: the boxes on some shortest keeper walk a used
//!    candidate needs, to its first stand and around the box at each turn,
//!    where the walk is blocked with every box in place.
//! 3. B2: for each box of steps 1 and 2, the box on the stand of each push
//!    that would move it to a cell that is not dead for it.
//! 4. The path neighbors: the boxes beside each used candidate's path
//!    cells.
//!
//! Below the last rung the list is then cut to `MAX_MOVE` boxes. The last
//! rung moves every box, so it skips steps 1 to 4. The need marks are the
//! used candidates' path cells and stands, and one shortest keeper walk on
//! the empty board from the root's keeper to each first stand.
//!
//! The work runs in units, each `O(cells)`: one `Path` unit per used
//! candidate, then the walk blockers' legs one `Leg` unit at a time, then
//! `Merge`. Steps 1 to 4 add boxes in the probe's order across the units,
//! and once the list holds `MAX_MOVE` boxes no later unit can add one that
//! survives the cut.
//!
//! The probe builds each step's list in full and cuts once at the end.
//! Here every append stops at `MAX_MOVE` instead. The probe only ever
//! appends, so its first `MAX_MOVE` boxes are fixed as soon as it has that
//! many, and a capped list equals its prefix at every point: the cut gives
//! the same movers, and the legs left when the list fills can be dropped.
//! The walk blockers go straight into `mv`, not into a list of their own
//! first; a box joins either way exactly on its first occurrence that is
//! not already a mover. The path neighbors do need their own list, `near`,
//! since B2, which comes before them, runs only at `Merge`.
use super::{Candidate, FOCUS, FRAMES, MAX_MOVE, Scratch, Tools, need_row, rank, set_bit, split};
use crate::reach::Reach;
use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, State};

/// One unit of choosing a stage's movers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MoversUnit {
    /// The need marks of used candidate `u`, and below the last rung its
    /// B1 boxes and path neighbors.
    Path(u8),
    /// Leg `k` of used candidate `u`'s walk: leg 0 runs from the root's
    /// keeper to the first stand, and leg `k >= 1` from the keeper's cell
    /// after push `k - 1` to the stand of push `k`, around the box on the
    /// path's cell `k - 1`.
    Leg { u: u8, k: u16 },
    /// B2, the path neighbors, the cut and the frozen cells.
    Merge,
}

/// The movers of one stage start, built over its units. Slots fit `u8`
/// and a slot set fits a `u32` mask, since `MAX_BOXES` is 32.
pub(super) struct Movers {
    /// The ladder rung: 0 and 1 cap the movers at `MAX_MOVE`, 2 moves every
    /// box.
    rung: u8,
    /// The frame whose row of `scratch.need` gets the need marks.
    top: u8,
    /// `mv[..focus_len]` is the focus.
    focus_len: u8,
    /// The used candidates are `scratch.candidates[..used_len]`.
    used_len: u8,
    /// The length of the path in the pool's path region, which the last
    /// `Path` unit or first `Leg` unit of a candidate rebuilt. Its stands
    /// follow from `scratch.via`.
    path_len: u16,
    /// The movers so far, in the probe's order: the focus, then the boxes
    /// steps 1 to 4 append.
    mv: [u8; MAX_BOXES],
    /// The length of `mv`.
    mv_len: u8,
    /// Bit `i` is set when slot `i` is in `mv`.
    mv_mask: u32,
    /// The path neighbors, each slot once, in the probe's order: used
    /// candidates in order, each path's cells in order, each cell's
    /// neighbors in direction order. Merge appends those not yet in `mv`.
    near: [u8; MAX_BOXES],
    /// The length of `near`.
    near_len: u8,
    /// Bit `i` is set when slot `i` is in `near`.
    near_mask: u32,
}

impl Movers {
    /// The focus and the used candidates of `candidates`, the frame root's
    /// ranked and filtered list, at rung `rung`, as the probe takes them:
    /// the candidates in order, each adding its slot to the focus if new,
    /// until a new slot would make the focus exceed `FOCUS[rung]`. The need
    /// marks go to frame `top`.
    pub(super) fn new(candidates: &[Candidate], rung: usize, top: usize) -> Self {
        debug_assert!(rung < FOCUS.len() && top < FRAMES);
        let mut movers = Self {
            rung: rung as u8,
            top: top as u8,
            focus_len: 0,
            used_len: 0,
            path_len: 0,
            mv: [0; MAX_BOXES],
            mv_len: 0,
            mv_mask: 0,
            near: [0; MAX_BOXES],
            near_len: 0,
            near_mask: 0,
        };
        // At most MAX_CANDIDATES (96) candidates are used, so the count
        // fits u8, and the focus holds distinct slots, at most MAX_BOXES.
        for c in candidates {
            if (movers.mv_mask >> c.slot) & 1 == 0 {
                if usize::from(movers.mv_len) == FOCUS[rung] {
                    break;
                }
                movers.push(c.slot);
            }
            movers.used_len += 1;
        }
        movers.focus_len = movers.mv_len;
        movers
    }

    /// Runs one unit, `unit`, of the movers of `root`, whose candidates are
    /// in `scratch.candidates`. Each unit first refreshes Deadlock's
    /// occupancy to `root`. Marks only set bits in frame `top`'s row of
    /// `scratch.need`; the ladder clears a frame's row when it creates the
    /// frame. The units run from `Path(0)` until one gives `None`.
    ///
    /// - `Path(u)`: reruns candidate `u`'s push search and path, with
    ///   `rank::push_bfs` and `rank::rebuild_path`. The search is
    ///   deterministic, so the path is the one Rank found. Marks the path's
    ///   cells, its stands and [`walk_path`] from the root's keeper to the
    ///   first stand. Below the last rung, appends B1 and records the path
    ///   neighbors in `near`. With no used candidate, `Path(0)` does
    ///   nothing. Gives `Path(u + 1)`. After the last used candidate it
    ///   gives `Merge` at the last rung, and below it `Leg { u: 0, k: 0 }`,
    ///   or `Merge` when there is no used candidate or `mv` already holds
    ///   `MAX_MOVE` boxes.
    /// - `Leg { u, k }`: at `k = 0`, first reruns `u`'s push search and
    ///   path, which later `Path` units overwrote. Leg 0 is (the root's
    ///   keeper, the first stand, the box's cell). Leg `k >= 1` is (before,
    ///   the stand of push `k`, the path's cell `k - 1`), where before is
    ///   the box's cell for `k = 1` and the path's cell `k - 2` after. The
    ///   probe forms leg 0 always and leg `k >= 1` only when its stand
    ///   differs from before, and a `Leg` unit only ever names a formed leg.
    ///   A leg (from, to, at) is skipped when from or to is `at`, or when
    ///   `reach.fill_from` from `from` reaches `to` with the root's other
    ///   boxes, minus those on from and to, plus `at` blocked. Otherwise dp
    ///   is the flood from `from` and ds the flood from `to`, each with only
    ///   `at` blocked: dp in the keeper flood, and ds in the pool's aux with
    ///   its queue region as scratch. When dp reaches `to`, each cell `x` in
    ///   index order with `dp[x] + ds[x] == dp[to]` appends its box, if it
    ///   holds a box other than `u`'s. Gives the next formed leg, or `Merge`
    ///   after the last leg of the last used candidate or as soon as `mv`
    ///   holds `MAX_MOVE` boxes.
    /// - `Merge`: below the last rung, appends B2 over the boxes
    ///   `mv[focus_len..]` holds at entry: for box `b` and direction `d`,
    ///   when `y`, `b`'s neighbor in direction `d`, and `s`, the one in the
    ///   opposite direction, are both on the board and `y` is not dead for
    ///   `b`, the box on `s`. Then appends `near` and cuts `mv` to
    ///   `MAX_MOVE`. At the last rung `mv` becomes every box. Sets
    ///   `scratch.frozen` to the cells of the root's boxes not in `mv`, and
    ///   gives `None`.
    pub(super) fn run(
        &mut self,
        tools: &mut Tools<'_>,
        scratch: &mut Scratch,
        root: &State,
        unit: MoversUnit,
    ) -> Option<MoversUnit> {
        let boxes = tools.board.labels().len();
        tools.deadlock.refresh(&root.boxes[..boxes]);
        match unit {
            MoversUnit::Path(u) => {
                if u < self.used_len {
                    self.path(tools, scratch, root, usize::from(u));
                }
                let next = if u + 1 < self.used_len {
                    MoversUnit::Path(u + 1)
                } else if self.capped() {
                    self.leg_start(0)
                } else {
                    MoversUnit::Merge
                };
                Some(next)
            }
            MoversUnit::Leg { u, k } => Some(self.leg(tools, scratch, root, u, k)),
            MoversUnit::Merge => {
                self.merge(tools, scratch, root);
                None
            }
        }
    }

    /// The slots a stage may move, in the probe's order, once `Merge` has
    /// run.
    pub(super) fn movers(&self) -> &[u8] {
        &self.mv[..usize::from(self.mv_len)]
    }

    /// Whether the rung caps the movers at `MAX_MOVE`: every rung but the
    /// last, which moves every box.
    fn capped(&self) -> bool {
        usize::from(self.rung) < FOCUS.len() - 1
    }

    /// The `Path(u)` unit of [`Movers::run`], for a used candidate `u`.
    fn path(&mut self, tools: &mut Tools<'_>, scratch: &mut Scratch, root: &State, u: usize) {
        self.rebuild(tools, scratch, root, u);
        let board = tools.board;
        let cells = board.tiles().len();
        let c = scratch.candidates[u];
        let (_, _, _, path) = split(&mut scratch.pool, cells);
        let path = &path[..usize::from(self.path_len)];
        let via = &scratch.via;
        let row = need_row(&mut scratch.need, cells, usize::from(self.top));
        for &x in path {
            set_bit(row, x);
            set_bit(row, rank::stand(board, via, x));
        }
        let (player, first) = (root.player, c.first_stand);
        walk_path(board, tools.reach, player, first, |x| set_bit(row, x));
        if !self.capped() {
            return;
        }
        // B1 takes the cells, then the stands, as the probe chains them.
        let deadlock = &*tools.deadlock;
        for &x in path {
            self.add(deadlock.at(x));
        }
        for &x in path {
            self.add(deadlock.at(rank::stand(board, via, x)));
        }
        for &x in path {
            for &y in &board.neighbors()[usize::from(x)] {
                self.note(deadlock.at(y));
            }
        }
    }

    /// Reruns used candidate `u`'s push search into `scratch.via` and its
    /// path into the pool's path region, and records the path's length.
    /// The search depends only on the box's slot and cell, so it rebuilds
    /// the path and stands Rank found for the candidate.
    fn rebuild(&mut self, tools: &Tools<'_>, scratch: &mut Scratch, root: &State, u: usize) {
        let (board, heuristic) = (tools.board, tools.heuristic);
        let c = scratch.candidates[u];
        let slot = usize::from(c.slot);
        let x = root.boxes[slot];
        let (dist, queue, _, path) = split(&mut scratch.pool, board.tiles().len());
        rank::push_bfs(board, heuristic, slot, x, dist, queue, &mut scratch.via);
        let len = rank::rebuild_path(board, x, c.target, &scratch.via, path);
        debug_assert!(len > 0 && rank::stand(board, &scratch.via, path[0]) == c.first_stand);
        self.path_len = len as u16;
    }

    /// The `Leg { u, k }` unit of [`Movers::run`]: walk blockers of one
    /// formed leg, then the next unit.
    fn leg(
        &mut self,
        tools: &mut Tools<'_>,
        scratch: &mut Scratch,
        root: &State,
        u: u8,
        k: u16,
    ) -> MoversUnit {
        let (board, i, k) = (tools.board, usize::from(u), usize::from(k));
        if k == 0 {
            self.rebuild(tools, scratch, root, i);
        }
        let c = scratch.candidates[i];
        let own = root.boxes[usize::from(c.slot)];
        let (_, queue, aux, path) = split(&mut scratch.pool, board.tiles().len());
        let path = &path[..usize::from(self.path_len)];
        let via = &scratch.via;
        let leg = if k == 0 {
            (root.player, c.first_stand, own)
        } else {
            let to = rank::stand(board, via, path[k]);
            (keeper_after(path, own, k), to, path[k - 1])
        };
        self.blockers(tools, root, usize::from(c.slot), leg, aux, queue);
        if usize::from(self.mv_len) >= MAX_MOVE {
            return MoversUnit::Merge;
        }
        match next_formed(board, via, path, own, k + 1) {
            Some(j) => MoversUnit::Leg { u, k: j as u16 },
            None => self.leg_start(u + 1),
        }
    }

    /// The first unit of used candidate `u`'s legs: its leg 0, which the
    /// probe always forms, or `Merge` past the last used candidate or once
    /// `mv` is full, since no later leg can add a box that survives the
    /// cut.
    fn leg_start(&self, u: u8) -> MoversUnit {
        if usize::from(self.mv_len) >= MAX_MOVE || u >= self.used_len {
            MoversUnit::Merge
        } else {
            MoversUnit::Leg { u, k: 0 }
        }
    }

    /// Appends the walk blockers of leg `(from, to, at)` of the candidate
    /// whose box is in `slot`, as [`Movers::run`] describes. The keeper
    /// flood ends as dp, and `aux` as ds, with `queue` as its scratch. The
    /// box in `slot` is never appended, since it is in the focus.
    fn blockers(
        &mut self,
        tools: &mut Tools<'_>,
        root: &State,
        slot: usize,
        (from, to, at): (Cell, Cell, Cell),
        aux: &mut [u16],
        queue: &mut [u16],
    ) {
        if from == at || to == at {
            return;
        }
        let board = tools.board;
        // The other boxes, but a box on the leg's ends is B1's (on a path
        // cell or stand), not a blocker, and the keeper starts on `from`.
        let mut blocked = [NONE; MAX_BOXES + 1];
        let mut len = 0;
        for (j, &b) in root.boxes[..board.labels().len()].iter().enumerate() {
            if j != slot && b != from && b != to {
                blocked[len] = b;
                len += 1;
            }
        }
        blocked[len] = at;
        tools.reach.fill_from(board, from, &blocked[..=len]);
        if tools.reach.distance(to) != NONE {
            return;
        }
        tools.reach.fill_from(board, from, &[at]);
        let span = tools.reach.distance(to);
        if span == NONE {
            return;
        }
        flood(board, to, at, aux, queue);
        let (reach, deadlock) = (&*tools.reach, &*tools.deadlock);
        // Distances stay below MAX_CELLS (4096), so the sum fits u16.
        for (x, &s) in aux.iter().enumerate() {
            let p = reach.distance(x as Cell);
            if p != NONE && s != NONE && p + s == span {
                self.add(deadlock.at(x as Cell));
            }
        }
    }

    /// The `Merge` unit of [`Movers::run`].
    fn merge(&mut self, tools: &Tools<'_>, scratch: &mut Scratch, root: &State) {
        let board = tools.board;
        let boxes = board.labels().len();
        if self.capped() {
            // B2 runs over a snapshot of steps 1 and 2. When `mv` is full,
            // the snapshot may lack some of the probe's boxes, but then
            // nothing B2 adds would survive the cut anyway.
            let (b1, b1_len) = (self.mv, usize::from(self.mv_len));
            for &b in &b1[usize::from(self.focus_len)..b1_len] {
                let b = usize::from(b);
                let around = board.neighbors()[usize::from(root.boxes[b])];
                for (d, &y) in around.iter().enumerate() {
                    let s = around[OPPOSITE[d]];
                    if y != NONE && s != NONE && !tools.heuristic.dead(b, y) {
                        self.add(tools.deadlock.at(s));
                    }
                }
            }
            let near = self.near;
            for &j in &near[..usize::from(self.near_len)] {
                self.add(Some(usize::from(j)));
            }
        } else {
            for (j, m) in self.mv[..boxes].iter_mut().enumerate() {
                *m = j as u8;
            }
            self.mv_len = boxes as u8;
            self.mv_mask = u32::MAX >> (MAX_BOXES - boxes);
        }
        scratch.frozen.fill(0);
        for (j, &b) in root.boxes[..boxes].iter().enumerate() {
            if (self.mv_mask >> j) & 1 == 0 {
                set_bit(&mut scratch.frozen, b);
            }
        }
    }

    /// Appends `slot`, which `mv` lacks.
    fn push(&mut self, slot: u8) {
        self.mv[usize::from(self.mv_len)] = slot;
        self.mv_len += 1;
        self.mv_mask |= 1 << slot;
    }

    /// Appends the box `slot`, from `deadlock.at`, unless there is none, it
    /// is already a mover or `mv` holds `MAX_MOVE` boxes. Only the capped
    /// rungs append, so the cap is always in force.
    fn add(&mut self, slot: Option<usize>) {
        let Some(j) = slot else {
            return;
        };
        if (self.mv_mask >> j) & 1 == 0 && usize::from(self.mv_len) < MAX_MOVE {
            self.push(j as u8);
        }
    }

    /// Records the path neighbor `slot`, from `deadlock.at`, in `near`
    /// unless there is none or it is already there.
    fn note(&mut self, slot: Option<usize>) {
        let Some(j) = slot else {
            return;
        };
        if (self.near_mask >> j) & 1 == 0 {
            self.near[usize::from(self.near_len)] = j as u8;
            self.near_len += 1;
            self.near_mask |= 1 << j;
        }
    }
}

/// The keeper's cell after push `k - 1` of a path from `own`, for
/// `k >= 1`: the box's cell before push `k - 1`.
fn keeper_after(path: &[Cell], own: Cell, k: usize) -> Cell {
    if k == 1 { own } else { path[k - 2] }
}

/// The first leg from `k >= 1` on that the probe forms: the stand of its
/// push differs from the keeper's cell after the previous push.
fn next_formed(board: &Board, via: &[u8], path: &[Cell], own: Cell, k: usize) -> Option<usize> {
    let formed = |j: usize| rank::stand(board, via, path[j]) != keeper_after(path, own, j);
    (k..path.len()).find(|&j| formed(j))
}

/// Keeper steps from `start` to each cell around the `blocked` cell into
/// `dist`, `NONE` where unreached, with `queue` as scratch. The keeper
/// flood already holds the leg's other flood, so this one needs its own
/// buffer.
fn flood(board: &Board, start: Cell, blocked: Cell, dist: &mut [u16], queue: &mut [u16]) {
    debug_assert_ne!(start, blocked);
    let neighbors = board.neighbors();
    dist[..neighbors.len()].fill(NONE);
    dist[usize::from(start)] = 0;
    queue[0] = start;
    let (mut head, mut tail) = (0, 1);
    while head < tail {
        let x = queue[head];
        head += 1;
        let next = dist[usize::from(x)] + 1;
        for &y in &neighbors[usize::from(x)] {
            if y == NONE || y == blocked || dist[usize::from(y)] != NONE {
                continue;
            }
            dist[usize::from(y)] = next;
            queue[tail] = y;
            tail += 1;
        }
    }
}

/// One shortest keeper walk from `from` to `to` on the empty board, as the
/// probe takes it. `reach.fill_from(board, to, &[])` floods back from `to`.
/// The walk then descends from `from`, each time to the first neighbor in
/// direction order (`U D L R`) that is one step closer. It calls `visit`
/// on each cell from `from` to `to`, both included, and calls nothing when
/// `from` cannot reach `to`. Overwrites the keeper flood.
fn walk_path(board: &Board, reach: &mut Reach, from: Cell, to: Cell, mut visit: impl FnMut(Cell)) {
    reach.fill_from(board, to, &[]);
    let mut d = reach.distance(from);
    if d == NONE {
        return;
    }
    let neighbors = board.neighbors();
    let mut x = from;
    visit(x);
    while d > 0 {
        d -= 1;
        // The flood reached x from a neighbor one step closer to `to`, so
        // the search always finds a step and the walk ends on `to`, the
        // only cell at distance 0.
        let closer = |&y: &Cell| y != NONE && reach.distance(y) == d;
        let Some(y) = neighbors[usize::from(x)].into_iter().find(closer) else {
            return;
        };
        x = y;
        visit(x);
    }
}

#[cfg(test)]
mod tests {
    use super::{Movers, MoversUnit, walk_path};
    use crate::{
        heuristic::Heuristic,
        reach::Reach,
        stage::{Candidate, FOCUS, FRAMES, MAX_MOVE, Scratch, Tools, bit, checks, rank, reference},
        testkit::{Lcg, catalog},
    };
    use sokomind_core::{Board, Cell, NONE, OPPOSITE, State, WALL};

    /// The (from, to) pairs `walk_path_matches_probe` draws per board.
    const PAIRS: usize = 200;
    /// A board of two floor components, the top row and the bottom row,
    /// which the wall row between splits. A catalog board's floor may be
    /// one component, so this board makes sure some pairs are unreachable.
    const SPLIT: &str = "RA a\nOOOO\n  Bb";
    /// The random roots `movers_match_probe` draws per catalog board.
    const DRAWS: usize = 3;
    /// The random roots it draws on huge. Its 17 boxes block many keeper
    /// walks, so its roots are the likeliest to fill `mv` among the legs.
    const HUGE_DRAWS: usize = 12;

    /// On random (from, to) pairs of floor cells of every catalog board and
    /// of [`SPLIT`], the cells `walk_path` visits, in order, equal
    /// `reference::Geo::walk_path` (the probe's 235-251). Some pairs must
    /// be unreachable, where neither visits anything.
    #[test]
    fn walk_path_matches_probe() {
        let mut rng = Lcg(0x3a1f);
        let mut boards = catalog();
        let two = Board::parse(SPLIT).expect("the split board");
        boards.push(("split".to_owned(), two));
        let mut far = 0;
        for (id, board) in &boards {
            let heuristic = Heuristic::new(board);
            let geo = reference::Geo::new(board, &heuristic);
            let tiles = board.tiles();
            let mut reach = Reach::new(tiles.len());
            let floor: Vec<Cell> = (0..tiles.len() as Cell)
                .filter(|&c| tiles[usize::from(c)] != WALL)
                .collect();
            for _ in 0..PAIRS {
                let from = floor[rng.below(floor.len())];
                let to = floor[rng.below(floor.len())];
                let mut got = Vec::new();
                walk_path(board, &mut reach, from, to, |c| got.push(c));
                let want = geo.walk_path(from, to);
                far += usize::from(want.is_empty());
                assert_eq!(got, want, "{id}: {from} to {to}");
            }
        }
        assert!(far > 0, "no pair was unreachable");
    }

    /// Random roots of every catalog board, plus huge's extra roots, at
    /// every rung: `rank::rank_all` with `checks::root_lanes`, then every
    /// movers unit from `Path(0)` (see [`drive`]). The units, `movers()`
    /// (order included), the `scratch.frozen` bits and the need rows must
    /// equal [`probe_movers`] on the same candidate list. The need marks
    /// go to frame `top`, a different one per rung, and every other row
    /// must stay clear. At rungs 0 and 1 some root must hit the early leg
    /// stop, with legs left when `mv` fills, and some root must hit it
    /// after a leg has run.
    #[test]
    fn movers_match_probe() {
        let mut rng = Lcg(0x6d0b);
        // Bit `r` is set once the early leg stop fires at rung `r`.
        let mut early = 0u32;
        let mut mid = 0;
        for (id, board) in catalog() {
            let mut owned = reference::Owned::new(&board);
            let geo = reference::Geo::new(&board, &owned.heuristic);
            let draws = if id == "huge" { HUGE_DRAWS } else { DRAWS };
            let mut roots = vec![board.initial()];
            for _ in 0..draws {
                roots.push(reference::random_state(&mut rng, &board, &owned.heuristic));
            }
            if id == "huge" {
                roots.extend(reference::huge_roots(&geo, 40));
            }
            let cells = board.tiles().len();
            let (mut tools, facts, scratch) = owned.parts(&board);
            for (r, root) in roots.iter().enumerate() {
                let lanes = checks::root_lanes(&mut tools, root);
                rank::rank_all(&mut tools, facts, scratch, root, lanes);
                let candidates = scratch.candidates.clone();
                for rung in 0..FOCUS.len() {
                    let top = rung * (FRAMES - 1) / 2;
                    let (units, movers) = drive(&mut tools, scratch, root, rung, top);
                    let want = probe_movers(&geo, root, &candidates, rung);
                    let at = format!("{id} root {r} rung {rung}");
                    assert_eq!(units, want.units, "{at}");
                    let mv: Vec<usize> = movers.movers().iter().map(|&j| usize::from(j)).collect();
                    assert_eq!(mv, want.mv, "{at}");
                    assert_eq!(bits(&scratch.frozen, cells), want.frozen, "{at}: frozen");
                    let row = scratch.need.len() / FRAMES;
                    for (f, words) in scratch.need.chunks_exact(row).enumerate() {
                        if f == top {
                            assert_eq!(bits(words, cells), want.need, "{at}: need");
                        } else {
                            assert!(words.iter().all(|&w| w == 0), "{at}: frame {f}");
                        }
                    }
                    if rung + 1 < FOCUS.len() && want.stop < want.legs {
                        early |= 1 << rung;
                        mid += usize::from(want.stop > 0);
                    }
                }
            }
        }
        assert_eq!(early, 0b11, "the leg stop missed a rung: {early:b}");
        assert!(mid > 0, "the leg stop never fired after a leg");
    }

    /// Runs every movers unit of `root` at `rung`, with the need marks in
    /// frame `top`, from `Path(0)` until one gives `None`. Clears `need`
    /// and fills `frozen` first, so a mark in the wrong row or a frozen bit
    /// `Merge` failed to clear shows.
    fn drive(
        tools: &mut Tools<'_>,
        scratch: &mut Scratch,
        root: &State,
        rung: usize,
        top: usize,
    ) -> (Vec<MoversUnit>, Movers) {
        scratch.need.fill(0);
        scratch.frozen.fill(u32::MAX);
        let mut movers = Movers::new(&scratch.candidates, rung, top);
        let mut units = Vec::new();
        let mut unit = Some(MoversUnit::Path(0));
        while let Some(now) = unit {
            units.push(now);
            unit = movers.run(tools, scratch, root, now);
        }
        (units, movers)
    }

    /// The cells of a bitset over `cells` cells, one flag each.
    fn bits(words: &[u32], cells: usize) -> Vec<bool> {
        (0..cells as Cell).map(|c| bit(words, c)).collect()
    }

    /// What the probe gives for one stage start, with the units production
    /// runs for it.
    struct Want {
        /// A `Path` per used candidate (one when none is used), below the
        /// last rung a `Leg` per formed leg before `stop`, then `Merge`.
        units: Vec<MoversUnit>,
        /// The movers, in order.
        mv: Vec<usize>,
        /// Per cell: whether it holds a root box outside `mv`.
        frozen: Vec<bool>,
        /// Per cell: whether the stage plan needs it.
        need: Vec<bool>,
        /// The number of legs the probe forms.
        legs: usize,
        /// The first leg that production does not run, because the movers
        /// already hold `MAX_MOVE` boxes before it, or `legs`.
        stop: usize,
    }

    /// A port of the probe's `movers` (p4b.rs 1057-1123, with
    /// `MOVE_EXTRA = 2`) and the need marks of a stage start (1758-1767) on
    /// `geo`, for the ranked list `cands`. The used candidates are a prefix,
    /// so a count stands for the probe's index list.
    fn probe_movers(
        geo: &reference::Geo<'_>,
        root: &State,
        cands: &[Candidate],
        rung: usize,
    ) -> Want {
        let n = geo.nb.len();
        let occ = geo.occ(root);
        let mut focus: Vec<usize> = Vec::new();
        let mut used = 0;
        for c in cands {
            let slot = usize::from(c.slot);
            if !focus.contains(&slot) {
                if focus.len() == FOCUS[rung] {
                    break;
                }
                focus.push(slot);
            }
            used += 1;
        }
        // The probe keeps each candidate's path cells and stands from Rank.
        let mut paths = Vec::new();
        for (k, c) in cands[..used].iter().enumerate() {
            let slot = usize::from(c.slot);
            let x = root.boxes[slot];
            let (_, via) = geo.push(x, &geo.dead[geo.group[slot]]);
            let (cells, stands) = geo.path(x, c.target, &via);
            assert_eq!(stands[0], c.first_stand, "candidate {k}");
            paths.push((slot, cells, stands));
        }
        let mut units = Vec::new();
        for u in 0..used.max(1) {
            units.push(MoversUnit::Path(u as u8));
        }
        let mut mv = focus.clone();
        let (mut legs, mut stop) = (0, 0);
        if rung + 1 < FOCUS.len() {
            for (_, cells, stands) in &paths {
                for &cell in cells.iter().chain(stands) {
                    add(&mut mv, occ[usize::from(cell)]);
                }
            }
            let base = mv.clone();
            let (out, formed) = walk_blockers(geo, root, &paths, &occ);
            // The movers' size before a leg, had the probe appended the
            // blockers of the legs before it as it found them.
            let grown = |len: usize| {
                let extra = out[..len].iter().filter(|&&b| !base.contains(&b)).count();
                base.len() + extra
            };
            let first = formed.iter().position(|&(_, b)| grown(b) >= MAX_MOVE);
            legs = formed.len();
            stop = first.unwrap_or(legs);
            units.extend(formed[..stop].iter().map(|&(unit, _)| unit));
            for b in out {
                if !mv.contains(&b) {
                    mv.push(b);
                }
            }
            let b1 = mv[focus.len()..].to_vec();
            for &b in &b1 {
                let around = geo.nb[usize::from(root.boxes[b])];
                for (d, &y) in around.iter().enumerate() {
                    let s = around[OPPOSITE[d]];
                    if y == NONE || s == NONE || geo.dead[geo.group[b]][usize::from(y)] {
                        continue;
                    }
                    add(&mut mv, occ[usize::from(s)]);
                }
            }
            for (_, cells, _) in &paths {
                for &cell in cells {
                    for y in geo.nb[usize::from(cell)] {
                        if y != NONE {
                            add(&mut mv, occ[usize::from(y)]);
                        }
                    }
                }
            }
            mv.truncate(MAX_MOVE);
        } else {
            mv = (0..geo.slots).collect();
        }
        units.push(MoversUnit::Merge);
        let mut frozen = vec![false; n];
        for (i, &b) in root.boxes[..geo.slots].iter().enumerate() {
            frozen[usize::from(b)] = !mv.contains(&i);
        }
        let mut need = vec![false; n];
        for (_, cells, stands) in &paths {
            for &x in cells.iter().chain(stands) {
                need[usize::from(x)] = true;
            }
            for x in geo.walk_path(root.player, stands[0]) {
                need[usize::from(x)] = true;
            }
        }
        Want {
            units,
            mv,
            frozen,
            need,
            legs,
            stop,
        }
    }

    /// The probe's append of the box with `occ` value `o` to `mv`: only for
    /// a box, and only once.
    fn add(mv: &mut Vec<usize>, o: usize) {
        if o != 0 && !mv.contains(&(o - 1)) {
            mv.push(o - 1);
        }
    }

    /// A port of the probe's `walk_blockers` (p4b.rs 1004-1051) over the
    /// used candidates' (slot, path cells, stands). Gives the blockers, and
    /// each leg the probe forms as production's `Leg` unit with the number
    /// of blockers found before it.
    fn walk_blockers(
        geo: &reference::Geo<'_>,
        root: &State,
        paths: &[(usize, Vec<Cell>, Vec<Cell>)],
        occ: &[usize],
    ) -> (Vec<usize>, Vec<(MoversUnit, usize)>) {
        let n = geo.nb.len();
        let mut out = Vec::new();
        let mut formed = Vec::new();
        for (u, (slot, cells, stands)) in paths.iter().enumerate() {
            let (u, own) = (u as u8, root.boxes[*slot]);
            // (leg, from, to, box cell during the leg)
            let mut legs = vec![(0, root.player, stands[0], own)];
            let mut before = own;
            for (k, (&at, &to)) in cells.iter().zip(&stands[1..]).enumerate() {
                if to != before {
                    legs.push((k as u16 + 1, before, to, at));
                }
                before = at;
            }
            for (k, from, to, at) in legs {
                formed.push((MoversUnit::Leg { u, k }, out.len()));
                let mut all = vec![false; n];
                for (i, &b) in root.boxes[..geo.slots].iter().enumerate() {
                    all[usize::from(b)] = i != *slot;
                }
                all[usize::from(from)] = false;
                all[usize::from(to)] = false;
                all[usize::from(at)] = true;
                if from == at || to == at {
                    continue;
                }
                if geo.flood(from, &all)[usize::from(to)] != reference::FAR {
                    continue;
                }
                let mut only = vec![false; n];
                only[usize::from(at)] = true;
                let dp = geo.flood(from, &only);
                let ds = geo.flood(to, &only);
                let span = dp[usize::from(to)];
                if span == reference::FAR {
                    continue;
                }
                for (x, (&p, &s)) in dp.iter().zip(&ds).enumerate() {
                    if p != reference::FAR && s != reference::FAR && p + s == span {
                        let o = occ[x];
                        if o != 0 && o - 1 != *slot && !out.contains(&(o - 1)) {
                            out.push(o - 1);
                        }
                    }
                }
            }
        }
        (out, formed)
    }
}
