//! The room bound of the exact search (docs/room-bound.md).
//!
//! Each single-door room Q that holds a start box or a goal gets tables
//! R(Q, P, LAB) over disjoint parts P of Q (see `table`). The hall heuristic,
//! which prices the pushes that end outside every part, adds to their sum S,
//! and the exact search queues max(M, S).
//!
//! The tables are built before the search proper, by `RoomBuild`, in slices
//! the search pays for as pops. The build charges every buffer to the share
//! of the memory budget the arena leaves, keeps its tables within a quarter
//! of the budget, and stops at a work budget, keeping what it built by then.
//!
//! - `room`: a room's cells and the keeper's moves its tables walk.
//! - `table`: one table and its resumable build.
mod room;
mod table;

use crate::heuristic::{Heuristic, ParentGroup};
use room::{MAX_ROOM, Moves, Room};
use sokomind_core::{Board, Cell, MAX_CELLS, NONE, State};
use std::mem::{size_of, take};
use table::{MAX_VALUE, Step, Table, TableBuild};

/// The build work units each pop slot of `Search::advance` pays for while
/// the build runs, so a time check between calls bounds the build like
/// pops. A unit expands, pops or scans about one state of a table's search
/// (see `TableBuild::step`).
pub(crate) const BUILD_UNITS_PER_POP: usize = 64;
/// A table's forward search may reach this many states per node the arena
/// may hold, so the build scales with the search it serves, ...
const STATES_PER_NODE: u64 = 16;
/// ... clamped to `MIN_CAP..=MAX_CAP`.
const MIN_CAP: u64 = 1 << 20;
const MAX_CAP: u64 = 1 << 24;
/// The build's work budget, in units, is this many caps.
const WORK_PER_CAP: usize = 4;
/// A room is skipped when the tables of the rooms it encloses reached more
/// than the cap over this many states: its own search, which holds theirs,
/// would very likely pass the cap and waste its work.
const ENCLOSED_SHARE: usize = 16;
/// The kept tables may take the budget left after the fixed bytes over this.
const KEPT_SHARE: u64 = 4;

// Rooms are disjoint, so there are at most `MAX_CELLS`, each with at most
// two tables, and the tables' sum fits u32 with room for the hall part.
const _: () = assert!(2 * MAX_CELLS as u64 * MAX_VALUE as u64 <= u32::MAX as u64 / 2);
// `candidates` stamps a cell `door << 2 | side`, which must fit a Cell and
// stay apart from `NONE`.
const _: () = assert!(MAX_CELLS << 2 <= NONE as usize);

/// Whether bit `i` of `mask` is set.
fn bit(mask: u64, i: usize) -> bool {
    (mask >> i) & 1 != 0
}

/// A room or table that does not fit: the ledger or the allocator refused
/// a buffer, or the build passed a cap.
struct SizeFailure;

/// Grown buffers start at this many bytes.
const MIN_GROW_BYTES: usize = 4096;

/// The build's memory account. Every large buffer the build holds is
/// charged here before it is allocated, a room's few small lists right
/// after, and each is refunded when it is freed. So `live` stays within
/// `limit`, the share of the budget the arena leaves, but for those small
/// lists. `kept`, part of `live`, counts the bytes of the built tables, and
/// a table is built only if it stays within `kept_limit`, so the tables
/// leave most of the budget to the arena.
struct Ledger {
    live: usize,
    limit: usize,
    kept: usize,
    kept_limit: usize,
}

impl Ledger {
    /// Charges `bytes`, or refuses when they don't fit.
    fn reserve(&mut self, bytes: usize) -> Result<(), SizeFailure> {
        match self.live.checked_add(bytes) {
            Some(live) if live <= self.limit => {
                self.live = live;
                Ok(())
            }
            _ => Err(SizeFailure),
        }
    }

    fn free(&mut self, bytes: usize) {
        self.live -= bytes;
    }

    /// An empty vector of exactly `capacity` items, charged.
    fn vec<T>(&mut self, capacity: usize) -> Result<Vec<T>, SizeFailure> {
        let bytes = capacity.checked_mul(size_of::<T>()).ok_or(SizeFailure)?;
        self.reserve(bytes)?;
        let mut vec = Vec::new();
        if vec.try_reserve_exact(capacity).is_err() {
            self.free(bytes);
            return Err(SizeFailure);
        }
        // The allocator may round up: charge the capacity `release` refunds.
        self.live += (vec.capacity() - capacity) * size_of::<T>();
        Ok(vec)
    }

