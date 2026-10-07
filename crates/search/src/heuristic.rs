//! Lower bound on the moves left: per label, a minimum-cost matching of the
//! boxes to that label's goals over reverse-push distances. Each distance
//! ignores every other box, so the bound is admissible in moves. With keeper
//! sides it counts the pushes a lone box needs from its cell with the keeper
//! on its current side, each with the walk its stand forces (see
//! `charges`); without, the pushes from its cell with the keeper anywhere.
//! It is incremental: a push re-solves only the pushed box's group, and a group
//! of `REPAIR_CROSSOVER` or more boxes repairs the parent's duals with one
//! augment instead of solving from scratch.
use crate::reach::Blocks;
use sokomind_core::{Board, Cell, MAX_BOXES, MAX_CELLS, NONE, OPPOSITE, State};
use std::{mem::size_of, ops::Range};

/// A cell has at most one side per neighbor, and [`Blocks`] labels each
/// side by a neighbor's direction.
const SIDES: usize = 4;

/// What a box's push distances depend on: with keeper sides, its cell and
/// the side of it the keeper is on, as `cell * SIDES + side`; without, its
/// cell.
type Spot = u16;
// Every spot fits below the NONE sentinel.
const _: () = assert!(SIDES * MAX_CELLS <= NONE as usize);

const INF: i32 = 1_000_000;
/// The largest push distance. An uncharged distance counts pushes between
/// distinct spots, so it never exceeds this; a charged one is capped here,
/// which keeps it a lower bound that drops by no more per push.
const MAX_DISTANCE: u16 = (SIDES * MAX_CELLS - 1) as u16;
// INF exceeds every finite matching, and a full row of INF costs cannot
// overflow i32. A dual repair's augmenting path can cost one row more than
// a full matching, so MAX_BOXES + 1 rows of the largest distance stay below
// INF.
const _: () = assert!(
    (MAX_BOXES + 1) * (MAX_DISTANCE as usize) < INF as usize
        && (MAX_BOXES as i64 + 1) * (INF as i64) <= i32::MAX as i64
);
// Duals indices and group_of group indices fit a byte.
const _: () = assert!(MAX_BOXES < u8::MAX as usize);
/// Group size from which a child's group cost is repaired from the parent's
/// duals with one augment instead of re-solved. Measured with bench:observe
/// at 1,000,000 states (2026-10-01), every search identical: 2 took 6.8%
/// more summed search time than 3, 4 was within noise (0.9% less), and
/// never repairing took 34% more.
const REPAIR_CROSSOVER: usize = 3;
// Each label group owns one bit of a cell's dead mask, and there are at most
// as many groups as boxes.
const _: () = assert!(MAX_BOXES <= u32::BITS as usize);

/// Per-goal reverse-push distances plus per-label assignment with duals.
pub(crate) struct Heuristic {
    /// Spot-major push distances: `distances[spot * goals + goal]` is the
    /// distance from `spot` to goal column `goal`, `NONE` when unreachable.
    distances: Vec<u16>,
    /// The keeper sides, when the estimate counts them.
    keeper: Option<KeeperSides>,
    goals: usize,
    /// Each goal column's cell, `NONE` past the last column.
    goal_cells: [Cell; MAX_BOXES],
    /// Boxes are grouped by label into contiguous index ranges.
    groups: Vec<Group>,
    /// The index into `groups` of each box's label group.
    group_of: [u8; MAX_BOXES],
    /// Per cell, bit `g` set when a lone box of group `g` there reaches none
    /// of the group's goals even with the keeper free to stand anywhere
    /// before each push: a box of that label there makes every estimate
    /// `None`.
    dead: Vec<u32>,
}
/// What the estimate needs to tell a box's sides apart. Exact search counts
/// them; the weighted policies do not (see `Policy::keeper_sides`).
struct KeeperSides {
    /// Which side of each cell holds each neighbor and the keeper, built
    /// from the start keeper.
    blocks: Blocks,
    /// Per spot, bit `g` set when its distance to every goal of group `g`
    /// is `NONE`: a box of that label there, with the keeper on that side,
    /// makes the estimate `None`. On every side of a cell it holds the
    /// cell's `dead` bits, and maybe more.
    dead_spots: Vec<u32>,
    /// Per cell, the walk a keeper there makes before it stands where a box
    /// can: see `charges`.
    charge: Vec<u8>,
    /// The largest charge.
    most: u32,
}
/// A label group's boxes are indices `start..start + len`. Goal columns
/// follow the same order, so the group's goals are the same column range.
struct Group {
    start: usize,
    len: usize,
}

