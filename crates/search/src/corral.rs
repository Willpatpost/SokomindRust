//! Sealed-corral dead states, checked once per expansion right after the
//! keeper flood. A corral is a 4-connected component of the cells the
//! keeper cannot reach, box cells included, flooded from one of its boxes.
//! Its empty cells border only walls and its own cells: an empty cell next
//! to a reached one would be reached, and any other neighbor would join the
//! component. So until one of its boxes moves, neither the keeper nor a box
//! from outside enters it, and the first push of one of its boxes in any
//! solution starts from a stand the keeper reaches now and lands on a cell
//! with no box now. Those potential pushes are judged as if every box
//! outside the corral were absent: a push is fatal when it lands on a dead
//! cell for its label, or when the freeze fixpoint over the corral's boxes
//! alone, after the push, holds one off its goal. Boxes outside can only
//! hold more axes, so the push is fatal wherever they go by then. A corral
//! whose potential pushes are all fatal never moves, and the state has no
//! solution when it holds a box off its goal or an empty goal cell, which
//! needs a box from outside that can never get in.
//!
//! Boxes outside the corral never block anything here: treating a box that
//! may still move as a permanent blocker is the false proof Sokomind2's
//! corral check made (audit F-001). Only the dead-state check is ported;
//! restricting a state's successors to corral pushes and ordering corrals
//! are push-objective techniques that lose routes or proofs under the move
//! objective, so neither is.
//!
//! A corral with no empty cell is skipped, a choice of scope and cost: its
//! boxes all border the keeper's region, and judging such corrals too flags
//! far more states but no more that the admit chain keeps (the census in
//! pruning/t2-census.json, not tracked, finds the same 1,276 and 620 kept
//! flags on the rooms and the catalog tested below either way). When the
//! reached cells and the boxes cover every floor cell, every corral is
//! such a one, so the scan is skipped outright. A corral with more than
//! [`CAP`] potential pushes is skipped too, which bounds the freeze work
//! per expansion; one with none is always judged.
//!
//! The pocket check, `pockets_begin` and then one `pockets_step` unit at a
//! time, asks a deeper question for the stage ladder. A pocket is a
//! 4-connected component of the empty cells the keeper cannot reach with
//! every box blocked; the boxes next to it are its members, and it borders
//! only them and walls. Its search explores the world of the members
//! alone, every other box taken off the board: pushes whose stand the
//! keeper reaches, whose destination is free and live for the member's
//! label, and that leave no freeze over the members holding one off its
//! goal. The pocket is dead, and the state with it, when no state of that
//! world lets the keeper into the pocket or has every member on a goal of
//! its label. That is sound by projection: until the keeper gets in, only
//! members' pushes change the pocket, and in a solution they form such a
//! sequence that ends with every member on a goal, since the other boxes
//! only obstruct, and a dead cell or a freeze over fewer boxes is dead with
//! more. The check ports the stage probe's `pockets_ok` and `pocket_dead`
//! (slurm/probes/p4b.rs 358-466, not tracked) with the same verdicts: a
//! pocket with more than [`POCKET_BOXES`] members, or whose search would
//! queue more than [`POCKET_NODES`] states, counts as alive.
use crate::{
    deadlock::{Deadlock, frozen_off_goal},
    heuristic::Heuristic,
    reach::Reach,
};
use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, State, WALL};
use std::{collections::TryReserveError, mem::size_of};

/// Potential pushes judged per corral at most; a corral with more is
/// skipped. The reference's PI-corral check caps a corral's boundary boxes
/// at six instead (`MAX_BOUNDARY_BOXES` in pi-corral.ts); capping the
/// pushes is equally sound and bounds the work directly.
const CAP: usize = 6;

/// A potential push of box `i` from `from` to `to`.
#[derive(Clone, Copy)]
struct Push {
    i: usize,
    from: Cell,
    to: Cell,
}

/// Sealed-corral detection with epoch stamps: each corral bumps the epoch
/// instead of clearing the stamps, so a check costs only the cells its
/// corrals cover. It only ever answers "dead" for states from which no
/// solution exists, so every mode may prune with it freely.
pub(crate) struct Corral {
    /// Cell -> epoch of the last corral that covered it. A stamp at or
    /// above the first epoch of the current check marks a cell already
    /// covered by it.
    stamps: Vec<u32>,
    epoch: u32,
    /// The current corral's cells, in flood order.
    queue: Vec<Cell>,
    /// Non-wall cells on the board.
    floor: usize,
}

/// The set bits of `mask`, lowest first: the box slots a members mask holds.
fn bits(mut mask: u32) -> impl Iterator<Item = usize> {
    std::iter::from_fn(move || {
        (mask != 0).then(|| {
            let j = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            j
        })
    })
}

/// The potential pushes of the boxes in `members`, box-major and in
/// direction order, as the expansion's static check would find them: a
/// stand the last fill reached and a destination with no box. `None` when
/// there are more than [`CAP`].
fn pushes(
    board: &Board,
    boxes: &[Cell],
    reach: &Reach,
    members: u32,
) -> Option<([Push; CAP], usize)> {
    let mut pushes = [Push {
        i: 0,
        from: NONE,
        to: NONE,
    }; CAP];
    let mut count = 0;
    for i in bits(members) {
        let from = boxes[i];
        let neighbors = board.neighbors()[from as usize];
        for (d, &opposite) in OPPOSITE.iter().enumerate() {
            let (to, stand) = (neighbors[d], neighbors[opposite]);
            if to == NONE || stand == NONE || reach.blocked(to) || reach.distance(stand) == NONE {
                continue;
            }
            if count == CAP {
                return None;
            }
            pushes[count] = Push { i, from, to };
            count += 1;
        }
    }
    Some((pushes, count))
}

/// Members a pocket may have and still be searched, the probe's
/// `POCKET_BOXES`; a pocket with more counts as alive.
const POCKET_BOXES: usize = 8;
/// States a pocket search may queue, its start included: the probe's
/// `POCKET_NODES`, past which its pop count calls the pocket alive.
const POCKET_NODES: usize = 512;
/// Log2 of the seen table's slots.
const TABLE_BITS: u32 = 10;
/// The seen table's slots: at least twice the states it can hold, so its
/// load stays at or below one half and every probe ends at an empty slot.
const TABLE: usize = 1 << TABLE_BITS;
/// An empty table slot, or the slot of an entry not in the table. No FIFO
/// index reaches it.
const NIL: u16 = u16::MAX;
const _: () = assert!(TABLE >= 2 * POCKET_NODES && POCKET_NODES < NIL as usize);