    /// A vector of `len` copies of `value`, charged.
    fn filled<T: Clone>(&mut self, len: usize, value: T) -> Result<Vec<T>, SizeFailure> {
        let mut vec = self.vec(len)?;
        vec.resize(len, value);
        Ok(vec)
    }

    /// Pushes `item`, doubling a full vector. The old and new buffers are
    /// both charged while the items move.
    fn push<T>(&mut self, vec: &mut Vec<T>, item: T) -> Result<(), SizeFailure> {
        if vec.len() == vec.capacity() {
            let mut grown = self.vec((2 * vec.capacity()).max(MIN_GROW_BYTES / size_of::<T>()))?;
            grown.append(vec);
            self.release(std::mem::replace(vec, grown));
        }
        vec.push(item);
        Ok(())
    }

    /// Frees `vec`, refunding its capacity.
    fn release<T>(&mut self, vec: Vec<T>) {
        self.free(vec.capacity() * size_of::<T>());
    }

    /// Whether `bytes` more kept bytes stay within the kept limit.
    fn fits_kept(&self, bytes: usize) -> bool {
        self.kept
            .checked_add(bytes)
            .is_some_and(|kept| kept <= self.kept_limit)
    }

    /// Counts a built table's bytes as kept.
    fn keep(&mut self, bytes: usize) {
        self.kept += bytes;
    }

    /// Stops counting `bytes` as kept, for the tables of rooms that a room
    /// being tried may replace.
    fn unkeep(&mut self, bytes: usize) {
        self.kept -= bytes;
    }
}

/// The candidate rooms: for each cell of `inside` taken as a door, each
/// side of it but the largest that holds a start box or a goal and has at
/// most `MAX_ROOM` cells. A side is a component of `inside` less the door.
/// Each comes with its cells sorted, and the list runs smallest first, by
/// size, door, then first cell.
///
/// The candidates are laminar: any two nest or are disjoint. Let T be
/// |inside|. A candidate Q is not its door's largest side, and the sides
/// partition `inside` less the door, so 2|Q| <= T - 1. Suppose Q1 and Q2,
/// with doors d1 and d2, meet but neither holds the other. Q2 is connected
/// and has cells in and out of Q1, and d1 is the only cell outside Q1 next
/// to it, so d1 is in Q2; likewise d2 is in Q1. `inside` less Q1 is d1 and
/// its other sides, which all touch d1, so it is connected. It misses d2, so
/// it lies in one side of d2, the one holding d1: Q2. Then |Q1| + |Q2| >= T,
/// against 2|Q1| <= T - 1 and 2|Q2| <= T - 1. Nor are two candidates equal:
/// a room's cells fix its door, the one cell outside them next to them. So,
/// smallest first, each room built before Q lies inside Q or apart from it,
/// which any one of its cells tells.
///
/// A side's flood stops at more than `MAX_ROOM` cells, so a door costs
/// O(`MAX_ROOM`) rather than O(|inside|). A side cut off is too large to
/// keep, and larger than every side kept. So is a later side that runs into
/// it, which is the same component: a side flooded in full holds every cell
/// connected to it, so no later side meets it.
fn candidates(
    board: &Board,
    start: &State,
    inside: &[bool],
    ledger: &mut Ledger,
) -> Result<Vec<(Cell, Vec<Cell>)>, SizeFailure> {
    let neighbors = board.neighbors();
    let cells = neighbors.len();
    let mut marked = ledger.filled(cells, false)?;
    let goals = board.goals().iter().map(|(cell, _)| cell);
    for &cell in start.boxes[..board.labels().len()].iter().chain(goals) {
        marked[usize::from(cell)] = true;
    }
    // Per cell, `door << 2 | j` when it is on side j of the last door whose
    // sides reached it (see the const assert on `MAX_CELLS`).
    let mut stamp = ledger.filled(cells, NONE)?;
    // A door's sides, one after another, each its own flood's queue.
    let mut sides = ledger.vec(4 * MAX_ROOM)?;
    let mut out = Vec::new();
    for door in (0..cells as Cell).filter(|&door| inside[usize::from(door)]) {
        sides.clear();
        // Per side, where it ends in `sides`, and whether it was cut off.
        let (mut ends, mut big, mut count) = ([0; 4], [false; 4], 0);
        for &first in &neighbors[usize::from(door)] {
            if first == NONE || stamp[usize::from(first)] >> 2 == door {
                continue;
            }
            let (begin, tag) = (sides.len(), (door << 2) | count as Cell);
            stamp[usize::from(first)] = tag;
            sides.push(first);
            let mut at = begin;
            'flood: while let Some(&cell) = sides.get(at) {
                at += 1;
                for &next in &neighbors[usize::from(cell)] {
                    if next == NONE || next == door || stamp[usize::from(next)] == tag {
                        continue;
                    }
                    if stamp[usize::from(next)] >> 2 == door || sides.len() - begin == MAX_ROOM {
                        big[count] = true;
                        break 'flood;
                    }
                    stamp[usize::from(next)] = tag;
                    sides.push(next);
                }
            }
            ends[count] = sides.len();
            count += 1;
        }
        if count < 2 {
            continue;
        }
        let side_of = |j: usize| &sides[if j == 0 { 0 } else { ends[j - 1] }..ends[j]];
        // When a side was cut off, the largest is one that was, and which
        // one is moot.
        let largest = (0..count)
            .max_by_key(|&j| (big[j], side_of(j).len()))
            .expect("a door with sides");
        for j in (0..count).filter(|&j| j != largest && !big[j]) {
            let side = side_of(j);
            if side.iter().any(|&cell| marked[usize::from(cell)]) {
                let mut room = ledger.vec(side.len())?;
                room.extend_from_slice(side);
                room.sort_unstable();
                ledger.push(&mut out, (door, room))?;
            }
        }
    }
    ledger.release(marked);
    ledger.release(stamp);
    ledger.release(sides);
    out.sort_unstable_by_key(|(door, cells)| (cells.len(), *door, cells[0]));
    Ok(out)
}

