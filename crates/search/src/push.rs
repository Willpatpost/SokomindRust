//! Child building: what turns a popped parent and one push of one of its
//! boxes into a child or a counted prune. This module owns the canonical
//! box order ([`canonicalize`], [`settle`]), the stale-entry check
//! ([`Parent::of`]), the expand prologue ([`Parts::open`]), the static
//! legality check ([`legal`]), the admit chain in its prune order
//! ([`admit`]), the queue key's f ([`key`]), the bound on the keeper's
//! walk to a state's first push ([`stand_walk`], [`root_estimate`]), and
//! the [`SkipCounters`] its prunes share with the engine's own skips.
//!
//! It is separate from the engine so that the planned stage search can
//! expand a node through the same [`Parts`], lent to it, as the engine
//! does, prune for prune and counter for counter, without importing the
//! engine. The expand prologue, including the sealed-corral check on the
//! whole parent, is [`Parts::open`], which every search runs before its
//! pushes; each push then goes through [`legal`] and [`admit`]. The
//! engine's expansion runs this same code, so the exact search proves with
//! it: nothing here reads the policy or the incumbent or touches the queue,
//! only the bound, the reopen flag and the weight each call passes in.
use crate::{
    arena::{Arena, Key, Node},
    corral::Corral,
    deadlock::{ALL_BOXES, Deadlock},
    heuristic::{Heuristic, ParentGroup},
    reach::Reach,
};
use sokomind_core::{Board, Cell, NONE, OPPOSITE, State};
use std::ops::Range;

/// Sorts interchangeable same-label boxes so states compare canonically.
/// Labels are grouped in slot order (see [`State`]), so sorting each run of
/// equal labels keeps every box on a slot of its own label. Only call this
/// on search-owned state copies: a live `Game` must keep its box order,
/// because undo records box indices. The engine sorts only its start this
/// way; after each push [`settle`] restores the order.
pub(crate) fn canonicalize(board: &Board, state: &mut State) {
    let labels = board.labels();
    let mut begin = 0;
    while begin < labels.len() {
        let mut end = begin + 1;
        while end < labels.len() && labels[end] == labels[begin] {
            end += 1;
        }
        state.boxes[begin..end].sort_unstable();
        begin = end;
    }
}

/// Restores [`canonicalize`]'s order in a state that had it until box `i`
/// alone moved. `group` is the box's label group ([`Heuristic::group`]):
/// the box shifts left or right within it, one neighbor at a time, until
/// the group is sorted again. The other boxes kept their sorted order and
/// no two boxes share a cell, so the result is exactly the full sort's, at
/// the cost of the boxes it passes instead of a sort of every group.
fn settle(state: &mut State, group: Range<usize>, i: usize) {
    let cell = state.boxes[i];
    let mut slot = i;
    while slot > group.start && state.boxes[slot - 1] > cell {
        state.boxes[slot] = state.boxes[slot - 1];
        slot -= 1;
    }
    // After a shift left the right neighbor is a box just passed, above
    // `cell`, so this shifts only a box that did not move left.
    while slot + 1 < group.end && state.boxes[slot + 1] < cell {
        state.boxes[slot] = state.boxes[slot + 1];
        slot += 1;
    }
    state.boxes[slot] = cell;
}

/// What the engine counts as it skips pops and children: pops whose node a
/// cheaper duplicate superseded, and pops and children a prune rejected.
/// Named for skips rather than prunes because a stale pop is no prune: its
/// node was replaced, not rejected. Each field is the
/// [`SearchStats`](crate::SearchStats) counter of the same name; the
/// arena's [`TableCounters`](crate::arena::TableCounters) holds the rest.
/// [`admit`] raises the child counters through its [`Kernel`] and
/// [`Parts::open`] the expansion's; the engine raises the pop ones itself.
#[derive(Default)]
pub(crate) struct SkipCounters {
    pub(crate) stale_pops: u32,
    pub(crate) pruned_dead_cells: u64,
    pub(crate) pruned_deadlocks: u64,
    pub(crate) pruned_duplicates: u64,
    pub(crate) pruned_assignment: u64,
    pub(crate) pruned_bound: u64,
    pub(crate) pruned_corrals: u64,
}