impl Group {
    /// The group's box slots, which are also its goal columns.
    fn range(&self) -> Range<usize> {
        self.start..self.start + self.len
    }
}

/// Hungarian working state for one group: 1-based potentials and matching,
/// with row and column 0 as the dummies the augment starts from.
#[derive(Clone, Copy)]
struct Duals {
    u: [i32; MAX_BOXES + 1],
    v: [i32; MAX_BOXES + 1],
    /// Row matched to each column, 0 when the column is free.
    p: [u8; MAX_BOXES + 1],
    /// Previous column on the augmenting path.
    way: [u8; MAX_BOXES + 1],
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
    /// The parent's spot of each box in the group. A push leaves every
    /// other box's spot as it was, so its children share them.
    spots: [Spot; MAX_BOXES],
}

impl ParentGroup {
    pub(crate) const EMPTY: Self = Self {
        group: usize::MAX,
        cost: 0,
        duals: Duals::EMPTY,
        spots: [0; MAX_BOXES],
    };
}

/// The push that leaves a box on `cell`, with the keeper on `side` of it,
/// moved opposite to direction `d`: the spot the box came from and the
/// stand the keeper pushed from, or `None` when there is no such push. The
/// box came from `previous`, pushed by a keeper that started on the stand
/// past `previous` and ended on `previous`, so on the side of `cell`
/// holding `previous`. Without `blocks` a spot is a cell and `side` is 0.
#[inline]
fn reverse_push(
    neighbors: &[[Cell; 4]],
    blocks: Option<&Blocks>,
    cell: usize,
    side: usize,
    d: usize,
) -> Option<(Spot, Cell)> {
    let previous = neighbors[cell][d];
    if previous == NONE || neighbors[previous as usize][d] == NONE {
        return None;
    }
    let stand = neighbors[previous as usize][d];
    let before = match blocks {
        None => previous,
        Some(blocks) if usize::from(blocks.toward(cell as Cell, d)) == side => {
            (previous as usize * SIDES + usize::from(blocks.toward(previous, d))) as Spot
        }
        Some(_) => return None,
    };
    Some((before, stand))
}

/// Reverse-push distances, spot-major per goal column: the least cost of
/// the pushes a lone box needs from each spot, `NONE` when unreachable.
/// Between two pushes of a box the keeper only walks around it, so it stays
/// on one side of the box, and it can push the box only from a stand on
/// that side. So with keeper sides, a push in direction `d` from `from` to
/// `to` moves the box from the spot of `from` on the side holding the stand
/// to the spot of `to` on the side holding `from`, and a goal cell is 0 on
/// every side; a push costs one plus its stand's charge (see `charges`),
/// and a distance is capped at `MAX_DISTANCE`. Without, the keeper may
/// stand anywhere, a spot is a cell, and a push costs one.
///
/// Other boxes keep their spots through a push: the keeper moves from the
/// stand to `from`, neighbors and neither one their cell. So such a push
/// gives d(before) <= d(after) + its cost for the pushed box's spots and
/// leaves every other box's entries as they were. An entry drops by at most
/// a push's cost per push, though it may rise by any amount or become
/// `NONE`, and walks change no spot. A min-cost matching over such entries,
/// less the keeper's charge, therefore drops by at most one per move: a
/// walk changes the charge by at most one, and a push ends with the keeper
/// on its box's old cell, where a box can stand, so at charge 0. That is
/// consistent in moves, and without keeper sides every charge is 0.
fn push_distances(
    board: &Board,
    sides: Option<(&Blocks, &[u8])>,
    columns: &[(Cell, u8)],
) -> Vec<u16> {
    let neighbors = board.neighbors();
    let goals = columns.len();
    let stride = if sides.is_some() { SIDES } else { 1 };
    let blocks = sides.map(|(blocks, _)| blocks);
    let cost = |stand: Cell| 1 + sides.map_or(0, |(_, charge)| usize::from(charge[stand as usize]));
    let most = sides.map_or(0, |(_, charge)| charge.iter().copied().max().unwrap_or(0));
    let mut distances = vec![NONE; neighbors.len() * stride * goals];
    // Dial's buckets, reused for every column: a push costs 1 to
    // 1 + `most`, so the spots waiting at once span fewer distances than
    // there are buckets. Spots in one bucket share a distance, so their
    // order does not matter.
    let span = usize::from(most) + 2;
    let mut buckets: Vec<Vec<Spot>> = vec![Vec::new(); span];
    for (column, &(goal, _)) in columns.iter().enumerate() {
        let at = |spot: Spot| spot as usize * goals + column;
        for side in 0..stride {
            let spot = (goal as usize * stride + side) as Spot;
            distances[at(spot)] = 0;
            buckets[0].push(spot);
        }
        let mut waiting = stride;
        let mut distance = 0;
        while waiting > 0 {
            let bucket = distance % span;
            while let Some(spot) = buckets[bucket].pop() {
                waiting -= 1;
                // Queued again since at a lower distance.
                if usize::from(distances[at(spot)]) != distance {
                    continue;
                }
                let (cell, side) = (spot as usize / stride, spot as usize % stride);
                for d in 0..OPPOSITE.len() {
                    let Some((before, stand)) = reverse_push(neighbors, blocks, cell, side, d)
                    else {
                        continue;
                    };
                    // Never below `distance`: a capped one may join the
                    // bucket this loop is still draining.
                    let next = (distance + cost(stand)).min(usize::from(MAX_DISTANCE)) as u16;
                    if next < distances[at(before)] {
                        distances[at(before)] = next;
                        buckets[usize::from(next) % span].push(before);
                        waiting += 1;
                    }
                }
            }
            distance += 1;
        }
    }
    distances
}