/// The heap bytes of the hall part, which has keeper sides like the exact
/// search's heuristic. At most `MAX_CELLS` cells, so no overflow.
fn hall_bytes(board: &Board) -> usize {
    board.tiles().len() * Heuristic::bytes_per_cell(board.labels().len(), true)
}

/// Whether a room with the sorted `cells` encloses `room`. Rooms nest or are
/// disjoint (see `candidates`), so one cell of `room` tells.
fn encloses(cells: &[Cell], room: &Room) -> bool {
    cells.binary_search(&room.cells()[0]).is_ok()
}

/// A built room and its tables, one or two.
struct RoomTables {
    room: Room,
    tables: Vec<Table>,
}

impl RoomTables {
    /// The tables' bytes, all kept.
    fn kept(&self) -> usize {
        self.tables.iter().map(Table::bytes).sum()
    }

    /// The states the tables' forward searches reached.
    fn reached(&self) -> usize {
        self.tables.iter().map(Table::reached).sum()
    }

    /// Frees the room and its tables, whose bytes are no longer kept.
    fn free(&mut self, ledger: &mut Ledger) {
        let mut tables = take(&mut self.tables);
        for table in tables.drain(..) {
            table.free(ledger);
        }
        ledger.release(tables);
        ledger.free(self.room.bytes());
    }
}

/// The room a build is trying: its tables so far and the build of the next.
struct Attempt {
    room: Room,
    moves: Moves,
    /// The parts to build, `parts[next..count]` still to go: the whole
    /// room, or after it failed, its halves.
    parts: [u64; 2],
    next: usize,
    count: usize,
    /// The table being built, with its part.
    build: Option<(u64, TableBuild)>,
    tables: Vec<Table>,
    /// Whether a build size-failed.
    failed: bool,
    /// Whether the room encloses built rooms, which it replaces if its
    /// whole-room table builds.
    replaces: bool,
}