/// Which dead-state detectors run: those in the admit chain and the
/// sealed-corral check at expansion. A flag turns one rule off wherever it
/// applies, so `dead_pair` also reaches the corral check's own freeze.
/// Production always uses [`Prunes::ALL`]. The value lives only on the
/// engine and the [`Parts`] it lends, never in the policy, the public mode
/// or anything else a caller sees, so no caller can turn a detector off and
/// none of this is an API or ABI change. Only the test-only
/// `Engine::set_prunes` installs another value, for the differential test
/// in the engine's tests that checks a detector never changes a live run;
/// each new detector adds a flag here and a row to that test's `TOGGLES`.
///
/// A flag no detector reads yet carries `#[expect(dead_code)]` rather than
/// `allow`: the first read leaves the expectation unfulfilled, which fails
/// the clippy gate until the attribute goes.
#[derive(Clone, Copy)]
pub(crate) struct Prunes {
    /// The freeze rule's dead-pair axis case: an axis also holds a box when
    /// both its neighbors on it are dead cells for the box's label.
    pub(crate) dead_pair: bool,
    /// The sealed-corral check at expansion: a state with a corral that can
    /// never move again, and must, gets no children.
    pub(crate) corral: bool,
}

impl Prunes {
    /// Every detector on, the only value outside tests.
    pub(crate) const ALL: Self = Self {
        dead_pair: true,
        corral: true,
    };
}

/// A popped node about to be expanded.
pub(crate) struct Parent {
    /// Its arena id, which each child records as its parent.
    pub(crate) index: u32,
    pub(crate) node: Node,
    /// The h it was queued with.
    pub(crate) queued_h: u32,
}

impl Parent {
    /// The node a dequeued `key` names, or `None` when the entry is stale:
    /// a cheaper duplicate has since taken over its state's slot. The
    /// caller counts a stale entry (`stale_pops`).
    #[inline]
    pub(crate) fn of(arena: &Arena, key: Key) -> Option<Self> {
        let (index, queued_h) = (key.id(), key.h());
        let superseded = arena.is_superseded(index);
        debug_assert_eq!(
            superseded,
            arena.find(&arena.node(index).state).1 != Some(index)
        );
        if superseded {
            return None;
        }
        Some(Self {
            index,
            node: arena.node(index),
            queued_h,
        })
    }
}

/// A push of the parent's box `i` in direction `d`, from `from` to `to`,
/// with the keeper on `stand`, that passed the static check in [`legal`].
/// Only [`legal`] builds one and only [`admit`] reads it, so a caller can
/// pass on nothing but a statically legal push.
pub(crate) struct Push {
    i: usize,
    d: usize,
    from: Cell,
    to: Cell,
    stand: Cell,
}

/// A child that passed every prune in [`admit`].
pub(crate) struct Child {
    pub(crate) node: Node,
    /// Its estimate in full; `node.h` holds it only while it fits.
    pub(crate) h: u32,
    /// The table slot [`Arena::find`] returned for its state.
    pub(crate) slot: usize,
}

impl Child {
    /// Whether the child is solved. A solved state's estimate is 0, so any
    /// other `h` answers without reading the board.
    #[inline]
    pub(crate) fn is_goal(&self, board: &Board) -> bool {
        self.h == 0 && board.solved(&self.node.state)
    }
}

/// The f [`Arena::enqueue`] takes for a node at `g` with estimate `h`:
/// `g + weight * h` at the current weight, which the queue key saturates.
/// Exact, since even `u32::MAX` for all three fits a u64.
#[inline]
pub(crate) fn key(g: u32, h: u32, weight: u32) -> u64 {
    u64::from(g) + u64::from(weight) * u64::from(h)
}