/// Per spot, bit `g` clear when a lone box of group `g` reaches one of the
/// group's goals by pushes: the reverse-push flood from the group's goals,
/// over the spots of `push_distances`. Every other bit stays set, those of
/// groups the board lacks too. Without `blocks` a spot is a cell and the
/// keeper may stand anywhere: deadlock, corral and stage checks read that
/// table with no keeper side at hand.
fn dead_masks(
    board: &Board,
    blocks: Option<&Blocks>,
    groups: &[Group],
    columns: &[(Cell, u8)],
) -> Vec<u32> {
    let neighbors = board.neighbors();
    let stride = if blocks.is_some() { SIDES } else { 1 };
    let mut dead = vec![u32::MAX; neighbors.len() * stride];
    let mut stack = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        let bit = 1u32 << g;
        for &(goal, _) in &columns[group.range()] {
            for side in 0..stride {
                let spot = goal as usize * stride + side;
                dead[spot] &= !bit;
                stack.push(spot);
            }
        }
        while let Some(spot) = stack.pop() {
            let (cell, side) = (spot / stride, spot % stride);
            for d in 0..OPPOSITE.len() {
                if let Some((before, _)) = reverse_push(neighbors, blocks, cell, side, d)
                    && dead[before as usize] & bit != 0
                {
                    dead[before as usize] &= !bit;
                    stack.push(before as usize);
                }
            }
        }
    }
    dead
}

/// Per cell, the fewest steps from it to a cell a box can stand on: one
/// with a spot in `dead_spots` that is live for some group. No state on a
/// route has a box anywhere else, since its estimate would be `None`, and a
/// push leaves the keeper on the cell its box left. So the keeper enters
/// any other cell only by walking, and before it pushes from stand `s` it
/// walks at least `charge(s)` steps since it last stood where a box can.
/// Those walks are disjoint for distinct pushes, as every push ends where a
/// box can stand. Capped at `u8::MAX`, and 0 where no such cell is
/// reachable: still a lower bound, and changed by at most one per step.
fn charges(board: &Board, dead_spots: &[u32]) -> Vec<u8> {
    let neighbors = board.neighbors();
    let mut steps = vec![u32::MAX; neighbors.len()];
    let mut queue: Vec<Cell> = Vec::with_capacity(neighbors.len());
    for (cell, masks) in dead_spots.as_chunks::<SIDES>().0.iter().enumerate() {
        if masks.iter().any(|&mask| mask != u32::MAX) {
            steps[cell] = 0;
            queue.push(cell as Cell);
        }
    }
    let mut head = 0;
    while head < queue.len() {
        let cell = queue[head] as usize;
        head += 1;
        for &next in &neighbors[cell] {
            if next != NONE && steps[next as usize] == u32::MAX {
                steps[next as usize] = steps[cell] + 1;
                queue.push(next);
            }
        }
    }
    steps
        .iter()
        .map(|&steps| match steps {
            u32::MAX => 0,
            steps => steps.min(u8::MAX.into()) as u8,
        })
        .collect()
}