impl Attempt {
    /// After the whole room's table failed, its halves are next, unless the
    /// room would replace enclosed rooms or a half is empty, which would
    /// rebuild the whole room.
    fn split(&mut self, board: &Board, part: u64) {
        let halves = self.room.halves(board);
        if part == self.room.all() && !self.replaces && !halves.contains(&0) {
            (self.parts, self.next, self.count) = (halves, 0, 2);
        }
    }
}

/// The resumable build of the room bound, run before the search proper.
///
/// The candidates go smallest first. Each is tried unless:
/// - it encloses a room whose build size-failed, so its own would too;
/// - or the rooms it encloses reached more than `cap / ENCLOSED_SHARE`
///   states in all.
///
/// A room that encloses built rooms tries only its whole-room table, and if
/// that builds, replaces them: their tables are freed. Otherwise they stay.
/// A room that encloses none tries its whole-room table, then, if that
/// size-failed or its key was too wide, its two halves, and keeps what
/// builds. A table whose root reaches no goal state is skipped.
///
/// A size failure is a forward search past `cap`, a refused buffer, a table
/// past the kept limit, or the work budget running out. It marks the room,
/// so each room enclosing it is skipped.
///
/// A replacement never lowers the tables' sum: R(C, C, LAB_C) >= the sum of
/// R(E, P, LAB) over the enclosed rooms' tables, in every state. Each LAB
/// is in LAB_C, as each P is in C, and the parts are disjoint. An optimal
/// solution of C's relaxed problem, cut down to E's labels and cells, is a
/// solution of E's, by the table lemma's projection; it counts a move on P
/// only where C's counts that move. The hall part may drop, since the
/// pushes that end in C outside the enclosed parts become free, so the
/// larger room is a choice, the one measured stronger, not a proven gain.
/// Only the whole-room table holds the enclosed rooms' problems, so only it
/// may replace them.
pub(crate) struct RoomBuild {
    ledger: Ledger,
    /// The keeper's floor, boxes ignored.
    inside: Vec<bool>,
    /// The doors and sorted cells of the candidates, `next..` still to try.
    candidates: Vec<(Cell, Vec<Cell>)>,
    next: usize,
    /// The first cell of each room that size-failed.
    failed: Vec<Cell>,
    /// The built rooms, pairwise disjoint.
    built: Vec<RoomTables>,
    attempt: Option<Attempt>,
    /// The most states a table's forward search may reach.
    cap: usize,
    /// The work units left.
    work: usize,
}

impl RoomBuild {
    /// The build for a search whose arena may hold `node_limit` nodes, with
    /// `fixed` bytes and an arena holding `held` bytes charged to its budget
    /// of `memory_mib` MiB. `None` when no candidate qualifies, or the hall
    /// part does not fit the budget left.
    pub(crate) fn new(
        board: &Board,
        start: &State,
        node_limit: usize,
        memory_mib: usize,
        fixed: u64,
        held: u64,
    ) -> Option<Self> {
        // In u64, since usize is 32 bits on wasm32. Both fit usize, as the
        // budget does.
        let budget = memory_mib as u64 * (1 << 20);
        let limit = budget.checked_sub(fixed.saturating_add(held))?;
        let kept_limit = budget.saturating_sub(fixed) / KEPT_SHARE;
        let mut ledger = Ledger {
            live: 0,
            limit: usize::try_from(limit).ok()?,
            kept: 0,
            kept_limit: usize::try_from(kept_limit).ok()?,
        };
        // The hall part is built last, so its bytes are set aside first.
        ledger.reserve(hall_bytes(board)).ok()?;
        let neighbors = board.neighbors();
        let mut inside = ledger.filled(neighbors.len(), false).ok()?;
        let mut queue = ledger.vec(neighbors.len()).ok()?;
        inside[usize::from(start.player)] = true;
        queue.push(start.player);
        let mut at = 0;
        while let Some(&cell) = queue.get(at) {
            at += 1;
            for &next in &neighbors[usize::from(cell)] {
                if next != NONE && !inside[usize::from(next)] {
                    inside[usize::from(next)] = true;
                    queue.push(next);
                }
            }
        }
        ledger.release(queue);
        let candidates = candidates(board, start, &inside, &mut ledger).ok()?;
        if candidates.is_empty() {
            return None;
        }
        // Built rooms are distinct candidates, as are failed ones, so
        // neither list ever grows.
        let failed = ledger.vec(candidates.len()).ok()?;
        let built = ledger.vec(candidates.len()).ok()?;
        let cap = (STATES_PER_NODE * (node_limit as u64 + 1)).clamp(MIN_CAP, MAX_CAP) as usize;
        Some(Self {
            ledger,
            inside,
            candidates,
            next: 0,
            failed,
            built,
            attempt: None,
            cap,
            work: WORK_PER_CAP * cap,
        })
    }