/// What expanding one node borrows from the search that owns it: the
/// prologue in [`Parts::open`], then each push's [`Kernel`]. Each field is
/// a separate part of that search, so building one costs a few references.
pub(crate) struct Parts<'a> {
    pub(crate) board: &'a Board,
    pub(crate) heuristic: &'a Heuristic,
    pub(crate) reach: &'a mut Reach,
    pub(crate) deadlock: &'a mut Deadlock,
    pub(crate) corral: &'a mut Corral,
    pub(crate) arena: &'a mut Arena,
    pub(crate) prunes: Prunes,
    pub(crate) skipped: &'a mut SkipCounters,
    /// The search's count of expanded nodes, which [`Parts::open`] raises.
    pub(crate) expanded: &'a mut u32,
}

impl Parts<'_> {
    /// The expand prologue, which every search runs on a popped node before
    /// its pushes: counts and closes the node, floods its keeper into
    /// `reach` and refreshes `deadlock` to its boxes, as each [`Kernel`]
    /// expects, then returns the node's estimate. Returns `None` instead,
    /// counted in `pruned_corrals`, when the sealed-corral check finds no
    /// solution from the node, which then gets no children.
    #[inline]
    pub(crate) fn open(&mut self, parent: &Parent) -> Option<u32> {
        *self.expanded += 1;
        self.arena.close(parent.index);
        self.reach.fill(self.board, &parent.node.state);
        let boxes = &parent.node.state.boxes[..self.board.labels().len()];
        self.deadlock.refresh(boxes);
        if self.prunes.corral
            && self.corral.is_dead(
                self.board,
                boxes,
                self.reach,
                self.deadlock,
                self.heuristic,
                self.prunes.dead_pair.then_some(self.heuristic),
            )
        {
            // No child of a state without a solution has one; the node
            // stays expanded and closed, so it counts as before.
            self.skipped.pruned_corrals += 1;
            return None;
        }
        let parent_h = parent.node.known_h().unwrap_or_else(|| {
            self.heuristic
                .estimate(&parent.node.state)
                .expect("queued state has an assignment")
        });
        Some(parent_h)
    }
    /// The [`Kernel`] for one push after [`Parts::open`], reborrowed from
    /// these parts: the one place a kernel is built.
    #[inline]
    pub(crate) fn kernel(&mut self) -> Kernel<'_> {
        Kernel {
            board: self.board,
            heuristic: self.heuristic,
            reach: self.reach,
            deadlock: self.deadlock,
            arena: self.arena,
            prunes: self.prunes,
            skipped: self.skipped,
        }
    }
}

/// What [`admit`] reads and counts, lent by [`Parts::kernel`]. Every borrow
/// but the counters is shared, so admit can prune and probe the table but
/// never store. The bound and the reopen flag come in with each call
/// instead, since the search may change them between two pushes of one
/// parent.
pub(crate) struct Kernel<'a> {
    board: &'a Board,
    heuristic: &'a Heuristic,
    /// The parent's flood: walks to the stands, and the cells its boxes
    /// block. Filled for the parent before its first push.
    reach: &'a Reach,
    /// The freeze check, its occupancy refreshed to the parent's boxes
    /// before its first push.
    deadlock: &'a Deadlock,
    arena: &'a Arena,
    prunes: Prunes,
    skipped: &'a mut SkipCounters,
}

/// The static check on pushing box `i`, on `from`, in direction `d`: the
/// cell ahead is floor no box blocks, and the stand behind is floor the
/// keeper reaches, both read from the parent's flood in `reach`. Returns the
/// push for [`admit`], or `None` when it is no move at all, which counts as
/// nothing: dead cells are a counted prune in admit.
#[inline]
pub(crate) fn legal(board: &Board, reach: &Reach, i: usize, from: Cell, d: usize) -> Option<Push> {
    let to = board.neighbors()[from as usize][d];
    let stand = board.neighbors()[from as usize][OPPOSITE[d]];
    if to == NONE || stand == NONE || reach.blocked(to) || reach.distance(stand) == NONE {
        return None;
    }
    Some(Push {
        i,
        d,
        from,
        to,
        stand,
    })
}