impl Heuristic {
    /// The cell's dead mask and one u16 push distance per goal column; there
    /// is one goal per box. With keeper sides, the sides, the charge, and per
    /// side a dead mask and the distances.
    pub(crate) const fn bytes_per_cell(boxes: usize, keeper_sides: bool) -> usize {
        let distances = boxes * size_of::<u16>();
        if keeper_sides {
            Blocks::bytes_for(1)
                + size_of::<u32>()
                + size_of::<u8>()
                + SIDES * (size_of::<u32>() + distances)
        } else {
            size_of::<u32>() + distances
        }
    }
    /// The estimator over the reverse-push distance table, so its estimates
    /// less the keeper's [`charge`](Self::charge) are admissible and
    /// consistent, with or without keeper sides. It counts them given
    /// `keeper`, the start keeper's cell, which every estimated state's
    /// keeper shares a floor component with.
    pub(crate) fn new(board: &Board, keeper: Option<Cell>) -> Self {
        let mut groups = Vec::new();
        let mut group_of = [0; MAX_BOXES];
        let mut start = 0;
        for run in board.labels().chunk_by(|a, b| a == b) {
            group_of[start..start + run.len()].fill(groups.len() as u8);
            groups.push(Group {
                start,
                len: run.len(),
            });
            start += run.len();
        }
        // Labels are sorted and every label has as many goals as boxes, so a
        // stable sort by label lays the goal columns out in group order.
        let mut columns = board.goals().to_vec();
        columns.sort_by_key(|&(_, label)| label);
        let goals = columns.len();
        let mut goal_cells = [NONE; MAX_BOXES];
        for (slot, &(cell, _)) in goal_cells.iter_mut().zip(&columns) {
            *slot = cell;
        }
        let blocks = keeper.map(|player| {
            let cells = board.tiles().len();
            let (mut stack, mut low, mut next) = (vec![0; cells], vec![0; cells], vec![0; cells]);
            // Fatal when refused, like the tables' own allocations here.
            Blocks::build(board, player, &mut stack, &mut low, &mut next)
                .expect("the heuristic's keeper sides")
        });
        let keeper = blocks.map(|blocks| {
            let dead_spots = dead_masks(board, Some(&blocks), &groups, &columns);
            let charge = charges(board, &dead_spots);
            let most = charge.iter().copied().max().map_or(0, u32::from);
            KeeperSides {
                blocks,
                dead_spots,
                charge,
                most,
            }
        });
        let distances = push_distances(
            board,
            keeper
                .as_ref()
                .map(|keeper| (&keeper.blocks, &keeper.charge[..])),
            &columns,
        );
        let dead = dead_masks(board, None, &groups, &columns);
        Self {
            distances,
            keeper,
            goals,
            goal_cells,
            groups,
            group_of,
            dead,
        }
    }

    /// Whether box `i` on `cell` has no reachable goal of its label, even
    /// with the keeper free to stand anywhere before each push. Then the
    /// box's group has no finite matching, so `estimate` and
    /// `child_estimate` would return `None`, and callers may prune first.
    pub(crate) fn dead(&self, i: usize, cell: Cell) -> bool {
        (self.dead[cell as usize] >> self.group_of[i]) & 1 != 0
    }

    /// Whether box `i`, pushed in direction `d` onto `to`, has no reachable
    /// goal of its label from there: then `child_estimate` of that push
    /// returns `None`. It holds wherever `dead(i, to)` does, and with keeper
    /// sides also where every goal needs a push from a side the keeper can
    /// no longer reach.
    pub(crate) fn dead_after(&self, i: usize, to: Cell, d: usize) -> bool {
        let mask = match &self.keeper {
            Some(keeper) => keeper.dead_spots[self.pushed(to, d) as usize],
            None => self.dead[to as usize],
        };
        (mask >> self.group_of[i]) & 1 != 0
    }

    /// The spot of a box on `cell` with the keeper on `player`. A box
    /// outside the keeper's floor component never moves, so its spot need
    /// only stay fixed: on a goal it is 0 on every side, and off one no
    /// route exists.
    fn spot(&self, cell: Cell, player: Cell) -> Spot {
        match &self.keeper {
            Some(KeeperSides { blocks, .. }) => {
                let side = if blocks.in_keeper(cell) {
                    blocks.side(cell, player)
                } else {
                    0
                };
                (cell as usize * SIDES + usize::from(side)) as Spot
            }
            None => cell,
        }
    }

