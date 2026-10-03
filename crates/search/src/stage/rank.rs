//! A frame root's candidate moves, ranked and filtered: ports the probe's
//! `rank`, `push` and `path` (p4b.rs 929-1003, 523-562).
//!
//! A box that is not on a goal of its own label gets up to three
//! candidates. Kind 0 targets are the two nearest goal columns of its
//! group that the root leaves unfilled. Kind 1 is the nearest floor cell
//! outside every room, which only a misplaced box gets. Each candidate
//! follows one shortest path of the box's one-box push search. That search
//! ignores the keeper and the other boxes and skips cells dead for the box.
//! The candidates are ranked by cost: the path's length, plus the keeper's
//! walk to the first stand, plus 2 for each other box on the path's cells
//! or stands.
//!
//! The work runs in units:
//! - `Rank(i)` for each box `i`: one push search and the box's candidates,
//!   at their partial cost.
//! - `Order`: the walk costs and the sort.
//! - `Filter`: one accept check of one kind-0 candidate, as if its box were
//!   already on the target.
//!
//! The probe filters before it sorts. The sort key is unique, so filtering
//! the sorted list keeps the same candidates in the same order.
use super::{
    Candidate, Facts, MAX_CANDIDATES, Scratch, Tools,
    checks::{self, Check, Verdict},
};
use crate::heuristic::Heuristic;
use sokomind_core::{Board, Cell, NONE, OPPOSITE, State};

// Order marks the candidates its first flood misses in a u128.
const _: () = assert!(MAX_CANDIDATES <= u128::BITS as usize);

/// One unit of ranking a frame root's candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RankUnit {
    /// The candidates of the box in slot `i`. `Rank(0)` starts a ranking.
    Rank(u8),
    /// Adds each candidate's keeper walk to its cost and sorts the list.
    Order,
    /// Runs accept unit `check` on candidate `cand`'s hypothetical state.
    Filter { cand: u8, check: Check },
}

/// Runs one unit, `unit`, of ranking `root`'s candidates into
/// `scratch.candidates`. `lanes` is `checks::root_lanes(root)`. Rank units
/// first refresh Deadlock's occupancy to `root`.
///
/// - `Rank(i)`: `Rank(0)` first clears the list. Box `i` is skipped when it
///   is on a goal of its own label. Otherwise [`push_bfs`] from its cell
///   finds its targets, in the probe's order:
///   - kind 0: the up to 2 goal columns of its group that are unfilled in
///     `root` and reached, nearest first, ties by cell. A target holding
///     any box is then dropped; it still counts toward the 2.
///   - kind 1, when the box is misplaced: the nearest reached floor cell
///     outside every room, ties by cell.
///
///   For each target, [`rebuild_path`] writes the path into the pool's
///   path region. The candidate's partial cost is the path's length plus 2
///   for each other box on the path's cells or stands. Gives `Rank(i + 1)`,
///   or `Order` after the last box.
/// - `Order`: a keeper walk is added to each cost. When `reach.fill(root)`
///   reaches the first stand, the walk is that distance. Otherwise, when
///   `reach.fill_from(root.player, &[])` reaches it, the walk is that
///   distance plus 8. Otherwise the walk is 999. The second flood runs only
///   when the first misses some first stand. Then the list is sorted by
///   (cost, kind, the box's cell, target). Gives `Filter` of the first
///   candidate at Lane, or `None` for an empty list.
/// - `Filter { cand, check }`: a kind-1 candidate is kept, and so is a
///   kind-0 candidate whose keeper cell holds another box of `root`.
///   Either gives `Filter` of the next candidate. Otherwise the unit runs
///   `checks::run` on the hypothetical state: `root` with the box on its
///   target and the keeper on the candidate's keeper cell. `scratch.pockets`
///   holds the pocket check. On a reject, the candidate is removed and the
///   unit gives `Filter` of the candidate that takes its index. On a pass,
///   the unit gives `Filter` of the next candidate, and on `Next` the next
///   check of the same candidate. Past the last candidate it gives `None`.
pub(super) fn run(
    tools: &mut Tools<'_>,
    facts: &Facts,
    scratch: &mut Scratch,
    root: &State,
    lanes: u32,
    unit: RankUnit,
) -> Option<RankUnit> {
    let boxes = tools.board.labels().len();
    match unit {
        RankUnit::Rank(i) => {
            rank_box(tools, facts, scratch, root, usize::from(i));
            let next = if usize::from(i) + 1 < boxes {
                RankUnit::Rank(i + 1)
            } else {
                RankUnit::Order
            };
            Some(next)
        }
        RankUnit::Order => {
            order(tools, &mut scratch.candidates, root);
            filter_from(0, scratch.candidates.len())
        }
        RankUnit::Filter { cand, check } => {
            let k = usize::from(cand);
            let c = scratch.candidates[k];
            let slot = usize::from(c.slot);
            // The relaxed path ignores the other boxes, so a keeper cell on
            // one of them is no evidence against the candidate.
            let clash = root.boxes[..boxes]
                .iter()
                .enumerate()
                .any(|(j, &b)| j != slot && b == c.hyp_player);
            let verdict = if c.kind != 0 || clash {
                Verdict::Pass
            } else {
                let mut hyp = *root;
                hyp.boxes[slot] = c.target;
                hyp.player = c.hyp_player;
                checks::run(tools, facts, &mut scratch.pockets, &hyp, lanes, check)
            };
            match verdict {
                Verdict::Next(check) => Some(RankUnit::Filter { cand, check }),
                Verdict::Reject => {
                    scratch.candidates.remove(k);
                    filter_from(k, scratch.candidates.len())
                }
                Verdict::Pass => filter_from(k + 1, scratch.candidates.len()),
            }
        }
    }
}