/// Runs one push through every prune after the static check, counting
/// the first that rejects it, and builds the child when none does.
/// `parent_h` is the parent's estimate and `parent_group` its lazily
/// solved label group, shared by all of its children. A cheaper duplicate
/// of an open state passes the duplicate check, and one of a closed
/// (expanded) state only with `reopen_closed`. `best` is the incumbent's
/// moves, which every bound prune compares against; with no incumbent none
/// of them runs.
#[inline]
pub(crate) fn admit(
    kernel: &mut Kernel<'_>,
    parent: &Parent,
    parent_h: u32,
    parent_group: &mut ParentGroup,
    push: Push,
    reopen_closed: bool,
    best: Option<u32>,
) -> Option<Child> {
    let Push {
        i,
        d,
        from,
        to,
        stand,
    } = push;
    if kernel.heuristic.dead(i, to) {
        kernel.skipped.pruned_dead_cells += 1;
        return None;
    }
    let g = Node::push_g(parent.node.g, kernel.reach.distance(stand));
    if best.is_some_and(|best| g >= best) {
        kernel.skipped.pruned_bound += 1;
        return None;
    }
    let dead_pair = kernel.prunes.dead_pair.then_some(kernel.heuristic);
    if kernel
        .deadlock
        .is_dead_after_push(kernel.board, dead_pair, ALL_BOXES, from, to)
    {
        kernel.skipped.pruned_deadlocks += 1;
        return None;
    }
    // Every stored state is canonical, the parent too, so settling the
    // pushed box canonicalizes the child.
    let mut next = parent.node.state;
    next.player = from;
    next.boxes[i] = to;
    settle(&mut next, kernel.heuristic.group(i), i);
    debug_assert_eq!(next, {
        let mut sorted = next;
        canonicalize(kernel.board, &mut sorted);
        sorted
    });
    let (slot, previous) = kernel.arena.find(&next);
    let previous = previous.map(|previous| kernel.arena.meta(previous));
    if previous.is_some_and(|previous| previous.g <= g || (!reopen_closed && previous.closed)) {
        kernel.skipped.pruned_duplicates += 1;
        return None;
    }
    // A cheaper duplicate reuses the stored estimate; otherwise the
    // parent's group is solved lazily, only once a child gets this far.
    let known = previous.and_then(|previous| previous.known_h());
    let Some(h) = known.or_else(|| {
        kernel
            .heuristic
            .child_estimate(parent_h, parent_group, &parent.node.state, i, to)
    }) else {
        kernel.skipped.pruned_assignment += 1;
        return None;
    };
    if best.is_some_and(|best| g as u64 + h as u64 >= best as u64) {
        kernel.skipped.pruned_bound += 1;
        return None;
    }
    // Before its first push the child's keeper walks to the stand of a
    // statically legal push, so g + h + that walk still bounds every
    // route through the child. A prune only: queue keys and stored
    // estimates stay push-only.
    if h > 0
        && let Some(best) = best
    {
        // At least 1, since the prune above failed.
        let need = best - g - h;
        let boxes = &next.boxes[..kernel.board.labels().len()];
        // The parent's flood marks its boxes, and the pushed box left
        // `from` for `to`.
        let occupied = |cell: Cell| cell == to || (cell != from && kernel.reach.blocked(cell));
        // Pushing the same box on again starts from `from`.
        let ahead = kernel.board.neighbors()[to as usize][d];
        let onward = ahead != NONE && !occupied(ahead) && !kernel.heuristic.dead(i, ahead);
        if !onward
            && stand_walk(kernel.board, kernel.heuristic, from, boxes, occupied, need) >= need
        {
            kernel.skipped.pruned_bound += 1;
            return None;
        }
    }
    Some(Child {
        node: Node {
            state: next,
            g,
            parent: parent.index,
            direction: d as u8,
            h: Node::store_h(h),
        },
        h,
        slot,
    })
}