    /// The spot of a box pushed in direction `d` onto `to`: the keeper is on
    /// the cell it came from.
    fn pushed(&self, to: Cell, d: usize) -> Spot {
        match &self.keeper {
            Some(KeeperSides { blocks, .. }) => {
                (to as usize * SIDES + usize::from(blocks.toward(to, OPPOSITE[d]))) as Spot
            }
            None => to,
        }
    }

    /// The walk a keeper on `cell` makes before it stands where a box can,
    /// which the estimate counts toward the next push from there (see
    /// `charges`); 0 without keeper sides, and on every cell a push leaves
    /// the keeper on.
    #[inline]
    pub(crate) fn charge(&self, cell: Cell) -> u32 {
        match &self.keeper {
            Some(keeper) => keeper.charge[cell as usize].into(),
            None => 0,
        }
    }

    /// The largest [`charge`](Self::charge) of any cell.
    pub(crate) fn most_charge(&self) -> u32 {
        self.keeper.as_ref().map_or(0, |keeper| keeper.most)
    }

    /// Box `i`'s label group as a range of box slots: `i` and every box
    /// interchangeable with it, the run of equal labels that
    /// [`canonicalize`](crate::push::canonicalize) sorts.
    pub(crate) fn group(&self, i: usize) -> Range<usize> {
        self.groups[self.group_of[i] as usize].range()
    }

    /// Each goal column's cell, in column order. Goal columns follow the box
    /// slots, so column `t` is a goal of box `t`'s label, and `group(i)` is
    /// also the range of columns holding box `i`'s goals.
    pub(crate) fn goal_cells(&self) -> &[Cell] {
        &self.goal_cells[..self.goals]
    }

    /// The fewest pushes a lone box on `cell` needs to reach goal column
    /// `column`, the keeper standing anywhere, `NONE` when it cannot: an
    /// entry the matching sums, which drops by at most one per push. Only a
    /// heuristic that ignores keeper sides keeps distances per cell, and no
    /// policy that counts them polishes (a const assert in engine.rs).
    pub(crate) fn distance(&self, cell: Cell, column: usize) -> u16 {
        debug_assert!(self.keeper.is_none());
        self.distances[cell as usize * self.goals + column]
    }

    /// Distances from `spot` to each of `group`'s goals, in column order.
    fn goal_distances(&self, group: &Group, spot: Spot) -> &[u16] {
        let at = spot as usize * self.goals + group.start;
        &self.distances[at..at + group.len]
    }

    /// Minimum-cost label-compatible matching over relaxed push distances.
    /// Less the keeper's [`charge`](Self::charge), admissible for total
    /// moves: the relaxation removes every other box. That charge is 0 in
    /// every state a push leaves, which is every queued state but the root.
    /// It is the sum of independent per-group optima, `None` when some group
    /// has no perfect matching over reachable goals.
    pub(crate) fn estimate(&self, state: &State) -> Option<u32> {
        let mut spots = [0; MAX_BOXES];
        for (spot, &cell) in spots.iter_mut().zip(&state.boxes[..self.goals]) {
            *spot = self.spot(cell, state.player);
        }
        let mut total = 0u32;
        for group in &self.groups {
            total += self.solve(group, &spots[group.range()])?.0;
        }
        Some(total)
    }

    /// `estimate` of the child where box `i` of `parent` was pushed in
    /// direction `d` onto `to`, given the parent's own estimate `parent_h`.
    /// A push changes only box `i`'s spot, so the child's total is the
    /// parent's with that group's cost swapped: equal to a fresh `estimate`
    /// of the child, because the optimal cost is unique. `cache` holds the
    /// parent's spots and solution of the last group asked for and is
    /// re-solved only when the group changes. `parent` must be a queued
    /// state: this panics when its own group has no assignment.
    pub(crate) fn child_estimate(
        &self,
        parent_h: u32,
        cache: &mut ParentGroup,
        parent: &State,
        i: usize,
        to: Cell,
        d: usize,
    ) -> Option<u32> {
        let g = self.group_of[i] as usize;
        let group = &self.groups[g];
        let range = group.range();
        if cache.group != g {
            for j in range.clone() {
                cache.spots[j] = self.spot(parent.boxes[j], parent.player);
            }
            // Never `None`: every group of a queued state is feasible.
            let (cost, duals) = self
                .solve(group, &cache.spots[range.clone()])
                .expect("queued state has an assignment");
            cache.group = g;
            cache.cost = cost;
            cache.duals = duals;
        }
        let mut spots = cache.spots;
        spots[i] = self.pushed(to, d);
        let spots = &spots[range];
        let cost = if group.len < REPAIR_CROSSOVER {
            self.solve(group, spots)?.0
        } else {
            self.repair(group, spots, i - group.start, cache)?
        };
        Some(parent_h - cache.cost + cost)
    }