/// The `Rank(i)` unit of [`run`]. A box has at most three candidates, so
/// the list stays within [`MAX_CANDIDATES`].
fn rank_box(tools: &mut Tools<'_>, facts: &Facts, scratch: &mut Scratch, root: &State, i: usize) {
    let (board, heuristic) = (tools.board, tools.heuristic);
    let boxes = board.labels().len();
    tools.deadlock.refresh(&root.boxes[..boxes]);
    let deadlock = &*tools.deadlock;
    if i == 0 {
        scratch.candidates.clear();
    }
    let x = root.boxes[i];
    if board.on_goal(i, x) {
        return;
    }
    let (dist, queue, _, path) = super::split(&mut scratch.pool, board.tiles().len());
    push_bfs(board, heuristic, i, x, dist, queue, &mut scratch.via);
    // The two nearest unfilled goals as (pushes, cell). A goal column of
    // the box's group is filled exactly when it holds a settled box.
    let mut near = [(NONE, NONE); 2];
    for &q in &heuristic.goal_cells()[heuristic.group(i)] {
        let d = dist[usize::from(q)];
        if d != NONE && !super::settled(board, deadlock, q) && (d, q) < near[1] {
            near[1] = (d, q);
            if near[1] < near[0] {
                near.swap(0, 1);
            }
        }
    }
    let mut targets: [(u8, Cell); 3] = [(0, NONE); 3];
    let mut count = 0;
    for (_, q) in near {
        if q != NONE && deadlock.at(q).is_none() {
            targets[count] = (0, q);
            count += 1;
        }
    }
    if facts.rooms.misplaced(heuristic, i, x) {
        let mut best = (NONE, NONE);
        for (c, &d) in dist.iter().enumerate() {
            if d < best.0 && facts.rooms.room(c as Cell) == 0 {
                best = (d, c as Cell);
            }
        }
        if best.1 != NONE {
            targets[count] = (1, best.1);
            count += 1;
        }
    }
    // A kind-0 target is a goal of the box's label, which its cell is not,
    // and a kind-1 target lies outside every room, while a misplaced box
    // is in one, so no path is empty.
    for &(kind, q) in &targets[..count] {
        let len = rebuild_path(board, x, q, &scratch.via, path);
        let mut blockers = 0u32;
        for &c in &path[..len] {
            for cell in [c, stand(board, &scratch.via, c)] {
                if let Some(j) = deadlock.at(cell).filter(|&j| j != i) {
                    blockers |= 1 << j;
                }
            }
        }
        debug_assert!(scratch.candidates.len() < scratch.candidates.capacity());
        scratch.candidates.push(Candidate {
            slot: i as u8,
            kind,
            target: q,
            first_stand: stand(board, &scratch.via, path[0]),
            hyp_player: if len >= 2 { path[len - 2] } else { x },
            cost: len as u32 + 2 * blockers.count_ones(),
        });
    }
}