/// A queued state of a pocket search.
#[derive(Clone, Copy)]
struct Entry {
    /// The members' cells in canonical order: the pocket's slot order, with
    /// each label's run sorted by cell, so states that differ only by
    /// swapping interchangeable boxes are equal arrays. `NONE` past the
    /// pocket's members.
    cells: [Cell; POCKET_BOXES],
    /// The keeper's cell when queued, which its pop replaces with the
    /// region, the least cell the keeper reaches: with `cells`, the key.
    at: Cell,
    /// The table slot this entry holds, `NIL` until its pop inserts it, and
    /// for good when its key was popped before.
    slot: u16,
}
// Ten u16 fields and no padding: the 20 bytes per entry `Pockets::BYTES`
// counts.
const _: () = assert!(size_of::<Entry>() == 20);

impl Entry {
    /// The key's home slot: the top bits of a multiplicative hash of the
    /// region and the cells.
    fn home(&self) -> usize {
        let mut hash = u64::from(self.at);
        for &cell in &self.cells {
            hash = (hash ^ u64::from(cell)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        (hash >> (u64::BITS - TABLE_BITS)) as usize
    }
}

/// What one `Corral::pockets_step` unit found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PocketStep {
    /// A pocket is dead, so the state has no solution. The check is over.
    Dead,
    /// No pocket is dead. The check is over.
    Alive,
    /// The check goes on with another unit.
    More,
}

/// The next unit of a pockets check.
enum Unit {
    /// Scan on from the cursor; the last fill is the scan flood.
    Scan,
    /// Refill the scan flood, which the pops overwrote, then scan on.
    Rescan,
    /// Pop the current pocket's next entry.
    Pop,
}

/// The pocket search's own fixed state: the FIFO of queued states, the
/// table of popped keys, the scan cursor and the current pocket's members.
/// The scan flood is `Reach`'s and the seen marks and pocket cells are
/// `Corral`'s stamps, so nothing here grows with the board.
///
/// Layout: the FIFO is [`POCKET_NODES`] 20-byte entries, reserved once,
/// and the table [`TABLE`] u16 FIFO indices, `NIL` when empty, probed
/// linearly from a key's home slot. Only popped entries enter the table, at
/// most one per FIFO entry, so its load stays at or below one half. A new
/// pocket resets it by writing `NIL` back to the slots its FIFO entries
/// recorded, never filling all of it. Nothing is allocated after `new`.
pub(crate) struct Pockets {
    /// Queued states, the next to pop at `head`; never past its capacity,
    /// [`POCKET_NODES`].
    fifo: Vec<Entry>,
    /// Slot -> FIFO index of the entry that first popped with its key.
    table: Vec<u16>,
    head: usize,
    /// The next cell the scan tests.
    cursor: usize,
    /// The check's first Corral epoch: a stamp below it marks a cell no
    /// pocket of this check covers.
    first: u32,
    /// Each member's box slot, ascending. A slot stands only for its
    /// label's group, dead cells and goals, so it stays valid for whichever
    /// interchangeable box canonical order puts in its place.
    slots: [u8; POCKET_BOXES],
    /// The current pocket's member count.
    count: usize,
    unit: Unit,
}

impl Pockets {
    /// Heap bytes `new` reserves: the FIFO and the table, 12,288 bytes.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) const BYTES: usize = POCKET_NODES * size_of::<Entry>() + TABLE * size_of::<u16>();
    /// The FIFO and the table, reserved once at their final sizes, whatever
    /// the board.
    pub(crate) fn new() -> Result<Self, TryReserveError> {
        let mut fifo = Vec::new();
        fifo.try_reserve_exact(POCKET_NODES)?;
        let mut table = Vec::new();
        table.try_reserve_exact(TABLE)?;
        table.resize(TABLE, NIL);
        Ok(Self {
            fifo,
            table,
            head: 0,
            cursor: 0,
            first: 0,
            slots: [0; POCKET_BOXES],
            count: 0,
            unit: Unit::Scan,
        })
    }
    /// Starts the search of a pocket with the boxes in `members`, one to
    /// [`POCKET_BOXES`] of them: clears the table and the FIFO, then queues
    /// the members' cells with the state's keeper.
    fn start(&mut self, heuristic: &Heuristic, state: &State, members: u32) {
        debug_assert!(members != 0 && members.count_ones() as usize <= POCKET_BOXES);
        for entry in &self.fifo {
            if entry.slot != NIL {
                self.table[usize::from(entry.slot)] = NIL;
            }
        }
        self.fifo.clear();
        self.head = 0;
        self.count = 0;
        let mut cells = [NONE; POCKET_BOXES];
        for j in bits(members) {
            self.slots[self.count] = j as u8;
            cells[self.count] = state.boxes[j];
            settle(
                heuristic,
                &self.slots,
                &mut cells[..=self.count],
                self.count,
            );
            self.count += 1;
        }
        debug_assert!(self.fifo.len() < self.fifo.capacity());
        self.fifo.push(Entry {
            cells,
            at: state.player,
            slot: NIL,
        });
        self.unit = Unit::Pop;
    }
    /// Inserts the key of FIFO entry `index`, whose `at` already holds its
    /// region, and records its slot; false when an earlier pop holds the
    /// same key.
    fn insert(&mut self, index: usize) -> bool {
        let entry = self.fifo[index];
        let mut slot = entry.home();
        loop {
            let held = self.table[slot];
            if held == NIL {
                self.table[slot] = index as u16;
                self.fifo[index].slot = slot as u16;
                return true;
            }
            let other = &self.fifo[usize::from(held)];
            if other.cells == entry.cells && other.at == entry.at {
                return false;
            }
            slot = (slot + 1) % TABLE;
        }
    }
}

/// Moves the cell at `k`, the only one out of place, to its canonical
/// position within its label's run of `cells`, whose box slots are
/// `slots`: left past larger cells or right past smaller ones.
fn settle(heuristic: &Heuristic, slots: &[u8], cells: &mut [Cell], mut k: usize) {
    let same = |a: usize, b: usize| {
        heuristic
            .group(usize::from(slots[a]))
            .contains(&usize::from(slots[b]))
    };
    while k > 0 && same(k - 1, k) && cells[k - 1] > cells[k] {
        cells.swap(k - 1, k);
        k -= 1;
    }
    while k + 1 < cells.len() && same(k, k + 1) && cells[k + 1] < cells[k] {
        cells.swap(k, k + 1);
        k += 1;
    }
}

impl Corral {
    /// The stamps and the queue.
    pub(crate) const BYTES_PER_CELL: usize = size_of::<u32>() + size_of::<Cell>();
    /// Empty buffers for `board`, allocated once.
    pub(crate) fn new(board: &Board) -> Self {
        let cells = board.tiles().len();
        Self {
            stamps: vec![0; cells],
            epoch: 0,
            queue: Vec::with_capacity(cells),
            floor: board.tiles().iter().filter(|&&tile| tile != WALL).count(),
        }
    }
    /// Whether some corral of the state with `boxes` leaves no solution.
    /// `reach` must hold the state's fill and `deadlock` its refresh, as
    /// they do in the engine's expansion before the first push. `dead_pair`
    /// is the freeze rule's dead-pair case, as admit passes it; the
    /// dead-cell test always uses `heuristic`.
    pub(crate) fn is_dead(
        &mut self,
        board: &Board,
        boxes: &[Cell],
        reach: &Reach,
        deadlock: &Deadlock,
        heuristic: &Heuristic,
        dead_pair: Option<&Heuristic>,
    ) -> bool {
        if reach.reached() + boxes.len() == self.floor {
            return false;
        }
        // A check bumps the epoch once per corral, at most once per box, so
        // zeroing the stamps whenever fewer than MAX_BOXES epochs are left
        // keeps it from wrapping inside a check.
        if self.epoch > u32::MAX - MAX_BOXES as u32 {
            self.stamps.fill(0);
            self.epoch = 0;
        }
        let first = self.epoch + 1;
        for (i, &seed) in boxes.iter().enumerate() {
            if self.stamps[seed as usize] >= first {
                // An earlier box's corral already covered this one.
                continue;
            }
            let members = self.flood(board, boxes, reach, i);
            if self.queue.len() == members.count_ones() as usize
                || !self.goal_open(board, boxes, reach, members)
            {
                continue;
            }
            let Some((pushes, count)) = pushes(board, boxes, reach, members) else {
                continue;
            };
            if pushes[..count].iter().all(|push| {
                heuristic.dead(push.i, push.to)
                    || deadlock.is_dead_after_push(board, dead_pair, members, push.from, push.to)
            }) {
                return true;
            }
        }
        false
    }
    /// Floods the corral of box `i` into the queue under a fresh epoch and
    /// returns its boxes as a members mask. A box before `i` in it would
    /// have covered `i` already, so only `i` and later boxes are tested.
    fn flood(&mut self, board: &Board, boxes: &[Cell], reach: &Reach, i: usize) -> u32 {
        self.epoch += 1;
        let epoch = self.epoch;
        self.queue.clear();
        self.queue.push(boxes[i]);
        self.stamps[boxes[i] as usize] = epoch;
        let mut head = 0;
        while head < self.queue.len() {
            let cell = self.queue[head];
            head += 1;
            for &next in &board.neighbors()[cell as usize] {
                if next != NONE
                    && reach.distance(next) == NONE
                    && self.stamps[next as usize] != epoch
                {
                    self.stamps[next as usize] = epoch;
                    self.queue.push(next);
                }
            }
        }
        if self.queue.len() == 1 {
            // A box with only reached cells and walls around it, a common
            // corral, needs no scan.
            return 1 << i;
        }
        boxes
            .iter()
            .enumerate()
            .skip(i)
            .filter(|&(_, &cell)| self.stamps[cell as usize] == epoch)
            .fold(0, |members, (j, _)| members | (1 << j))
    }
    /// Whether the corral in the queue still needs a box to move: one of
    /// `members` is off its goal, or a goal cell in it has no box. Without
    /// either it is settled, and leaving it alone is no deadlock.
    fn goal_open(&self, board: &Board, boxes: &[Cell], reach: &Reach, members: u32) -> bool {
        bits(members).any(|j| !board.on_goal(j, boxes[j]))
            // The flood queues no wall, since `Board::neighbors` lists none,
            // so a tile other than plain floor, 0, is a goal.
            || self
                .queue
                .iter()
                .any(|&cell| board.tiles()[cell as usize] != 0 && !reach.blocked(cell))
    }
    /// Opens a pockets check of `state`: fills `reach` with every box
    /// blocked, the probe's scan flood, and puts the scan at cell 0 under a
    /// fresh range of epochs. Until the check ends, `deadlock` must hold the
    /// state's refresh and nothing else may use this `Corral` or `reach`:
    /// the scan reads the fill between units, and stamps at or above the
    /// check's first epoch mark the cells its pockets cover.
    pub(crate) fn pockets_begin(
        &mut self,
        pockets: &mut Pockets,
        board: &Board,
        reach: &mut Reach,
        state: &State,
    ) {
        reach.fill(board, state);
        // A check bumps the epoch once per pocket, fewer times than there
        // are cells, so zeroing the stamps whenever fewer epochs than cells
        // are left keeps it from wrapping inside a check.
        if self.epoch > u32::MAX - self.stamps.len() as u32 {
            self.stamps.fill(0);
            self.epoch = 0;
        }
        pockets.first = self.epoch + 1;
        pockets.cursor = 0;
        pockets.unit = Unit::Scan;
    }
    /// One unit of the check `pockets_begin` opened: a pop of the current
    /// pocket's search, or a scan on to the next pocket, which it floods
    /// and, with one to [`POCKET_BOXES`] members, starts searching. A
    /// pocket with no member borders only walls; every member on a goal
    /// holds vacuously there, so it is alive and never searched.
    pub(crate) fn pockets_step(
        &mut self,
        pockets: &mut Pockets,
        board: &Board,
        heuristic: &Heuristic,
        reach: &mut Reach,
        deadlock: &Deadlock,
        state: &State,
    ) -> PocketStep {
        match pockets.unit {
            Unit::Pop => return self.pocket_pop(pockets, board, heuristic, reach),
            Unit::Rescan => reach.fill(board, state),
            Unit::Scan => {}
        }
        pockets.unit = Unit::Scan;
        let (tiles, first) = (board.tiles(), pockets.first);
        // An empty cell the scan flood missed, in no pocket of this check.
        let Some(start) = (pockets.cursor..tiles.len()).find(|&cell| {
            tiles[cell] != WALL
                && self.stamps[cell] < first
                && reach.distance(cell as Cell) == NONE
                && deadlock.at(cell as Cell).is_none()
        }) else {
            pockets.cursor = tiles.len();
            return PocketStep::Alive;
        };
        pockets.cursor = start + 1;
        let members = self.pocket_flood(board, deadlock, start as Cell);
        if members != 0 && members.count_ones() as usize <= POCKET_BOXES {
            pockets.start(heuristic, state, members);
        }
        PocketStep::More
    }
    /// Floods the pocket of the empty cell `start` into the queue under a
    /// fresh epoch, stamping each of its cells, and returns the boxes next
    /// to it as a members mask. An empty neighbor of an unreached empty
    /// cell is unreached too, so the flood needs no reach test, and it
    /// always completes, so a later scan skips every cell it covered.
    fn pocket_flood(&mut self, board: &Board, deadlock: &Deadlock, start: Cell) -> u32 {
        self.epoch += 1;
        let epoch = self.epoch;
        self.queue.clear();
        debug_assert!(self.queue.len() < self.queue.capacity());
        self.queue.push(start);
        self.stamps[start as usize] = epoch;
        let mut members = 0;
        let mut head = 0;
        while head < self.queue.len() {
            let cell = self.queue[head];
            head += 1;
            for &next in &board.neighbors()[cell as usize] {
                if next == NONE {
                    continue;
                }
                if let Some(j) = deadlock.at(next) {
                    members |= 1 << j;
                } else if self.stamps[next as usize] != epoch {
                    self.stamps[next as usize] = epoch;
                    debug_assert!(self.queue.len() < self.queue.capacity());
                    self.queue.push(next);
                }
            }
        }
        members
    }
    /// Pops the current pocket's next state. It floods the keeper's region
    /// with only the members blocked and replaces the entry's keeper with
    /// the region's least cell, completing its key. A key popped before is
    /// skipped, as the probe's seen set does. Otherwise the pocket ends
    /// alive when the keeper gets into it, a cell with its flood's stamp,
    /// or every member is on a goal of its label; else the state's pushes
    /// are queued. The pocket ends dead when the FIFO runs out.
    ///
    /// The probe queues every push, repeats included, and calls the pocket
    /// alive on its pop past [`POCKET_NODES`]; it orders members and pushes
    /// differently from this search's slots and settled cells, but neither
    /// verdict depends on order. A key fixes its region's flood and so its
    /// pushes, so in any order a search reaches the same keys, and once it
    /// has popped them all it has queued the same total: the start plus
    /// each key's pushes on its first pop. Both call the pocket dead
    /// exactly when no reachable key lets the keeper in or settles every
    /// member and that total is at most [`POCKET_NODES`]; every other end
    /// is alive in both. So ending alive on the first push the full FIFO
    /// cannot take gives the probe's verdict, and a dead pocket has popped
    /// exactly as many entries as the FIFO holds.
    fn pocket_pop(
        &self,
        pockets: &mut Pockets,
        board: &Board,
        heuristic: &Heuristic,
        reach: &mut Reach,
    ) -> PocketStep {
        let index = pockets.head;
        pockets.head += 1;
        let (entry, count, slots) = (pockets.fifo[index], pockets.count, pockets.slots);
        let cells = &entry.cells[..count];
        reach.fill_from(board, entry.at, cells);
        // The keeper is never on a member, so the fill reaches at least its
        // cell.
        let region = *reach.reached_cells().iter().min().unwrap_or(&entry.at);
        pockets.fifo[index].at = region;
        if pockets.insert(index) {
            let opened = reach
                .reached_cells()
                .iter()
                .any(|&cell| self.stamps[cell as usize] == self.epoch);
            let settled = cells
                .iter()
                .zip(&slots)
                .all(|(&cell, &slot)| board.on_goal(usize::from(slot), cell));
            if opened || settled {
                pockets.unit = Unit::Rescan;
                return PocketStep::More;
            }
            for (j, (&from, &slot)) in cells.iter().zip(&slots).enumerate() {
                let around = board.neighbors()[from as usize];
                for (d, &opposite) in OPPOSITE.iter().enumerate() {
                    let (to, stand) = (around[d], around[opposite]);
                    if to == NONE
                        || stand == NONE
                        || reach.distance(stand) == NONE
                        || reach.blocked(to)
                        || heuristic.dead(usize::from(slot), to)
                    {
                        continue;
                    }
                    let mut next = entry.cells;
                    next[j] = to;
                    settle(heuristic, &slots[..count], &mut next[..count], j);
                    let pairs: [(usize, Cell); POCKET_BOXES] =
                        std::array::from_fn(|k| (usize::from(slots[k]), next[k]));
                    if frozen_off_goal(board, heuristic, &pairs[..count]) {
                        continue;
                    }
                    if pockets.fifo.len() == POCKET_NODES {
                        pockets.unit = Unit::Rescan;
                        return PocketStep::More;
                    }
                    debug_assert!(pockets.fifo.len() < pockets.fifo.capacity());
                    pockets.fifo.push(Entry {
                        cells: next,
                        at: from,
                        slot: NIL,
                    });
                }
            }
        }
        if pockets.head == pockets.fifo.len() {
            PocketStep::Dead
        } else {
            PocketStep::More
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Corral, Entry, POCKET_BOXES, POCKET_NODES, PocketStep, Pockets, bits, pushes};
    use crate::{
        Status,
        deadlock::{ALL_BOXES, Deadlock, frozen_off_goal},
        engine::{Engine, Policy},
        heuristic::Heuristic,
        reach::Reach,
        testkit::{Lcg, catalog, explored_catalog, random_room, remaining, solvable_states},
    };
    use sokomind_core::{Board, Cell, MAX_BOXES, NONE, OPPOSITE, State, WALL};
    use std::{
        collections::{HashSet, VecDeque},
        mem::size_of,
    };

    /// Two X boxes stacked in a one-cell door and the room below it, both
    /// goals above: the keeper reaches only the goal row, no push is legal,
    /// and the door and the room are one corral with empty cells and no
    /// potential push.
    const SEALED: &str = "OOOOOOO\nOSS R O\nOOOXOOO\nO  X  O\nO     O\nO     O\nOOOOOOO";
    /// Audit F-001's board, which Sokomind2's corral check reported
    /// unsolvable; pushing the upper X down twice and the other one left
    /// solves it in 3.
    const CROSSING: &str = "OOOOOO\nOOOROO\nOOOXOO\nOSX OO\nOOOSOO\nOOOOOO";
    /// A closes the pocket above it. Its only potential pushes go left onto
    /// a dead corner and right against B, which can still slide right to
    /// its goal first: the start is solvable, and only a check that counts
    /// B as a blocker freezes A and B against the wall above them.
    const OUTSIDE_BOX: &str = "OOOOOOOO\nOO OOOOO\nO A BabO\nORO    O\nO      O\nOOOOOOOO";
    /// Three boxes, each with floor on every side.
    const OPEN: &str = "OOOOOOO\nO     O\nO X X O\nO  X  O\nOSSS RO\nOOOOOOO";
    /// A must settle on its goal in the corridor only after B has passed
    /// it to the goal below; pushed in first, A seals B's goal off.
    const SETTLED: &str = "OOOOOOO\nO     O\nO BRA O\nOOOaOOO\nO  b  O\nOOOOOOO";
    /// Three pockets in scan order below the keeper's corridor: B's goal
    /// cell under B, which B fills on the second pop, so the first pocket
    /// is alive; a cell with walls all round, a pocket with no member; and
    /// the cell under D, which D can only be pushed into, a dead cell, so
    /// the third pocket is dead. The first pocket's pops flood with B alone
    /// blocked and reach the third, so the scan finds it only from a fresh
    /// scan flood.
    const RESUME: &str = "OOOOOOOOO\nOR  d   O\nOOBOOODOO\nOObO O OO\nOOOOOOOOO";
    /// Eight A boxes in the gaps of a wall above a corridor holding the A
    /// goals in the columns between them, and B at the corridor's end below
    /// a wall, its goal at the end of the keeper's corridor. The lower
    /// corridor is one pocket with all nine boxes as members. Each A can
    /// only be pushed down into it, onto a cell no other A can reach, and
    /// the keeper never gets in.
    const GAPS: &str = concat!(
        "OOOOOOOOOOOOOOOOOOO\n",
        "OR               bO\n",
        "OOAOAOAOAOAOAOAOAOO\n",
        "Oa a a a a a a a BO\n",
        "OOOOOOOOOOOOOOOOOOO",
    );

    fn at(board: &Board, row: usize, column: usize) -> Cell {
        (row * board.width() + column) as Cell
    }

    /// The buffers the engine hands the detector, filled for one state at a
    /// time as an expansion fills them.
    struct Checker {
        board: Board,
        heuristic: Heuristic,
        reach: Reach,
        deadlock: Deadlock,
        corral: Corral,
    }
    impl Checker {
        fn new(board: &Board) -> Self {
            Self {
                board: board.clone(),
                heuristic: Heuristic::new(board),
                reach: Reach::new(board.tiles().len()),
                deadlock: Deadlock::new(board),
                corral: Corral::new(board),
            }
        }
        /// Fills and refreshes for the state, then runs the detector with
        /// the dead-pair case, as the engine does.
        fn dead(&mut self, player: Cell, boxes: &[Cell]) -> bool {
            self.reach.fill_from(&self.board, player, boxes);
            self.deadlock.refresh(boxes);
            self.corral.is_dead(
                &self.board,
                boxes,
                &self.reach,
                &self.deadlock,
                &self.heuristic,
                Some(&self.heuristic),
            )
        }
        /// Whether the admit chain keeps the state `dead` last saw: no box
        /// on a dead cell and none frozen off its goal over every box.
        fn admitted(&self, boxes: &[Cell]) -> bool {
            let h = &self.heuristic;
            boxes.iter().enumerate().all(|(i, &cell)| !h.dead(i, cell))
                && !boxes.iter().any(|&cell| {
                    self.deadlock
                        .is_dead_after_push(&self.board, Some(h), ALL_BOXES, cell, cell)
                })
        }
        /// Refreshes for the state and runs a pockets check to its end, unit
        /// by unit as the ladder will: whether a pocket is dead, and how
        /// many units the check took.
        fn pockets(&mut self, pockets: &mut Pockets, state: &State) -> (bool, usize) {
            let count = self.board.labels().len();
            self.deadlock.refresh(&state.boxes[..count]);
            self.corral
                .pockets_begin(pockets, &self.board, &mut self.reach, state);
            let mut units = 0;
            loop {
                units += 1;
                match self.corral.pockets_step(
                    pockets,
                    &self.board,
                    &self.heuristic,
                    &mut self.reach,
                    &self.deadlock,
                    state,
                ) {
                    PocketStep::Dead => return (true, units),
                    PocketStep::Alive => return (false, units),
                    PocketStep::More => {}
                }
            }
        }
        /// The probe's `pockets_ok` (p4b.rs 358-394) over plain vectors and
        /// a breadth-first flood of its own, with at most `boxes` members
        /// per searched pocket and `nodes` pops per search: the pops of the
        /// first dead pocket's search, or `None` when none is dead.
        fn reference(&self, state: &State, boxes: usize, nodes: usize) -> Option<usize> {
            let (board, tiles) = (&self.board, self.board.tiles());
            let mut occupant: Vec<Option<usize>> = vec![None; tiles.len()];
            for (i, &cell) in state.boxes[..board.labels().len()].iter().enumerate() {
                occupant[cell as usize] = Some(i);
            }
            let blocked: Vec<bool> = occupant.iter().map(Option::is_some).collect();
            let reached = flood(board, state.player, &blocked);
            let mut seen = vec![false; tiles.len()];
            for start in 0..tiles.len() {
                if tiles[start] == WALL || blocked[start] || reached[start] || seen[start] {
                    continue;
                }
                seen[start] = true;
                let (mut pocket, mut members) = (vec![start as Cell], Vec::new());
                let mut k = 0;
                while k < pocket.len() {
                    let x = pocket[k];
                    k += 1;
                    for &y in &board.neighbors()[x as usize] {
                        if y == NONE {
                            continue;
                        }
                        if let Some(i) = occupant[y as usize] {
                            if !members.contains(&i) {
                                members.push(i);
                            }
                        } else if !seen[y as usize] {
                            seen[y as usize] = true;
                            pocket.push(y);
                        }
                    }
                }
                if members.len() > boxes {
                    continue;
                }
                if let Some(pops) = self.pocket_dead(state, &members, &pocket, nodes) {
                    return Some(pops);
                }
            }
            None
        }
        /// The probe's `pocket_dead` (p4b.rs 411-466) over plain vectors:
        /// the pops of the members' search when the pocket is dead, `None`
        /// when some state lets the keeper in or has every member on a goal
        /// of its label, or a pop passes `nodes`. Each member keeps its own
        /// slot, and a key sorts its (group, cell) pairs, as the probe's
        /// does.
        fn pocket_dead(
            &self,
            state: &State,
            members: &[usize],
            pocket: &[Cell],
            nodes: usize,
        ) -> Option<usize> {
            let (board, heuristic) = (&self.board, &self.heuristic);
            let start: Vec<Cell> = members.iter().map(|&i| state.boxes[i]).collect();
            let mut seen = HashSet::new();
            let mut queue = VecDeque::from([(start, state.player)]);
            let mut pops = 0;
            while let Some((cells, player)) = queue.pop_front() {
                pops += 1;
                if pops > nodes {
                    return None;
                }
                let mut blocked = vec![false; board.tiles().len()];
                for &cell in &cells {
                    blocked[cell as usize] = true;
                }
                let reached = flood(board, player, &blocked);
                let mut key: Vec<(usize, Cell)> = members
                    .iter()
                    .map(|&i| heuristic.group(i).start)
                    .zip(cells.iter().copied())
                    .collect();
                key.sort_unstable();
                let region = reached.iter().position(|&r| r).unwrap() as Cell;
                if !seen.insert((key, region)) {
                    continue;
                }
                let opened = pocket.iter().any(|&cell| reached[cell as usize]);
                let settled = members
                    .iter()
                    .zip(&cells)
                    .all(|(&i, &cell)| board.on_goal(i, cell));
                if opened || settled {
                    return None;
                }
                for (j, &x) in cells.iter().enumerate() {
                    let around = board.neighbors()[x as usize];
                    for (d, &opposite) in OPPOSITE.iter().enumerate() {
                        let (y, stand) = (around[d], around[opposite]);
                        if y == NONE
                            || stand == NONE
                            || !reached[stand as usize]
                            || blocked[y as usize]
                            || heuristic.dead(members[j], y)
                        {
                            continue;
                        }
                        let mut next = cells.clone();
                        next[j] = y;
                        let pairs: Vec<_> = members.iter().copied().zip(next.clone()).collect();
                        if !frozen_off_goal(board, heuristic, &pairs) {
                            queue.push_back((next, x));
                        }
                    }
                }
            }
            Some(pops)
        }
    }

    /// The cells `player` reaches by a plain breadth-first search, with the
    /// `blocked` cells as walls.
    fn flood(board: &Board, player: Cell, blocked: &[bool]) -> Vec<bool> {
        let mut reached = vec![false; blocked.len()];
        reached[player as usize] = true;
        let mut queue = VecDeque::from([player]);
        while let Some(cell) = queue.pop_front() {
            for &next in &board.neighbors()[cell as usize] {
                if next != NONE && !blocked[next as usize] && !reached[next as usize] {
                    reached[next as usize] = true;
                    queue.push_back(next);
                }
            }
        }
        reached
    }

    /// A state of `board` with its boxes and keeper on distinct floor cells
    /// drawn by `rng`.
    fn random_state(board: &Board, rng: &mut Lcg) -> State {
        let tiles = board.tiles();
        let mut free: Vec<Cell> = (0..tiles.len() as Cell)
            .filter(|&cell| tiles[cell as usize] != WALL)
            .collect();
        let mut state = board.initial();
        for cell in &mut state.boxes[..board.labels().len()] {
            *cell = free.swap_remove(rng.below(free.len()));
        }
        state.player = free.swap_remove(rng.below(free.len()));
        state
    }

    /// Runs the exact policy on `board` from its start to a terminal
    /// status, with every prune on.
    fn exact(board: &Board, max_states: usize) -> Engine {
        let mut engine = Engine::new(
            board.clone(),
            board.initial(),
            Policy::EXACT,
            max_states,
            64,
        )
        .unwrap();
        while engine.status() == Status::Running {
            engine.advance(1 << 10);
        }
        engine
    }

    #[test]
    fn bits_lists_each_slot_once() {
        assert_eq!(bits(0).count(), 0);
        assert_eq!(bits(0b1010_0001).collect::<Vec<_>>(), [0, 5, 7]);
        assert_eq!(bits(u32::MAX).count(), MAX_BOXES);
    }

    #[test]
    fn bytes_per_cell_matches_the_buffers() {
        let board = Board::parse(SETTLED).unwrap();
        let corral = Corral::new(&board);
        assert_eq!(
            corral.stamps.len() * size_of::<u32>() + corral.queue.capacity() * size_of::<Cell>(),
            board.tiles().len() * Corral::BYTES_PER_CELL
        );
    }

    /// The keeper reaches 5 of the 21 floor cells, so the scan runs. The
    /// door and the room below are one corral with both boxes, empty cells
    /// and no potential push, and its boxes are off their goals. Neither
    /// box is on a dead cell or frozen, so the admit chain alone keeps the
    /// state. The Python port of this module
    /// (`pruning/replicas/corral_port.py fixtures`, not tracked, line f1)
    /// finds 5 reachable states, one per keeper cell, all flagged and none
    /// solvable.
    #[test]
    fn sealed_corral_is_dead() {
        let board = Board::parse(SEALED).unwrap();
        let start = board.initial();
        let boxes = &start.boxes[..2];
        assert_eq!(boxes, [at(&board, 2, 3), at(&board, 3, 3)]);
        let mut checker = Checker::new(&board);
        assert!(checker.dead(start.player, boxes));
        assert_eq!((checker.reach.reached(), checker.corral.floor), (5, 21));
        assert!(checker.admitted(boxes));
        let solvable = solvable_states(&board);
        assert_eq!(solvable.len(), 5);
        for (&(player, cells), &has_solution) in &solvable {
            assert!(!has_solution);
            assert!(checker.dead(player, &cells[..2]), "{player}");
        }
    }

    /// A check that starts with exactly `MAX_BOXES` epochs left runs
    /// without zeroing the stamps and spends one; the next one, with fewer
    /// left, zeroes them and starts over at epoch 1. Both see the same
    /// corral (`corral_port.py wrap`).
    #[test]
    fn epochs_restart_before_they_wrap() {
        let board = Board::parse(SEALED).unwrap();
        let start = board.initial();
        let mut checker = Checker::new(&board);
        checker.corral.epoch = u32::MAX - MAX_BOXES as u32;
        assert!(checker.dead(start.player, &start.boxes[..2]));
        assert_eq!(checker.corral.epoch, u32::MAX - MAX_BOXES as u32 + 1);
        assert!(checker.dead(start.player, &start.boxes[..2]));
        assert_eq!(checker.corral.epoch, 1);
        assert!(checker.corral.stamps.iter().all(|&stamp| stamp <= 1));
    }

    /// Audit F-001: none of the board's 10 reachable states is flagged, so
    /// the exact policy still finds 3 (`corral_port.py fixtures`, line
    /// f2). The board guards the reported false proof but cannot catch its
    /// cause: freezing over every box flags none of its states either;
    /// `a_box_outside_the_corral_is_floor` does.
    #[test]
    fn a_corral_with_a_live_push_survives() {
        let board = Board::parse(CROSSING).unwrap();
        let mut checker = Checker::new(&board);
        let solvable = solvable_states(&board);
        assert_eq!(solvable.len(), 10);
        for &(player, cells) in solvable.keys() {
            assert!(!checker.dead(player, &cells[..2]), "{player}");
        }
        let engine = exact(&board, 1_000);
        assert_eq!(
            (engine.status(), engine.best_moves()),
            (Status::Solved, Some(3))
        );
        assert_eq!(engine.stats().pruned_corrals, 0);
    }

    /// A box outside the corral counts as floor. A's push right lands
    /// beside B, and the freeze over every box holds the pair against the
    /// wall above; over A alone it does not. B lies outside the corral and
    /// can be pushed right to its goal before A moves, so it must not count
    /// as a blocker. The start is solvable and not flagged, while a detector
    /// that froze over every box flags it. `corral_port.py fixtures`, line
    /// f4, finds 233 reachable states, 109 solvable, 44 flagged and none
    /// of those solvable; freezing over every box flags 59, 15 of them
    /// solvable, the start among them. The exact policy finds 18 with the
    /// corral on or off.
    #[test]
    fn a_box_outside_the_corral_is_floor() {
        let board = Board::parse(OUTSIDE_BOX).unwrap();
        let start = board.initial();
        let (a, b) = (at(&board, 2, 2), at(&board, 2, 4));
        assert_eq!(start.boxes[..2], [a, b]);
        let mut checker = Checker::new(&board);
        assert!(!checker.dead(start.player, &start.boxes[..2]));
        let (left, right) = (at(&board, 2, 1), at(&board, 2, 3));
        assert!(checker.heuristic.dead(0, left) && !checker.heuristic.dead(0, right));
        let pushed = |members| {
            checker
                .deadlock
                .is_dead_after_push(&board, Some(&checker.heuristic), members, a, right)
        };
        assert!(!pushed(1) && pushed(ALL_BOXES));
        let solvable = solvable_states(&board);
        let mut flagged = 0;
        for (&(player, cells), &has_solution) in &solvable {
            if checker.dead(player, &cells[..2]) {
                flagged += 1;
                assert!(!has_solution, "{player} {:?}", &cells[..2]);
            }
        }
        assert_eq!((solvable.len(), flagged), (233, 44));
        let engine = exact(&board, 1_000);
        assert_eq!(
            (engine.status(), engine.best_moves()),
            (Status::Solved, Some(18))
        );
    }

    /// The 45 reachable states with A on its goal in the corridor and B
    /// still in the upper room, nine keeper cells for each of B's five
    /// cells, have no solution. B's empty goal below A keeps the corral of
    /// A and the lower row open, and A's one potential push, down, lands on
    /// a dead cell. B on a corner, (2,1) or (2,5), is on a dead cell, so
    /// the admit chain keeps 27 of the states; the detector flags all but
    /// the 9 with B at (2,3), the 18 kept ones with B at (2,2) or (2,4)
    /// among them. B at (2,3) joins A's corral, its sideways pushes are
    /// live, and it blocks A's stand. The counts are `corral_port.py
    /// fixtures`, line f3.
    #[test]
    fn an_empty_goal_behind_a_settled_corral_is_dead() {
        let board = Board::parse(SETTLED).unwrap();
        assert_eq!(board.labels(), b"AB");
        let a = at(&board, 3, 3);
        let solvable = solvable_states(&board);
        let mut checker = Checker::new(&board);
        let mut counts = [(0, 0); 5];
        for (column, (flagged, admitted)) in (1..=5).zip(&mut counts) {
            let b = at(&board, 2, column);
            let mut cells = [NONE; MAX_BOXES];
            cells[..2].copy_from_slice(&[a, b]);
            for row in 1..=2 {
                for player in (1..=5).map(|c| at(&board, row, c)).filter(|&p| p != b) {
                    assert!(!solvable[&(player, cells)], "{player} {b}");
                    *flagged += usize::from(checker.dead(player, &cells[..2]));
                    *admitted += usize::from(checker.admitted(&cells[..2]));
                }
            }
        }
        assert_eq!(counts, [(9, 0), (9, 9), (0, 9), (9, 9), (9, 0)]);
    }

    /// Every state the detector flags on the 150 rooms that
    /// `flagged_pushes_leave_no_solution` checks, drawn the same way from
    /// `Lcg(0x5eed)`, has no solution. `corral_port.py rooms` flags 12_591
    /// of their 108_590 canonical states, 1_276 of them states the admit
    /// chain keeps. The bounds sit a little below, and
    /// `pruning/replicas/corral_variants.py` (not tracked) shows what fails
    /// them: skipping a corral whose boxes are all on goals despite an
    /// empty goal cell (12_573 flags), capping at three pushes (12_067) or
    /// dropping the dead-pair case (11_971). Lifting the cap, or freezing
    /// over every box, flags the same states here and on the catalog;
    /// `a_box_outside_the_corral_is_floor` catches the second.
    #[test]
    fn corral_dead_room_states_have_no_solution() {
        let mut rng = Lcg(0x5eed);
        let (mut flagged, mut admitted) = (0, 0);
        for _ in 0..150 {
            let rows = random_room(&mut rng);
            let board = Board::parse(&rows).unwrap();
            let boxes = board.labels().len();
            let mut checker = Checker::new(&board);
            for (&(player, cells), &has_solution) in &solvable_states(&board) {
                if !checker.dead(player, &cells[..boxes]) {
                    continue;
                }
                flagged += 1;
                admitted += usize::from(checker.admitted(&cells[..boxes]));
                assert!(!has_solution, "{rows:?}: {player} {:?}", &cells[..boxes]);
            }
        }
        assert!(
            flagged >= 12_580 && admitted >= 1_250,
            "{flagged} {admitted}"
        );
    }

    /// Every flagged state of an explored catalog board has no solution.
    /// `corral_port.py oracle` flags 8_675 of the 32_743 states, 620 of
    /// them states the admit chain keeps, all on box-5x5-a and the two
    /// gen-v2 boards. The variants above flag 8_664, 8_638 and 8_624, below
    /// the bound.
    #[test]
    fn corral_dead_states_have_no_solution() {
        let (mut flagged, mut admitted) = (0, 0);
        for (id, board, states, edges) in explored_catalog() {
            let left = remaining(board, states, edges);
            let boxes = board.labels().len();
            let mut checker = Checker::new(board);
            for (s, state) in states.iter().enumerate() {
                if !checker.dead(state.player, &state.boxes[..boxes]) {
                    continue;
                }
                flagged += 1;
                admitted += usize::from(checker.admitted(&state.boxes[..boxes]));
                assert_eq!(left[s], u32::MAX, "{id}: state {s}");
            }
        }
        assert!(flagged >= 8_670 && admitted >= 600, "{flagged} {admitted}");
    }

    /// No catalog start is flagged, huge's included; every one has a
    /// route.
    #[test]
    fn no_catalog_start_is_flagged() {
        for (id, board) in catalog() {
            let start = board.initial();
            let mut checker = Checker::new(&board);
            assert!(
                !checker.dead(start.player, &start.boxes[..board.labels().len()]),
                "{id}"
            );
        }
    }

    /// The large boards the oracles cannot explore: at 20,000 states the
    /// exact policy still finds medium's 34 and stops at the limit on large
    /// and huge, never ending Exhausted. `corral_port.py negative` prunes
    /// 680, 2_376 and 11 states there, and medium's search generates 16_110
    /// records, 18_504 without the check. The lower bound, the frontier
    /// capped by the route as `ExactSearch` reads it, rises from 73 to 84
    /// on large and stays at 215 on huge.
    #[test]
    fn large_boards_are_never_exhausted() {
        let boards = catalog();
        for (id, status, moves, lower, corrals, generated) in [
            ("medium", Status::Solved, Some(34), 34, 680, Some(16_110)),
            ("large", Status::StateLimit, None, 84, 2_376, None),
            ("huge", Status::StateLimit, None, 215, 11, None),
        ] {
            let (_, board) = boards.iter().find(|(name, _)| name == id).unwrap();
            let engine = exact(board, 20_000);
            assert_eq!(
                (engine.status(), engine.best_moves()),
                (status, moves),
                "{id}"
            );
            let route = engine.best_moves().map_or(u64::MAX, u64::from);
            assert_eq!(engine.frontier().min(route), lower, "{id}");
            assert_eq!(engine.stats().pruned_corrals, corrals, "{id}");
            if let Some(generated) = generated {
                assert_eq!(engine.generated(), generated, "{id}");
            }
        }
    }

    /// Two boxes in the open have eight potential pushes, past `CAP`, so
    /// their corral would be skipped; one alone has four.
    #[test]
    fn a_corral_past_the_cap_is_skipped() {
        let board = Board::parse(OPEN).unwrap();
        let start = board.initial();
        let boxes = &start.boxes[..3];
        let mut reach = Reach::new(board.tiles().len());
        reach.fill(&board, &start);
        assert!(pushes(&board, boxes, &reach, 0b001).is_some_and(|(_, count)| count == 4));
        assert!(pushes(&board, boxes, &reach, 0b011).is_none());
        assert!(pushes(&board, boxes, &reach, 0b111).is_none());
    }

    /// The pockets check against `Checker::reference`, the probe's
    /// `pockets_ok` with its pop-count exit, on the start and two random
    /// states of every catalog board and the start and three random states
    /// of 100 random rooms, all drawn from `Lcg(0x90c4)`: the same verdict
    /// and, on a dead one, a FIFO holding as many states as the reference
    /// popped. Both verdicts occur. The fixtures pin the edges. RESUME's
    /// first pocket ends alive after its pops overwrote the scan flood, and
    /// the scan finds the dead third pocket only from a fresh one. GAPS's
    /// pocket has nine members, past `POCKET_BOXES`, so it is skipped,
    /// though the reference without that cap finds it dead. With B moved
    /// onto its goal, the eight A members queue 1,024 pushes past the start,
    /// one for each A still up in each subset of them pushed down, so the
    /// FIFO fills and the pocket counts as alive, though the uncapped
    /// reference finds it dead after 1,025 pops. With one A already down as
    /// well, the other seven queue 448, and the pocket is dead after 449.
    #[test]
    fn pockets_match_reference() {
        let (boxes, nodes) = (POCKET_BOXES, POCKET_NODES);
        let mut pockets = Pockets::new().unwrap();
        let mut rng = Lcg(0x90c4);
        let mut boards: Vec<(String, Board, usize)> = catalog()
            .into_iter()
            .map(|(id, board)| (id, board, 2))
            .collect();
        for n in 0..100 {
            let board = Board::parse(&random_room(&mut rng)).unwrap();
            boards.push((format!("room {n}"), board, 3));
        }
        let (mut dead, mut alive) = (0, 0);
        for (id, board, draws) in &boards {
            let mut checker = Checker::new(board);
            for draw in 0..=*draws {
                let state = if draw == 0 {
                    board.initial()
                } else {
                    random_state(board, &mut rng)
                };
                let (found, _) = checker.pockets(&mut pockets, &state);
                let pops = checker.reference(&state, boxes, nodes);
                assert_eq!(found, pops.is_some(), "{id} draw {draw}");
                if let Some(pops) = pops {
                    assert_eq!(pockets.fifo.len(), pops, "{id} draw {draw}");
                }
                dead += usize::from(found);
                alive += usize::from(!found);
            }
        }
        assert!(dead > 0 && alive > 0, "{dead} {alive}");

        let board = Board::parse(RESUME).unwrap();
        let start = board.initial();
        let mut checker = Checker::new(&board);
        assert_eq!(checker.pockets(&mut pockets, &start), (true, 6));
        assert_eq!(pockets.fifo.len(), 1);
        assert_eq!(checker.reference(&start, boxes, nodes), Some(1));

        let board = Board::parse(GAPS).unwrap();
        let mut state = board.initial();
        assert_eq!(state.boxes[7..9], [at(&board, 2, 16), at(&board, 3, 17)]);
        let mut checker = Checker::new(&board);
        assert_eq!(checker.pockets(&mut pockets, &state), (false, 2));
        assert_eq!(checker.reference(&state, boxes, nodes), None);
        assert_eq!(checker.reference(&state, MAX_BOXES, nodes), Some(1));
        state.boxes[8] = at(&board, 1, 17);
        assert!(!checker.pockets(&mut pockets, &state).0);
        assert_eq!(pockets.fifo.len(), nodes);
        assert_eq!(checker.reference(&state, boxes, nodes), None);
        assert_eq!(checker.reference(&state, boxes, usize::MAX), Some(1_025));
        state.boxes[7] = at(&board, 3, 16);
        assert!(checker.pockets(&mut pockets, &state).0);
        assert_eq!(pockets.fifo.len(), 449);
        assert_eq!(checker.reference(&state, boxes, nodes), Some(449));
    }

    /// `Pockets::BYTES` is what `new` reserves: the FIFO's 512 entries of
    /// 20 bytes and the table's 1,024 u16 slots.
    #[test]
    fn pockets_bytes_match_capacity() {
        let pockets = Pockets::new().unwrap();
        let reserved = pockets.fifo.capacity() * size_of::<Entry>()
            + pockets.table.capacity() * size_of::<u16>();
        assert_eq!(reserved, Pockets::BYTES);
        assert_eq!(Pockets::BYTES, 12_288);
    }
}