    /// Deterministic Hungarian over one label group: its optimal cost with
    /// the final duals, `None` when no perfect matching exists.
    fn solve(&self, group: &Group, spots: &[Spot]) -> Option<(u32, Duals)> {
        // A one-box group needs no Hungarian: one distance is the assignment.
        if group.len == 1 {
            let cost = cost(self.goal_distances(group, spots[0])[0]);
            return (cost < INF).then_some((cost as u32, Duals::EMPTY));
        }
        let mut duals = Duals::EMPTY;
        for row in 1..=group.len {
            if !self.augment(group, spots, row, &mut duals) {
                return None;
            }
        }
        Some((self.matched_cost(group, spots, &duals)?, duals))
    }

    /// One-row repair of the parent's optimal solution after only 0-based
    /// `row` moved, to `spots[row]`. Freeing the row's column and zeroing its
    /// potential keeps the duals feasible (every `v <= 0 <= cost`), so one
    /// augmenting path from that row restores an optimal matching; a missing
    /// finite column proves no matching exists.
    ///
    /// The path ends at its first step when the row's least reduced cost is
    /// on its own column, the only free one. That holds whenever the push
    /// took the box along a cheapest path to its matched goal: no distance
    /// drops by more than the push's cost, so no other reduced cost falls
    /// below the row's old potential less that cost. Then the cost follows
    /// from the duals, with no copy, augment or rescan.
    fn repair(
        &self,
        group: &Group,
        spots: &[Spot],
        row: usize,
        parent: &ParentGroup,
    ) -> Option<u32> {
        let duals = &parent.duals;
        let row = row + 1;
        let column = (1..=group.len)
            .find(|&j| duals.p[j] as usize == row)
            .expect("the parent's matching is complete");
        let distances = self.goal_distances(group, spots[row - 1]);
        let reduced = |j: usize| cost(distances[j - 1]) - duals.v[j];
        let own = reduced(column);
        if (1..=group.len).all(|j| reduced(j) >= own) {
            // Raising the row's potential to `own` keeps the duals feasible
            // and makes its edge to `column` tight, so the parent's matching
            // is optimal again. Only that edge's cost changed: it was
            // u[row] + v[column], tight in the parent, and is now
            // own + v[column]. It is at most the total, so an unreachable
            // goal still lands the total at INF or more.
            let total = parent.cost as i32 - duals.u[row] + own;
            return (total < INF).then_some(total as u32);
        }
        let mut state = *duals;
        state.u[row] = 0;
        state.p[column] = 0;
        if !self.augment(group, spots, row, &mut state) {
            return None;
        }
        self.matched_cost(group, spots, &state)
    }