/// The root's estimate: its assignment's pushes plus the keeper's walk
/// to its first push, bounded by [`stand_walk`], the Manhattan
/// distance to the nearest stand of a statically legal push. Those
/// walking moves are disjoint from the counted pushes, so the sum stays
/// admissible. A root with no statically legal push has no route and
/// adds no walk. Only the root queues this walk: `admit` uses the stand
/// walk on children only as a prune, which keeps queue keys and stored
/// estimates push-only.
pub(crate) fn root_estimate(
    board: &Board,
    heuristic: &Heuristic,
    state: &State,
    pushes: u32,
) -> u32 {
    if pushes == 0 && board.solved(state) {
        return 0;
    }
    let boxes = &state.boxes[..board.labels().len()];
    let occupied = |cell: Cell| boxes.contains(&cell);
    pushes + stand_walk(board, heuristic, state.player, boxes, occupied, 1)
}

/// The Manhattan distance from `player` to the nearest stand of a
/// statically legal push, or 0 when there is none. Pushing box `j` in
/// direction `e` is statically legal when the cell ahead of it and the
/// stand behind it are floor that `occupied` leaves free, and the cell
/// ahead is not dead for the box. The first push of every route from an
/// unsolved state is one of these, made with exactly these boxes, so the
/// walk to its stand bounds the route's walking moves, which the
/// assignment's pushes do not count. Callers skip solved states.
///
/// Stops early once the answer is known to be below `enough` (0 asks for
/// the exact walk): the result is below `enough` exactly when the exact
/// walk is, and equals the exact walk otherwise. O(boxes * 4).
fn stand_walk(
    board: &Board,
    heuristic: &Heuristic,
    player: Cell,
    boxes: &[Cell],
    occupied: impl Fn(Cell) -> bool,
    enough: u32,
) -> u32 {
    let width = board.width();
    let (x, y) = (player as usize % width, player as usize / width);
    let walk =
        |cell: Cell| (x.abs_diff(cell as usize % width) + y.abs_diff(cell as usize / width)) as u32;
    let neighbors = board.neighbors();
    let mut best = u32::MAX;
    for (j, &cell) in boxes.iter().enumerate() {
        // A stand is next to its box, so at most one step closer.
        if walk(cell) > best {
            continue;
        }
        for (e, &opposite) in OPPOSITE.iter().enumerate() {
            let ahead = neighbors[cell as usize][e];
            let stand = neighbors[cell as usize][opposite];
            if ahead == NONE
                || stand == NONE
                || occupied(ahead)
                || occupied(stand)
                || heuristic.dead(j, ahead)
            {
                continue;
            }
            best = best.min(walk(stand));
            if best < enough {
                return best;
            }
        }
    }
    if best == u32::MAX { 0 } else { best }
}

#[cfg(test)]
mod tests {
    use super::{canonicalize, root_estimate, settle, stand_walk};
    use crate::{
        heuristic::Heuristic,
        reach::Reach,
        testkit::{Lcg, catalog, explored_catalog, remaining},
    };
    use sokomind_core::{Board, Cell, NONE, State};

    /// Sorting stays inside each label group: a global sort would interleave
    /// the A and B boxes, and inactive slots keep `NONE`.
    #[test]
    fn canonicalize_sorts_each_label_group() {
        let board = Board::parse("OOOOOOO\nOR    O\nOABAB O\nOaabb O\nOOOOOOO").unwrap();
        assert_eq!(board.labels(), b"AABB");
        let start = board.initial();
        let [a1, a2, b1, b2] = [0, 1, 2, 3].map(|i| start.boxes[i]);
        assert!(a1 < b1 && b1 < a2 && a2 < b2);
        let mut state = start;
        state.boxes[..4].copy_from_slice(&[a2, a1, b2, b1]);
        canonicalize(&board, &mut state);
        assert_eq!(state, start);
    }