/// The `Order` unit of [`run`]: adds the keeper walks and sorts.
fn order(tools: &mut Tools<'_>, candidates: &mut [Candidate], root: &State) {
    let board = tools.board;
    tools.reach.fill(board, root);
    let mut far = 0u128;
    for (k, c) in candidates.iter_mut().enumerate() {
        let d = tools.reach.distance(c.first_stand);
        if d == NONE {
            far |= 1 << k;
        } else {
            c.cost += u32::from(d);
        }
    }
    if far != 0 {
        tools.reach.fill_from(board, root.player, &[]);
        for (k, c) in candidates.iter_mut().enumerate() {
            if far & (1 << k) != 0 {
                let d = tools.reach.distance(c.first_stand);
                c.cost += if d == NONE { 999 } else { u32::from(d) + 8 };
            }
        }
    }
    let cell = |c: &Candidate| root.boxes[usize::from(c.slot)];
    candidates.sort_unstable_by_key(|c| (c.cost, c.kind, cell(c), c.target));
}

/// `Filter` of candidate `k` at its first check, or `None` when `k` is past
/// the last of `len` candidates.
fn filter_from(k: usize, len: usize) -> Option<RankUnit> {
    (k < len).then_some(RankUnit::Filter {
        cand: k as u8,
        check: Check::Lane,
    })
}

/// Runs every unit of ranking `root`, from `Rank(0)` until the sequence
/// ends, and gives the units in the order they ran.
#[cfg(test)]
pub(super) fn rank_all(
    tools: &mut Tools<'_>,
    facts: &Facts,
    scratch: &mut Scratch,
    root: &State,
    lanes: u32,
) -> Vec<RankUnit> {
    let mut units = Vec::new();
    let mut unit = Some(RankUnit::Rank(0));
    while let Some(now) = unit {
        units.push(now);
        unit = run(tools, facts, scratch, root, lanes, now);
    }
    units
}

/// The probe's one-box push search from `from` for the box in `slot`. The
/// keeper and every other box are ignored, and cells that
/// `heuristic.dead(slot, _)` calls dead are skipped. A push in direction
/// `d` needs both the cell ahead and the stand behind on the board.
/// Directions are tried in `U D L R` order, so the paths and stands match
/// the probe's.
///
/// Afterwards `dist` holds the pushes to each reached cell and `NONE`
/// elsewhere, and `via` holds the direction of the push that entered each
/// reached cell other than `from`. `queue` is scratch. Each slice needs at
/// least one entry per cell.
pub(super) fn push_bfs(
    board: &Board,
    heuristic: &Heuristic,
    slot: usize,
    from: Cell,
    dist: &mut [u16],
    queue: &mut [u16],
    via: &mut [u8],
) {
    let neighbors = board.neighbors();
    dist[..neighbors.len()].fill(NONE);
    dist[usize::from(from)] = 0;
    queue[0] = from;
    let (mut head, mut tail) = (0, 1);
    while head < tail {
        let x = queue[head];
        head += 1;
        let around = neighbors[usize::from(x)];
        let pushes = dist[usize::from(x)] + 1;
        for (d, &y) in around.iter().enumerate() {
            if y == NONE || around[OPPOSITE[d]] == NONE {
                continue;
            }
            if dist[usize::from(y)] != NONE || heuristic.dead(slot, y) {
                continue;
            }
            dist[usize::from(y)] = pushes;
            via[usize::from(y)] = d as u8;
            queue[tail] = y;
            tail += 1;
        }
    }
}

/// Writes the path of the last [`push_bfs`] from `from` to `to`, a reached
/// cell other than `from`, into `path`: the cells the box enters, in push
/// order, `to` last. Returns the path's length.
pub(super) fn rebuild_path(
    board: &Board,
    from: Cell,
    to: Cell,
    via: &[u8],
    path: &mut [Cell],
) -> usize {
    let neighbors = board.neighbors();
    let mut len = 0;
    let mut x = to;
    while x != from {
        path[len] = x;
        len += 1;
        x = neighbors[usize::from(x)][OPPOSITE[usize::from(via[usize::from(x)])]];
    }
    path[..len].reverse();
    len
}

/// The keeper's stand for the push that entered `cell` in the last
/// [`push_bfs`]: two cells back along that push's direction.
pub(super) fn stand(board: &Board, via: &[u8], cell: Cell) -> Cell {
    let back = OPPOSITE[usize::from(via[usize::from(cell)])];
    let prev = board.neighbors()[usize::from(cell)][back];
    board.neighbors()[usize::from(prev)][back]
}