    /// Runs the build for at most `units` work units, taking those it uses
    /// from `units`. `None` while it runs; then the room bound, or
    /// `Some(None)` when no table was built. Done, it must not be stepped
    /// again.
    pub(crate) fn step(
        &mut self,
        board: &Board,
        start: &State,
        heuristic: &Heuristic,
        units: &mut usize,
    ) -> Option<Option<RoomBound>> {
        // Each pass spends units, or moves on to a part or a candidate.
        loop {
            if self.work == 0 || (self.attempt.is_none() && self.next == self.candidates.len()) {
                return Some(self.finish(board, start));
            }
            if *units == 0 {
                return None;
            }
            let Some(attempt) = &mut self.attempt else {
                self.begin(board);
                continue;
            };
            if let Some((part, build)) = &mut attempt.build {
                let part = *part;
                let mut slice = (*units).min(self.work);
                let given = slice;
                let step = build.step(&attempt.moves, &mut slice, &mut self.ledger);
                *units -= given - slice;
                self.work -= given - slice;
                match step {
                    Ok(Step::Running) => continue,
                    Ok(Step::Done(table)) => {
                        if let Some(table) = table {
                            self.ledger.keep(table.bytes());
                            attempt.tables.push(table);
                        }
                    }
                    Err(SizeFailure) => {
                        attempt.failed = true;
                        attempt.split(board, part);
                    }
                }
                attempt.build = None;
            } else if attempt.next < attempt.count {
                let part = attempt.parts[attempt.next];
                attempt.next += 1;
                match TableBuild::new(board, heuristic, &attempt.room, start, part, self.cap) {
                    Some(build) => attempt.build = Some((part, build)),
                    None => attempt.split(board, part),
                }
            } else {
                let attempt = self.attempt.take().expect("an attempt");
                self.conclude(attempt);
            }
        }
    }

    /// Starts on the next candidate, or skips it (see `RoomBuild`).
    fn begin(&mut self, board: &Board) {
        let (door, cells) = {
            let next = &mut self.candidates[self.next];
            (next.0, take(&mut next.1))
        };
        self.next += 1;
        let poisoned = self
            .failed
            .iter()
            .any(|cell| cells.binary_search(cell).is_ok());
        let (mut replaces, mut reached, mut kept) = (false, 0, 0);
        for built in self.built.iter().filter(|b| encloses(&cells, &b.room)) {
            replaces = true;
            reached += built.reached();
            kept += built.kept();
        }
        if poisoned || reached > self.cap / ENCLOSED_SHARE {
            self.ledger.release(cells);
            return;
        }
        // The room takes over the candidate's charged cells. Its index and
        // moves are small, so they are charged once built.
        let (first, charged) = (cells[0], cells.capacity() * size_of::<Cell>());
        let room = Room::new(board, door, cells);
        let moves = Moves::new(board, &room, &self.inside);
        let tables = Vec::with_capacity(2);
        let bytes = room.bytes() - charged + moves.bytes() + tables.capacity() * size_of::<Table>();
        if self.ledger.reserve(bytes).is_err() {
            self.ledger.free(charged);
            self.failed.push(first);
            return;
        }
        if replaces {
            self.ledger.unkeep(kept);
        }
        self.attempt = Some(Attempt {
            parts: [room.all(), 0],
            room,
            moves,
            next: 0,
            count: 1,
            build: None,
            tables,
            failed: false,
            replaces,
        });
    }