    /// After any one-box move from a canonical state, settling the moved box
    /// gives exactly canonicalize's order. Random moves on every catalog
    /// board, which ignore the keeper since neither order reads it, reach
    /// one-box groups, boxes that keep their slot, and boxes that pass two
    /// or more others to either end of their group.
    #[test]
    fn settle_matches_canonicalize_after_every_move() {
        let mut rng = Lcg(1);
        // One-box group, same slot, to the group's start past at least two
        // boxes, to its end past at least two.
        let mut seen = [0; 4];
        for (id, board) in catalog() {
            let heuristic = Heuristic::new(&board);
            let n = board.labels().len();
            let mut state = board.initial();
            for _ in 0..1_000 {
                let i = rng.below(n);
                let to = board.neighbors()[state.boxes[i] as usize][rng.below(4)];
                if to == NONE || state.boxes[..n].contains(&to) {
                    continue;
                }
                let mut moved = state;
                moved.boxes[i] = to;
                let mut sorted = moved;
                canonicalize(&board, &mut sorted);
                let group = heuristic.group(i);
                settle(&mut moved, group.clone(), i);
                assert_eq!(moved, sorted, "{id}: box {i} to {to} in {state:?}");
                let slot = moved.boxes[..n]
                    .iter()
                    .position(|&cell| cell == to)
                    .unwrap();
                if group.len() == 1 {
                    seen[0] += 1;
                } else if slot == i {
                    seen[1] += 1;
                } else if slot == group.start && i >= slot + 2 {
                    seen[2] += 1;
                } else if slot + 1 == group.end && slot >= i + 2 {
                    seen[3] += 1;
                }
                state = moved;
            }
        }
        assert!(seen.iter().all(|&count| count > 0), "{seen:?}");
    }

    /// The exact walk over `boxes`, occupancy read from the boxes.
    fn walk(board: &Board, heuristic: &Heuristic, player: Cell, boxes: &[Cell]) -> u32 {
        let occupied = |cell: Cell| boxes.contains(&cell);
        stand_walk(board, heuristic, player, boxes, occupied, 0)
    }

    /// (h, h') with h' = h + walk, and 0 when solved; `None` without an
    /// assignment.
    fn estimates(board: &Board, heuristic: &Heuristic, state: &State) -> Option<(u32, u32)> {
        let h = heuristic.estimate(state)?;
        if h == 0 {
            return Some((0, 0));
        }
        let boxes = &state.boxes[..board.labels().len()];
        Some((h, h + walk(board, heuristic, state.player, boxes)))
    }

    /// Admissible: never above the exact remaining moves. Consistent: a
    /// move lowers h' by at most 1 wherever it lowers h by at most 1,
    /// which is every move, since the push-distance estimate is consistent.
    /// On every catalog board whose primitive state space fits the
    /// testkit's exploration cap.
    #[test]
    fn stand_walk_is_admissible_and_consistent() {
        for (id, board, states, edges) in explored_catalog() {
            let heuristic = Heuristic::new(board);
            let exact = remaining(board, states, edges);
            let values: Vec<_> = states
                .iter()
                .map(|state| estimates(board, &heuristic, state))
                .collect();
            for (from, state) in states.iter().enumerate() {
                let Some((h, value)) = values[from] else {
                    assert_eq!(exact[from], u32::MAX, "{id}: no assignment at {state:?}");
                    continue;
                };
                assert!(
                    value <= exact[from],
                    "{id}: {value} > {} at {state:?}",
                    exact[from]
                );
                for &(to, _) in &edges[from] {
                    if let Some((next_h, next)) = values[to]
                        && h <= next_h + 1
                    {
                        assert!(value <= next + 1, "{id}: {value} then {next} at {state:?}");
                    }
                }
            }
        }
    }