#[cfg(test)]
mod tests {
    use super::{Check, RankUnit, push_bfs, rank_all, rebuild_path, stand};
    use crate::{
        corral::Pockets,
        heuristic::Heuristic,
        stage::{Candidate, Facts, Scratch, Tools, checks, reference, split},
        testkit::{Lcg, catalog},
    };
    use sokomind_core::{Cell, NONE, State, WALL};

    /// Random roots of every catalog board, plus huge's extra roots: on
    /// each, `rank_all` runs the units in the order [`check_units`] wants
    /// and gives the candidate list of [`probe_rank`], fields and order
    /// included. The filter must drop some candidate and keep some kind-0
    /// clash, and some root must get a kind-1 candidate. Each root also
    /// runs [`check_paths`].
    #[test]
    fn filter_matches_rank() {
        let mut rng = Lcg(0x7a2c);
        let (mut dropped, mut clashes) = (0, 0);
        let mut kinds = [0usize; 2];
        for (id, board) in catalog() {
            let mut owned = reference::Owned::new(&board);
            let geo = reference::Geo::new(&board, &owned.heuristic);
            let mut roots = vec![board.initial()];
            for _ in 0..3 {
                roots.push(reference::random_state(&mut rng, &board, &owned.heuristic));
            }
            if id == "huge" {
                roots.extend(reference::huge_roots(&geo, 80));
            }
            let boxes = board.labels().len();
            let (mut tools, facts, scratch) = owned.parts(&board);
            for (r, root) in roots.iter().enumerate() {
                check_paths(&geo, tools.heuristic, scratch, root);
                let lanes = checks::root_lanes(&mut tools, root);
                let units = rank_all(&mut tools, facts, scratch, root, lanes);
                check_units(&units, boxes);
                let got = scratch.candidates.clone();
                let pockets = &mut scratch.pockets;
                let (want, d, c) = probe_rank(&geo, &mut tools, facts, pockets, root, lanes);
                assert_eq!(got, want, "{id} root {r}");
                dropped += d;
                clashes += c;
                for cand in &got {
                    kinds[usize::from(cand.kind)] += 1;
                }
            }
        }
        assert!(dropped > 0, "the filter dropped no candidate");
        assert!(clashes > 0, "no kind-0 candidate was kept for a clash");
        assert!(kinds[1] > 0, "no root got a kind-1 candidate");
    }

    /// Checks the order of the units of ranking a root of `boxes` boxes:
    /// `Rank(0)` to `Rank(boxes - 1)`, `Order`, then `Filter` units, the
    /// first on candidate 0 at Lane and each later one on the same
    /// candidate or at Lane on the next.
    fn check_units(units: &[RankUnit], boxes: usize) {
        for (i, &u) in units[..boxes].iter().enumerate() {
            assert_eq!(u, RankUnit::Rank(i as u8));
        }
        assert_eq!(units[boxes], RankUnit::Order);
        let mut prev = None;
        for &u in &units[boxes + 1..] {
            let RankUnit::Filter { cand, check } = u else {
                panic!("{u:?} after Order");
            };
            let ok = match prev {
                None => cand == 0 && check == Check::Lane,
                Some(p) => cand == p || (cand == p + 1 && check == Check::Lane),
            };
            assert!(ok, "{u:?} after {prev:?}");
            prev = Some(cand);
        }
    }

    /// Checks `push_bfs`, `rebuild_path` and `stand` against `Geo::push`
    /// and `Geo::path` from each box of `root`, on every cell reached.
    fn check_paths(
        geo: &reference::Geo<'_>,
        heuristic: &Heuristic,
        scratch: &mut Scratch,
        root: &State,
    ) {
        let cells = geo.nb.len();
        for i in 0..geo.slots {
            let x = root.boxes[i];
            let (dist, queue, _, path) = split(&mut scratch.pool, cells);
            push_bfs(geo.board, heuristic, i, x, dist, queue, &mut scratch.via);
            let (want, via) = geo.push(x, &geo.dead[geo.group[i]]);
            for (c, &d) in want.iter().enumerate() {
                if d == reference::FAR {
                    assert_eq!(dist[c], NONE, "box {i} cell {c}");
                    continue;
                }
                assert_eq!(u32::from(dist[c]), d, "box {i} cell {c}");
                if c == usize::from(x) {
                    continue;
                }
                let to = c as Cell;
                assert_eq!(scratch.via[c], via[c], "box {i} cell {c}");
                let (want_cells, stands) = geo.path(x, to, &via);
                let len = rebuild_path(geo.board, x, to, &scratch.via, path);
                assert_eq!(&path[..len], want_cells.as_slice(), "box {i} cell {c}");
                for (&cell, &s) in want_cells.iter().zip(&stands) {
                    assert_eq!(stand(geo.board, &scratch.via, cell), s, "box {i} cell {c}");
                }
            }
        }
    }