    /// One Hungarian augmenting path from 1-based `row`, which must be
    /// unmatched, over feasible potentials in `state`. Returns false when no
    /// finite column is reachable: then no perfect matching exists.
    fn augment(&self, group: &Group, spots: &[Spot], row: usize, state: &mut Duals) -> bool {
        let n = group.len;
        let Duals { u, v, p, way } = state;
        p[0] = row as u8;
        let mut minv = [INF; MAX_BOXES + 1];
        let mut used = [false; MAX_BOXES + 1];
        let mut j0 = 0;
        loop {
            used[j0] = true;
            let i0 = p[j0] as usize;
            let distances = self.goal_distances(group, spots[i0 - 1]);
            let mut delta = INF;
            let mut j1 = 0;
            for j in 1..=n {
                if used[j] {
                    continue;
                }
                let reduced = cost(distances[j - 1]) - u[i0] - v[j];
                if reduced < minv[j] {
                    minv[j] = reduced;
                    way[j] = j0 as u8;
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
                    u[p[j] as usize] += delta;
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
            let j1 = way[j0] as usize;
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
    fn matched_cost(&self, group: &Group, spots: &[Spot], state: &Duals) -> Option<u32> {
        let mut total = 0i32;
        for j in 1..=group.len {
            total += cost(self.goal_distances(group, spots[state.p[j] as usize - 1])[j - 1]);
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
    use crate::push::canonicalize;
    use crate::reach::Reach;
    use crate::testkit::{Lcg, catalog, explored_catalog, remaining};
    use sokomind_core::{Board, Cell, NONE, OPPOSITE, WALL};
    use std::{collections::VecDeque, mem::size_of};

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

    /// Walks `steps` random feasible states of `board`, restarting every 25,
    /// once without keeper sides and once with. Makes every push the keeper
    /// can reach, box-major with one cache per parent as the engine does,
    /// and checks the incremental child estimate against a fresh one.
    /// Tallies `[feasible, infeasible]` per path: singleton, re-solve, dual
    /// repair.
    fn walk(board: &Board, steps: usize, rng: &mut Lcg, counts: &mut [[u32; 2]; 3]) {
        for keeper in [None, Some(board.initial().player)] {
            walk_with(&Heuristic::new(board, keeper), board, steps, rng, counts);
        }
    }

    fn walk_with(
        heuristic: &Heuristic,
        board: &Board,
        steps: usize,
        rng: &mut Lcg,
        counts: &mut [[u32; 2]; 3],
    ) {
        let boxes = board.labels().len();
        let neighbors = board.neighbors();
        let mut reach = Reach::new(neighbors.len());
        let mut state = board.initial();
        for step in 0..steps {
            if step.is_multiple_of(25) {
                state = board.initial();
            }
            let parent_h = heuristic.estimate(&state).unwrap();
            let mut cache = ParentGroup::EMPTY;
            reach.fill(board, &state);
            let mut options = Vec::new();
            for i in 0..boxes {
                let len = heuristic.group(i).len();
                let path = if len == 1 {
                    0
                } else if len < REPAIR_CROSSOVER {
                    1
                } else {
                    2
                };
                let from = state.boxes[i];
                for (d, &to) in neighbors[from as usize].iter().enumerate() {
                    let stand = neighbors[from as usize][OPPOSITE[d]];
                    if to == NONE
                        || stand == NONE
                        || reach.blocked(to)
                        || reach.distance(stand) == NONE
                    {
                        continue;
                    }
                    let mut child = state;
                    child.boxes[i] = to;
                    child.player = from;
                    canonicalize(board, &mut child);
                    let fresh = heuristic.estimate(&child);
                    let incremental =
                        heuristic.child_estimate(parent_h, &mut cache, &state, i, to, d);
                    assert_eq!(incremental, fresh, "{state:?} box {i} to {to}");
                    counts[path][fresh.is_none() as usize] += 1;
                    if fresh.is_some() {
                        options.push(child);
                    }
                }
            }
            state = if options.is_empty() {
                board.initial()
            } else {
                options[rng.below(options.len())]
            };
        }
    }

    /// The stage probe's `pull` with nothing blocked: the cells from which
    /// one box reaches `goal` by pushes, the keeper ignored. A cell joins
    /// when the cell beyond it, where the keeper stands to push it toward
    /// `goal`'s side, is floor.
    fn pull(board: &Board, goal: Cell) -> Vec<bool> {
        let neighbors = board.neighbors();
        let mut seen = vec![false; neighbors.len()];
        seen[goal as usize] = true;
        let mut queue = VecDeque::from([goal]);
        while let Some(x) = queue.pop_front() {
            for (d, &p) in neighbors[x as usize].iter().enumerate() {
                if p == NONE || seen[p as usize] || neighbors[p as usize][d] == NONE {
                    continue;
                }
                seen[p as usize] = true;
                queue.push_back(p);
            }
        }
        seen
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
        assert_eq!(board.labels(), [b'A', b'B']);
        let heuristic = Heuristic::new(&board, None);
        let at = |x: usize, y: usize| (y * board.width() + x) as Cell;
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

    /// On every catalog board and for every label group, `dead` holds on
    /// exactly the floor cells the probe's `pull` from no goal of the label
    /// reaches, so the stage ladder's dead cells and these are one table.
    /// Walls differ only in representation: the flood never reaches them,
    /// so `dead` marks them dead for every group where the probe leaves them
    /// out, and no caller asks about a wall. `goal_cells` lists, per column
    /// `t`, a goal of box `t`'s label, each goal once. Neither depends on
    /// keeper sides. With them, each spot's dead mask marks exactly the
    /// groups whose goals are all `NONE` from it, so `dead_after` prunes no
    /// child the estimate keeps.
    #[test]
    fn dead_matches_pull() {
        for (id, board) in catalog() {
            let sides = Heuristic::new(&board, Some(board.initial().player));
            let keeper = sides.keeper.as_ref().unwrap();
            for (spot, row) in sides.distances.chunks_exact(sides.goals).enumerate() {
                for (g, group) in sides.groups.iter().enumerate() {
                    let unreachable = row[group.range()].iter().all(|&d| d == NONE);
                    let dead = (keeper.dead_spots[spot] >> g) & 1 != 0;
                    assert_eq!(dead, unreachable, "{id}: group {g} at spot {spot}");
                }
            }
            let heuristic = Heuristic::new(&board, None);
            let labels = board.labels();
            let goal_cells = heuristic.goal_cells();
            assert_eq!(goal_cells.len(), labels.len(), "{id}");
            for (t, &cell) in goal_cells.iter().enumerate() {
                assert!(board.on_goal(t, cell), "{id}: column {t} on {cell}");
                assert!(!goal_cells[..t].contains(&cell), "{id}: {cell} twice");
            }
            for start in (0..labels.len()).filter(|&i| heuristic.group(i).start == i) {
                let mut live = vec![false; board.tiles().len()];
                for &(goal, label) in board.goals() {
                    if label == labels[start] {
                        for (alive, reached) in live.iter_mut().zip(pull(&board, goal)) {
                            *alive |= reached;
                        }
                    }
                }
                for (cell, &tile) in board.tiles().iter().enumerate() {
                    let dead = heuristic.dead(start, cell as Cell);
                    if tile == WALL {
                        assert!(dead, "{id}: wall {cell} live for box {start}");
                    } else {
                        assert_eq!(dead, !live[cell], "{id}: box {start} on {cell}");
                    }
                }
            }
        }
    }

    /// The arena accounts exactly the distance table and the dead masks,
    /// plus with keeper sides the per-spot dead masks, the charges and the
    /// blocks.
    #[test]
    fn bytes_per_cell_matches_the_tables() {
        let board = Board::parse(WIDE).unwrap();
        let cells = board.tiles().len();
        for keeper in [None, Some(board.initial().player)] {
            let h = Heuristic::new(&board, keeper);
            let sides = h.keeper.as_ref().map_or(0, |keeper| {
                keeper.dead_spots.len() * size_of::<u32>()
                    + keeper.charge.len() * size_of::<u8>()
                    + keeper.blocks.heap_bytes()
            });
            assert_eq!(
                h.distances.len() * size_of::<u16>() + h.dead.len() * size_of::<u32>() + sides,
                cells * Heuristic::bytes_per_cell(board.labels().len(), keeper.is_some())
            );
        }
    }

    /// The incremental child estimate equals a fresh estimate for label
    /// groups of 1 through 8 boxes and WIDE's 9, on every path, including
    /// children with no perfect matching. The open rooms give each cell one
    /// side; the catalog's corridors give cells several.
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
        for (_, board) in catalog() {
            walk(&board, 50, &mut rng, &mut counts);
        }
        assert!(counts.iter().flatten().all(|&n| n > 0), "{counts:?}");
    }

    /// Against exact remaining moves from every primitive state of each
    /// catalog board small enough to enumerate, with keeper sides and
    /// without: the estimate less the keeper's charge is admissible, `None`
    /// only where no solution exists, and consistent: a move lowers it by at
    /// most one.
    #[test]
    fn heuristic_is_admissible_and_consistent() {
        for (id, board, states, edges) in explored_catalog() {
            let exact = remaining(board, states, edges);
            for keeper in [None, Some(board.initial().player)] {
                let heuristic = Heuristic::new(board, keeper);
                let estimates: Vec<_> = states
                    .iter()
                    .map(|state| {
                        let h = heuristic.estimate(state)?;
                        Some(h.saturating_sub(heuristic.charge(state.player)))
                    })
                    .collect();
                for (from, state) in states.iter().enumerate() {
                    let h = estimates[from];
                    match h {
                        Some(h) => {
                            assert!(h <= exact[from], "{id}: {h} > {} at {state:?}", exact[from])
                        }
                        None => {
                            assert_eq!(exact[from], u32::MAX, "{id}: no assignment at {state:?}")
                        }
                    }
                    for &(to, _) in &edges[from] {
                        let Some(next) = estimates[to] else {
                            continue;
                        };
                        assert!(
                            h.is_some_and(|h| h <= next + 1),
                            "{id}: {h:?} then {next} at {state:?}"
                        );
                    }
                }
            }
        }
    }
}