    /// Keeps or drops the tables of a finished attempt (see `RoomBuild`).
    fn conclude(&mut self, attempt: Attempt) {
        let Attempt {
            room,
            moves,
            build,
            tables,
            failed,
            replaces,
            ..
        } = attempt;
        debug_assert!(build.is_none());
        self.ledger.free(moves.bytes());
        if failed {
            self.failed.push(room.cells()[0]);
        }
        let Self { built, ledger, .. } = self;
        let keep = !tables.is_empty();
        if replaces && keep {
            built.retain_mut(|old| {
                let enclosed = encloses(room.cells(), &old.room);
                if enclosed {
                    old.free(ledger);
                }
                !enclosed
            });
        } else if replaces {
            let enclosed = built.iter().filter(|old| encloses(room.cells(), &old.room));
            ledger.keep(enclosed.map(RoomTables::kept).sum());
        }
        if keep {
            built.push(RoomTables { room, tables });
        } else {
            ledger.release(tables);
            ledger.free(room.bytes());
        }
    }

    /// Ends the build, the work budget spent or every candidate tried: the
    /// room bound of the rooms built, if any.
    fn finish(&mut self, board: &Board, start: &State) -> Option<RoomBound> {
        if let Some(mut attempt) = self.attempt.take() {
            if let Some((_, mut build)) = attempt.build.take() {
                build.free(&mut self.ledger);
                attempt.failed = true;
            }
            self.conclude(attempt);
        }
        if self.built.is_empty() {
            return None;
        }
        let mut rooms = take(&mut self.built);
        rooms.shrink_to_fit();
        let counted = &mut self.inside;
        counted.fill(false);
        for built in &rooms {
            for table in &built.tables {
                for (q, &cell) in built.room.cells().iter().enumerate() {
                    if bit(table.part(), q) {
                        counted[usize::from(cell)] = true;
                    }
                }
            }
        }
        let hall = Heuristic::hall(board, start.player, counted);
        let tables: usize = rooms
            .iter()
            .map(|b| b.room.bytes() + b.tables.capacity() * size_of::<Table>() + b.kept())
            .sum();
        let bytes = rooms.capacity() * size_of::<RoomTables>() + tables + hall_bytes(board);
        Some(RoomBound {
            rooms,
            hall,
            bytes: bytes as u64,
        })
    }
}

/// The parent of an expansion, for the hall part's incremental estimate:
/// its M_hall once a child needs it, and the hall's group cache.
pub(crate) struct RoomParent {
    hall_h: Option<u32>,
    group: ParentGroup,
}

impl RoomParent {
    pub(crate) const EMPTY: Self = Self {
        hall_h: None,
        group: ParentGroup::EMPTY,
    };
}

/// The built room tables and the hall part, which the search keeps.
pub(crate) struct RoomBound {
    rooms: Vec<RoomTables>,
    hall: Heuristic,
    /// The heap bytes all of it holds.
    bytes: u64,
}

impl RoomBound {
    /// The heap bytes the tables, rooms and hall part hold.
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The tables' sum for `state`, `None` when a projection is missing.
    fn tables_sum(&self, state: &State) -> Option<u32> {
        let mut sum = 0;
        for built in &self.rooms {
            for table in &built.tables {
                sum += table.value(&built.room, state)?;
            }
        }
        Some(sum)
    }

    /// S of `state` from scratch, `None` when a projection is missing.
    pub(crate) fn root(&self, state: &State) -> Option<u32> {
        Some(self.tables_sum(state)? + self.hall.estimate(state)?)
    }

    /// S of `child`, the state box `i` of `parent` reaches when pushed in
    /// direction `d` onto `to`, `None` when a projection is missing. The
    /// hall part updates the parent's incrementally, through `cache`, which
    /// every child of one expansion shares.
    pub(crate) fn child_estimate(
        &self,
        parent: &State,
        cache: &mut RoomParent,
        child: &State,
        (i, to, d): (usize, Cell, usize),
    ) -> Option<u32> {
        let tables = self.tables_sum(child)?;
        let heuristic = &self.hall;
        // M_hall prices pushes no higher than M does over the same moves,
        // so it is feasible wherever M is, in every queued state.
        let parent_hall = *cache.hall_h.get_or_insert_with(|| {
            heuristic
                .estimate(parent)
                .expect("a queued state's hall estimate is feasible")
        });
        let hall = self
            .hall
            .child_estimate(parent_hall, &mut cache.group, parent, i, to, d)?;
        Some(tables + hall)
    }
}