    /// A port of the probe's `rank` (p4b.rs 929-1003) on `geo`, run in one
    /// call: it filters each candidate as it builds it, then sorts. The
    /// probe's `accept` of a kind-0 candidate's hypothetical state is
    /// `checks::first_reject`, the production checks run to completion.
    /// The misplaced and room tests are `facts.rooms`. Also gives the
    /// number of candidates the filter dropped and of kind-0 candidates it
    /// kept for a keeper clash.
    fn probe_rank(
        geo: &reference::Geo<'_>,
        tools: &mut Tools<'_>,
        facts: &Facts,
        pockets: &mut Pockets,
        root: &State,
        lanes: u32,
    ) -> (Vec<Candidate>, usize, usize) {
        let n = geo.nb.len();
        let occ = geo.occ(root);
        let mut all_boxes = vec![false; n];
        for &b in &root.boxes[..geo.slots] {
            all_boxes[usize::from(b)] = true;
        }
        let none = vec![false; n];
        let reach = geo.flood(root.player, &all_boxes);
        let free = geo.flood(root.player, &none);
        let tiles = geo.board.tiles();
        let mut cands = Vec::new();
        let (mut dropped, mut clashes) = (0, 0);
        for i in 0..geo.slots {
            let x = root.boxes[i];
            if geo.board.on_goal(i, x) {
                continue;
            }
            let g = geo.group[i];
            let (dist, via) = geo.push(x, &geo.dead[g]);
            let mut goals = Vec::new();
            for &(q, gq) in &geo.goals {
                let d = dist[usize::from(q)];
                if gq == g && !geo.filled(q, g, &occ) && d != reference::FAR {
                    goals.push((d, q));
                }
            }
            goals.sort_unstable();
            let mut targets: Vec<(u8, Cell)> = Vec::new();
            for &(_, q) in goals.iter().take(2) {
                targets.push((0, q));
            }
            if facts.rooms.misplaced(tools.heuristic, i, x) {
                let exit = (0..n)
                    .filter(|&c| tiles[c] != WALL && facts.rooms.room(c as Cell) == 0)
                    .filter(|&c| dist[c] != reference::FAR)
                    .map(|c| (dist[c], c as Cell))
                    .min();
                if let Some((_, q)) = exit {
                    targets.push((1, q));
                }
            }
            for (kind, q) in targets {
                let (cells, stands) = geo.path(x, q, &via);
                let hyp_player = if cells.len() >= 2 {
                    cells[cells.len() - 2]
                } else {
                    x
                };
                if kind == 0 {
                    if occ[usize::from(q)] != 0 {
                        continue;
                    }
                    let mut hyp = *root;
                    hyp.boxes[i] = q;
                    hyp.player = hyp_player;
                    let o = occ[usize::from(hyp.player)];
                    if o != 0 && o != i + 1 {
                        clashes += 1;
                    } else {
                        let reject = checks::first_reject(tools, facts, pockets, &hyp, lanes);
                        if reject.is_some() {
                            dropped += 1;
                            continue;
                        }
                    }
                }
                let s0 = usize::from(stands[0]);
                let walk = if reach[s0] != reference::FAR {
                    reach[s0]
                } else if free[s0] != reference::FAR {
                    free[s0] + 8
                } else {
                    999
                };
                let mut blk = 0;
                for (j, b) in root.boxes[..geo.slots].iter().enumerate() {
                    if j != i && (cells.contains(b) || stands.contains(b)) {
                        blk += 1;
                    }
                }
                cands.push(Candidate {
                    slot: i as u8,
                    kind,
                    target: q,
                    first_stand: stands[0],
                    hyp_player,
                    cost: cells.len() as u32 + walk + 2 * blk,
                });
            }
        }
        cands.sort_by_key(|c| (c.cost, c.kind, root.boxes[usize::from(c.slot)], c.target));
        (cands, dropped, clashes)
    }
}