    /// The child prune's inputs: occupancy from the parent's flood plus
    /// the pushed box gives the child's own walk, an early stop only
    /// answers "below enough", and the onward exit only skips a walk
    /// of 0.
    #[test]
    fn stand_walk_after_a_push_matches_a_fresh_scan() {
        let (mut pushes, mut onward) = (0, 0);
        for (id, board, states, edges) in explored_catalog() {
            let heuristic = Heuristic::new(board);
            let mut reach = Reach::new(board.tiles().len());
            let n = board.labels().len();
            for (parent, state) in states.iter().enumerate() {
                reach.fill(board, state);
                for &(child, push) in &edges[parent] {
                    let Some((i, d)) = push else {
                        continue;
                    };
                    let (from, to) = (state.boxes[i], states[child].boxes[i]);
                    let boxes = &states[child].boxes[..n];
                    let occupied = |cell: Cell| cell == to || (cell != from && reach.blocked(cell));
                    let exact = walk(board, &heuristic, from, boxes);
                    for enough in 0..=exact + 1 {
                        let got = stand_walk(board, &heuristic, from, boxes, occupied, enough);
                        assert_eq!(got < enough, exact < enough, "{id}: {got} {exact} {enough}");
                        assert!(got < enough || got == exact, "{id}: {got} {exact} {enough}");
                    }
                    let ahead = board.neighbors()[to as usize][d];
                    if ahead != NONE && !occupied(ahead) && !heuristic.dead(i, ahead) {
                        assert_eq!(exact, 0, "{id} at {:?}", states[child]);
                        onward += 1;
                    }
                    pushes += 1;
                }
            }
        }
        assert!(onward > 0 && onward < pushes, "{onward} {pushes}");
    }

    /// The root estimate is pushes + stand walk. It rises over pushes + box
    /// walk (the step to the nearest box's side, recomputed below) by
    /// exactly these gains, on exactly these catalog boards, so a change to
    /// either walk, to the dead masks or to the catalog shows here. The gain
    /// reads only cells and dead masks, not the push count.
    ///
    /// The seven gains come from a Python replica of the engine's start
    /// estimate, written from the stand-walk rule before this code and not
    /// kept in the repo, which raised these 7 of the 57 boards by 2 each.
    /// That is a second implementation of the same rule, not an
    /// independent proof. tutorial-push checks by hand: its only statically
    /// legal first push, right, needs a stand 3 steps from the keeper, 2
    /// more than the step to the box's side, so its root is 1 + 3 = 4, the
    /// optimum the crate example proves.
    #[test]
    fn stand_walk_raises_exactly_these_root_estimates() {
        const RAISED: [(&str, u32); 7] = [
            ("tutorial-push", 2),
            ("beginner-detour", 2),
            ("garden-2", 2),
            ("classic-1", 2),
            ("adv-gallery", 2),
            ("theme-parking", 2),
            ("expert-maze", 2),
        ];
        let mut raised = 0;
        for (id, board) in catalog() {
            let heuristic = Heuristic::new(&board);
            // The start as the engine seeds it, canonical.
            let mut start = board.initial();
            canonicalize(&board, &mut start);
            let pushes = heuristic.estimate(&start).unwrap();
            let width = board.width();
            let (x, y) = (start.player as usize % width, start.player as usize / width);
            let box_walk = start.boxes[..board.labels().len()]
                .iter()
                .map(|&cell| {
                    x.abs_diff(cell as usize % width) + y.abs_diff(cell as usize / width) - 1
                })
                .min()
                .unwrap() as u32;
            let gain = RAISED
                .iter()
                .find(|&&(raised_id, _)| raised_id == id)
                .map_or(0, |&(_, gain)| gain);
            let root = root_estimate(&board, &heuristic, &start, pushes);
            assert_eq!(root, pushes + box_walk + gain, "{id}");
            raised += usize::from(root > pushes + box_walk);
        }
        assert_eq!(raised, RAISED.len());
    }
}
