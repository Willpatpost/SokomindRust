//! The room bound of the exact search (docs/room-bound.md).
//!
//! Each room of at most `MAX_DOORS` doors that holds a start box or a goal
//! gets tables R(Q, P, LAB) over disjoint parts P of Q (see `table`). The
//! hall heuristic, which prices the moves that end outside every part,
//! adds to their sum S, and the exact search queues max(M, S). At the
//! capped work scale the walk term joins S — one LP at the start, carried
//! per state through its duals, pricing the off-counted pushes and keeper
//! walks jointly (see `walk_term`).
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

use crate::heuristic::{Heuristic, ParentGroup, walk_term::WalkTerm};
use room::{MAX_ROOM, Moves, Room};
use sokomind_core::{Board, Cell, MAX_CELLS, NONE, State};
use std::mem::{size_of, take};
use table::{MAX_VALUE, Step, Table, TableBuild};

/// The build work units each pop slot of `Search::advance` pays for while
/// the build runs, so a time check between calls bounds the build like
/// pops. A unit expands, pops or scans about one state of a table's search
/// (see `TableBuild::step`).
pub(crate) const BUILD_UNITS_PER_POP: usize = 64;
/// The least state budget the room tables build at: the arena's node
/// limit, which the declared states and the memory budget both cap, must
/// reach it. Below, a search dies before the tables' pruning repays their
/// build: at the benchmark gate's 20,000 states the tables' largest saving
/// is 9,142 expanded (expert-maze), about 25 ms of search, against builds
/// of hundreds of milliseconds (a ten-state cap measured expert-maze at
/// 301 ms and huge at 1.48 s), while at observe's 1,000,000 states large
/// saves 130,425 expanded and repays its build. The bound is pruning
/// only, so a search below it proves the same routes with more
/// expansions, and a capped one reports the start bound of M alone.
pub(crate) const BUILD_MIN_STATES: usize = 1_000_000;
/// A table's forward search may reach this many states per node the arena
/// may hold, so the build scales with the search it serves, ...
const STATES_PER_NODE: u64 = 16;
/// ... clamped to `MIN_CAP..=MAX_CAP`.
const MIN_CAP: u64 = 1 << 20;
const MAX_CAP: u64 = 1 << 24;
/// The build's work budget, in units, is this many caps.
const WORK_PER_CAP: usize = 4;
/// The wide candidates — four and five doors — draw their build work from
/// a share of at least this many caps, beyond the rooms' `WORK_PER_CAP`:
/// whatever the rooms' budget leaves once their candidates run out flows
/// to the wide phase, which adds this share to it, so the rooms cannot
/// starve the wide candidates however much of the budget they spend. The
/// build order puts the wider candidates last, so the rooms draw from
/// their share first, and the wide phase never shortens it.
const WIDE_WORK_PER_CAP: usize = 1;
/// The fewest doors a candidate needs to draw from the wide work share.
const WIDE_DOORS: usize = 4;
/// A room is skipped when the tables of the rooms it encloses reached more
/// than the cap over this many states: its own search, which holds theirs,
/// would very likely pass the cap and waste its work.
const ENCLOSED_SHARE: usize = 16;
/// The kept tables may take the budget left after the fixed bytes over this.
const KEPT_SHARE: u64 = 4;
/// The most doors a room may have. Every door widens the relaxed problem
/// the tables over the room solve — the keeper leaves and returns at any
/// of them — so a bound behind many doors says little; five is where the
/// regions the wide pass finds still pay (docs/room-bound.md, Rooms with
/// several doors).
const MAX_DOORS: usize = 5;
/// The most cells the door pool may hold before the multi-door candidates
/// are skipped: the cuts the pool makes are tried in sorted order under a
/// budget, and a giant pool would only ever reach its start.
const MAX_POOL: usize = 512;
/// The most cuts the pool's pairs may try, in sorted order. Trying a cut
/// costs one flood of the floor, so a board with a giant pool cannot try
/// them all.
const MAX_CUTS: usize = 8_192;
/// The most cells the pairs' cuts' floods and scans may visit in all, so
/// the enumeration cannot spend the caller's time on a giant board: each
/// cut scans the floor once and floods the components it finds.
const MAX_STEPS: usize = 1 << 21;
/// The most cuts the triple pass may try, in sorted order. It runs beside
/// the pairs with budgets of its own, so neither pass starves the other
/// where a board's pool is large.
const TRIPLE_CUTS: usize = 1 << 17;
/// The most cells the triple pass's cuts' floods and scans may visit in
/// all, for the same reason as `MAX_STEPS`. It carries the pass's real
/// cost — each cut scans the floor once — and is set so a pool of a few
/// hundred cells reaches the triples deep in its enumeration, where the
/// regions behind three-cell necks sit, while a giant pool still stops
/// early.
const TRIPLE_STEPS: usize = 1 << 23;
/// The most combinations of pool cells the triple pass's enumeration may
/// examine, span-filtered or not, so a giant pool cannot spend the
/// caller's time even before a cut is tried.
const TRIPLE_COMBOS: usize = 1 << 21;
/// The most cuts the wide pass — the four- and five-door cuts — may try,
/// in sorted order, under budgets of its own like the triple pass's.
/// Set with `WIDE_STEPS` so a small board's sweep reaches the spans its
/// door bands sit at, deep in the enumeration, while a giant pool still
/// stops early.
const WIDE_CUTS: usize = 1 << 16;
/// The most cells the wide pass's cuts' floods and scans may visit in
/// all, for the same reason as `TRIPLE_STEPS`, and sized with
/// `WIDE_CUTS`: a small board's spans cluster low, so the sweep spends
/// its budget before the widest spans, where the regions behind bands of
/// doors two wide sit.
const WIDE_STEPS: usize = 1 << 24;
/// The most combinations the wide pass's enumeration may examine, for
/// the same reason as `TRIPLE_COMBOS`.
const WIDE_COMBOS: usize = 1 << 21;
/// The widest span, along either axis, between the cells of a wide cut.
/// A region behind a band of doors two wide is compact, or the band
/// leaves gaps the flood would cross, so its doors sit close together;
/// the raw four- and five-cell combinations are too many to examine, and
/// this bounds them to the neighborhoods that isolate anything.
const WIDE_SPAN: usize = 16;

// Rooms are disjoint, so there are at most `MAX_CELLS`, each with at most
// two tables, and the tables' sum fits u32 with room for the hall part.
const _: () = assert!(2 * MAX_CELLS as u64 * MAX_VALUE as u64 <= u32::MAX as u64 / 2);
// `candidates` stamps a cell `door << 2 | side`, which must fit a Cell and
// stay apart from `NONE`.
const _: () = assert!(MAX_CELLS << 2 <= NONE as usize);

/// One candidate room: its doors and its cells, both sorted.
type Candidate = (Vec<Cell>, Vec<Cell>);

/// Whether bit `i` of `mask` is set.
fn bit(mask: u64, i: usize) -> bool {
    (mask >> i) & 1 != 0
}

/// A room or table that does not fit: the ledger or the allocator refused
/// a buffer, or the build passed a cap.
#[derive(Debug)]
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

/// The candidate rooms: the one-door rooms, then the rooms of more doors.
///
/// The one-door rooms: for each cell of `inside` taken as a door, each
/// side of it but the largest that holds a start box or a goal and has at
/// most `MAX_ROOM` cells. A side is a component of `inside` less the door.
///
/// The one-door candidates are laminar: any two nest or are disjoint. Let
/// T be |inside|. A candidate Q is not its door's largest side, and the
/// sides partition `inside` less the door, so 2|Q| <= T - 1. Suppose Q1
/// and Q2, with doors d1 and d2, meet but neither holds the other. Q2 is
/// connected and has cells in and out of Q1, and d1 is the only cell
/// outside Q1 next to it, so d1 is in Q2; likewise d2 is in Q1. `inside`
/// less Q1 is d1 and its other sides, which all touch d1, so it is
/// connected. It misses d2, so it lies in one side of d2, the one holding
/// d1: Q2. Then |Q1| + |Q2| >= T, against 2|Q1| <= T - 1 and
/// 2|Q2| <= T - 1. Nor are two candidates equal: a room's cells fix its
/// door, the one cell outside them next to them. So a one-door room built
/// before Q holds Q, lies inside Q, or lies apart from it, and one cell of
/// the smaller of the two tells which.
///
/// The rooms of more doors, at most `MAX_DOORS`: a region whose boundary —
/// the floor cells outside it next to it — is the cut that isolates it,
/// found by `try_cut`. Their doors come from a pool of the floor cells
/// within `MAX_ROOM` steps of a box or a goal with two floor neighbors at
/// least: a kept region holds a box or a goal, so each of its doors
/// stands within `MAX_ROOM` steps of one, and a cell with one floor
/// neighbor cannot close a region that keeps it, since dropping the cell
/// leaves its neighbor's region whole. Two doors are tried as every pair
/// of pool cells within `MAX_ROOM` of each other, under `MAX_CUTS` cuts
/// and `MAX_STEPS` visited cells; three, as every triple of pool cells
/// pairwise within `MAX_ROOM`, under budgets of the triple pass's own —
/// `TRIPLE_CUTS` cuts, `TRIPLE_STEPS` visited cells and `TRIPLE_COMBOS`
/// examined combinations — so neither pass starves the other. Four doors
/// are tried as every two disjoint adjacent pairs of pool cells within a
/// `WIDE_SPAN` box, and five as every two such pairs with one pool cell
/// more, under `WIDE_CUTS` cuts, `WIDE_STEPS` visited cells and
/// `WIDE_COMBOS` examined combinations: a region behind a band of doors
/// two wide is isolated by no narrower cut, and the raw four- and
/// five-cell combinations are too many to examine. A pool
/// past `MAX_POOL` cells yields no multi-door candidates at all, so a
/// giant board cannot pay for them, and the triple enumeration stops at
/// its combination budget, so not every three-door region is sought.
///
/// A candidate of several doors can cross another — the one-door
/// candidates nest or are disjoint, but a wider cut can share cells with a
/// room without holding it — so the build skips a candidate that crosses a
/// room it has built (see `RoomBuild`), and the rooms it keeps stay
/// pairwise disjoint, their tables' sum sound.
///
/// Each candidate comes with its sorted cells and its sorted doors, and
/// the list runs the one-door rooms first — in the order the build always
/// gave them, by boundary width, size, first door, then first cell — and
/// the wider rooms behind them, so the build tries what it always tried
/// before anything new, and the wide candidates draw from a work share
/// the rooms cannot starve (see `WIDE_WORK_PER_CAP`).
///
/// The board's single-door rooms, for one-shot strategic analyses: each
/// room's door and its cells, from the same candidate detection the
/// table build uses, flooded over the keeper's floor the same way.
/// Only a single-door room serializes its traffic, so candidates with
/// wider necks are dropped. A detection that cannot pay its scratch
/// ledger returns no rooms, and the caller runs without its analysis
/// rather than failing the search.
pub(crate) fn gated_rooms(board: &Board, start: &State) -> Vec<(Vec<Cell>, Vec<Cell>)> {
    let neighbors = board.neighbors();
    let mut inside = vec![false; neighbors.len()];
    let mut queue = vec![start.player];
    inside[usize::from(start.player)] = true;
    let mut at = 0;
    while at < queue.len() {
        let cell = queue[at];
        at += 1;
        for &next in &neighbors[usize::from(cell)] {
            if next != NONE && !inside[usize::from(next)] {
                inside[usize::from(next)] = true;
                queue.push(next);
            }
        }
    }
    let mut ledger = Ledger {
        live: 0,
        limit: 1 << 22,
        kept: 0,
        kept_limit: 1 << 22,
    };
    candidates(board, start, &inside, &mut ledger)
        .unwrap_or_default()
        .into_iter()
        .filter(|(doors, _)| doors.len() == 1)
        .collect()
}

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
) -> Result<Vec<Candidate>, SizeFailure> {
    let neighbors = board.neighbors();
    let cells = neighbors.len();
    let mut marked = ledger.filled(cells, false)?;
    let goals = board.goals().iter().map(|(cell, _)| cell);
    let mut seeds = Vec::new();
    for &cell in start.boxes[..board.labels().len()].iter().chain(goals) {
        marked[usize::from(cell)] = true;
        ledger.push(&mut seeds, cell)?;
    }
    // Per cell, `door << 2 | j` when it is on side j of the last door whose
    // sides reached it (see the const assert on `MAX_CELLS`).
    let mut stamp = ledger.filled(cells, NONE)?;
    // A door's sides, one after another, each its own flood's queue.
    let mut sides = ledger.vec(4 * MAX_ROOM)?;
    let mut out: Vec<Candidate> = Vec::new();
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
                let mut doors = ledger.vec(1)?;
                doors.push(door);
                ledger.push(&mut out, (doors, room))?;
            }
        }
    }
    ledger.release(stamp);
    ledger.release(sides);
    // The door pool and the cuts it makes (see the function docs).
    let total = inside.iter().filter(|&&inside| inside).count();
    let mut epoch = ledger.filled(cells, 0u32)?;
    let mut queue = ledger.vec(cells)?;
    for &seed in &seeds {
        epoch[usize::from(seed)] = 1;
        queue.push(seed);
    }
    let mut at = 0;
    while let Some(&cell) = queue.get(at) {
        at += 1;
        let depth = epoch[usize::from(cell)];
        for &next in &neighbors[usize::from(cell)] {
            if next != NONE && epoch[usize::from(next)] == 0 && depth < MAX_ROOM as u32 {
                epoch[usize::from(next)] = depth + 1;
                queue.push(next);
            }
        }
    }
    let mut pool = Vec::new();
    for cell in 0..cells {
        let degree = neighbors[cell].iter().filter(|&&next| next != NONE).count();
        if inside[cell] && epoch[cell] != 0 && degree >= 2 {
            ledger.push(&mut pool, cell as Cell)?;
        }
    }
    let mut cuts = Cuts {
        board,
        inside,
        marked: &marked,
        total,
        queue,
        epoch,
        age: 1 << 20,
    };
    let mut cut = ledger.vec(MAX_DOORS)?;
    let mut tried = 0;
    let mut steps = MAX_STEPS;
    if !pool.is_empty() && pool.len() <= MAX_POOL {
        let width = board.width();
        let span = |a: Cell, b: Cell| {
            let (a, b) = (usize::from(a), usize::from(b));
            (a / width)
                .abs_diff(b / width)
                .max((a % width).abs_diff(b % width))
        };
        'pairs: for a in 0..pool.len() {
            for b in (a + 1)..pool.len() {
                if span(pool[a], pool[b]) > MAX_ROOM {
                    continue;
                }
                tried += 1;
                if tried > MAX_CUTS {
                    break 'pairs;
                }
                cut.clear();
                cut.push(pool[a]);
                cut.push(pool[b]);
                cuts.try_cut(&cut, &mut out, ledger, &mut steps)?;
            }
        }
        // The three-door cuts: every triple of pool cells pairwise within
        // a `MAX_ROOM` box, in pool order. A region behind a three-cell
        // neck is isolated by no pair, so the pairs alone miss it, and
        // the growth that ran here before grew only from the two-door
        // candidates the pairs found.
        let (mut triples, mut examined) = (0, 0);
        let mut steps = TRIPLE_STEPS;
        'triples: for a in 0..pool.len() {
            for b in (a + 1)..pool.len() {
                if span(pool[a], pool[b]) > MAX_ROOM {
                    continue;
                }
                for c in (b + 1)..pool.len() {
                    examined += 1;
                    if examined > TRIPLE_COMBOS {
                        break 'triples;
                    }
                    if span(pool[a], pool[c]) > MAX_ROOM || span(pool[b], pool[c]) > MAX_ROOM {
                        continue;
                    }
                    triples += 1;
                    if triples > TRIPLE_CUTS {
                        break 'triples;
                    }
                    cut.clear();
                    cut.push(pool[a]);
                    cut.push(pool[b]);
                    cut.push(pool[c]);
                    cuts.try_cut(&cut, &mut out, ledger, &mut steps)?;
                }
            }
        }
        // The wide cuts: every two disjoint adjacent pairs of pool cells,
        // tried in order of the cut's span — the widest Chebyshev gap
        // between its cells — so the tight door bands run first and the
        // budgets die in the tail rather than the front. Then every
        // four-door candidate's doors with one pool cell more, which
        // finds the regions a wider cut of a found region isolates:
        // Grand Hall's middle band, two neck pairs and the chimney cell.
        // A region behind a band of doors two wide is isolated by no
        // narrower cut, and the raw four- and five-cell combinations are
        // too many to examine.
        let mut pairs = Vec::new();
        for (a, &cell) in pool.iter().enumerate() {
            for &next in &neighbors[usize::from(cell)] {
                if next != NONE && pool[..a].contains(&next) {
                    ledger.push(&mut pairs, (next, cell))?;
                }
            }
        }
        let (mut wide, mut examined) = (0, 0);
        let mut steps = WIDE_STEPS;
        'span: for s in 1..=WIDE_SPAN {
            for p in 0..pairs.len() {
                for q in (p + 1)..pairs.len() {
                    let (left, right) = (pairs[p], pairs[q]);
                    if left.0 == right.0
                        || left.0 == right.1
                        || left.1 == right.0
                        || left.1 == right.1
                    {
                        continue;
                    }
                    let band = span(left.0, right.0)
                        .max(span(left.0, right.1))
                        .max(span(left.1, right.0))
                        .max(span(left.1, right.1));
                    if band != s {
                        continue;
                    }
                    examined += 1;
                    if examined > WIDE_COMBOS {
                        break 'span;
                    }
                    wide += 1;
                    if wide > WIDE_CUTS {
                        break 'span;
                    }
                    cut.clear();
                    cut.extend_from_slice(&[left.0, left.1, right.0, right.1]);
                    cut.sort_unstable();
                    cuts.try_cut(&cut, &mut out, ledger, &mut steps)?;
                }
            }
        }
        // The five-door cuts: each four-door candidate's doors with one
        // pool cell more.
        let mut grown: Vec<[Cell; 4]> = Vec::new();
        for (doors, _) in out.iter().filter(|(doors, _)| doors.len() == 4) {
            ledger.push(&mut grown, [doors[0], doors[1], doors[2], doors[3]])?;
        }
        'grown: for doors in &grown {
            for &extra in &pool {
                if doors.contains(&extra) {
                    continue;
                }
                examined += 1;
                if examined > WIDE_COMBOS {
                    break 'grown;
                }
                wide += 1;
                if wide > WIDE_CUTS {
                    break 'grown;
                }
                cut.clear();
                cut.extend_from_slice(doors);
                cut.push(extra);
                cuts.try_cut(&cut, &mut out, ledger, &mut steps)?;
            }
        }
        ledger.release(grown);
        // The five-door cuts no four-door cut grows from — a pocket
        // behind five neck cells, whose every four cells leave one
        // connecting — as every two adjacent pairs with one pool cell
        // more, tried at the cut's own span so the tight necks run first
        // and the budgets die in the tail.
        'raw: for s in 1..=WIDE_SPAN {
            for p in 0..pairs.len() {
                for q in (p + 1)..pairs.len() {
                    let (left, right) = (pairs[p], pairs[q]);
                    if left.0 == right.0
                        || left.0 == right.1
                        || left.1 == right.0
                        || left.1 == right.1
                    {
                        continue;
                    }
                    let band = span(left.0, right.0)
                        .max(span(left.0, right.1))
                        .max(span(left.1, right.0))
                        .max(span(left.1, right.1));
                    if band > s {
                        continue;
                    }
                    for &extra in &pool {
                        if extra == left.0
                            || extra == left.1
                            || extra == right.0
                            || extra == right.1
                        {
                            continue;
                        }
                        let total = band
                            .max(span(left.0, extra))
                            .max(span(left.1, extra))
                            .max(span(right.0, extra))
                            .max(span(right.1, extra));
                        if total != s {
                            continue;
                        }
                        examined += 1;
                        if examined > WIDE_COMBOS {
                            break 'raw;
                        }
                        wide += 1;
                        if wide > WIDE_CUTS {
                            break 'raw;
                        }
                        cut.clear();
                        cut.extend_from_slice(&[left.0, left.1, right.0, right.1, extra]);
                        cut.sort_unstable();
                        cuts.try_cut(&cut, &mut out, ledger, &mut steps)?;
                    }
                }
            }
        }
        ledger.release(pairs);
    }
    ledger.release(cut);
    ledger.release(pool);
    ledger.release(cuts.queue);
    ledger.release(cuts.epoch);
    ledger.release(seeds);
    ledger.release(marked);
    // The passes find one region through several cuts; the first copy in
    // cell order stays, and among equal cells the narrowest cut's, so a
    // region a wide cut also isolates keeps the doors of its narrowest
    // cut and its place in the build's order.
    out.sort_unstable_by(|a, b| a.1.cmp(&b.1).then(a.0.len().cmp(&b.0.len())));
    let mut at = 0;
    while at + 1 < out.len() {
        if out[at].1 == out[at + 1].1 {
            let (doors, cells) = out.remove(at + 1);
            ledger.release(doors);
            ledger.release(cells);
        } else {
            at += 1;
        }
    }
    out.sort_unstable_by_key(|(doors, cells)| {
        (
            doors.len(),
            boundary_width(board, doors, cells),
            cells.len(),
            doors[0],
            cells[0],
        )
    });
    Ok(out)
}

/// The scratch the multi-door cuts share: the board they cut, the floor
/// and the marked cells a kept region holds, the flood's stamps and
/// queue, and the floor's cell count. `candidates` tries one cut at a
/// time through [`Self::try_cut`], each pass under the cut and cell
/// budgets it passes down.
struct Cuts<'a> {
    board: &'a Board,
    inside: &'a [bool],
    marked: &'a [bool],
    total: usize,
    /// Per cell, the pool flood's depth, at most `MAX_ROOM`, then every
    /// cut's component stamps, which start past every depth and only
    /// grow.
    epoch: Vec<u32>,
    /// The next component's stamp.
    age: u32,
    /// The flood's queue.
    queue: Vec<Cell>,
}

impl Cuts<'_> {
    /// Adds to `out` every component of the floor less `cut` that is a
    /// candidate: sorted, at most `MAX_ROOM` cells, holding a marked cell,
    /// no larger than the rest, and with `cut` for its whole boundary:
    /// every cut cell stands next to the component, and every floor cell
    /// next to it lies in it or in the cut, which the flood's stamps could
    /// otherwise carve around. The largest component floods in full
    /// however big, so one cut costs O(|inside|); the whole pass stops
    /// when its caller's cell budget runs out.
    fn try_cut(
        &mut self,
        cut: &[Cell],
        out: &mut Vec<Candidate>,
        ledger: &mut Ledger,
        steps: &mut usize,
    ) -> Result<(), SizeFailure> {
        let neighbors = self.board.neighbors();
        // The stamps of this cut's components run from `begin` up; an
        // earlier cut's are older, and the pool's depths younger, so a
        // cell stamped past `begin` belongs to a component this cut
        // already flooded.
        let begin = self.age + 1;
        for cell in (0..self.inside.len() as Cell).filter(|&cell| self.inside[usize::from(cell)]) {
            if *steps == 0 {
                return Ok(());
            }
            // A component's flood may overrun the budget's end, as the
            // one-door pass's sides do; the next one does not start.
            *steps = steps.saturating_sub(1);
            let first = usize::from(cell);
            if self.epoch[first] >= begin || cut.contains(&cell) {
                continue;
            }
            self.age += 1;
            self.queue.clear();
            self.queue.push(cell);
            self.epoch[first] = self.age;
            let mut at = 0;
            let mut big = false;
            while let Some(&c) = self.queue.get(at) {
                at += 1;
                for &next in &neighbors[usize::from(c)] {
                    let n = usize::from(next);
                    if next == NONE || self.epoch[n] >= begin || cut.contains(&next) {
                        continue;
                    }
                    self.epoch[n] = self.age;
                    self.queue.push(next);
                    *steps = steps.saturating_sub(1);
                    if self.queue.len() > MAX_ROOM {
                        big = true;
                        break;
                    }
                }
                if big {
                    break;
                }
            }
            if big {
                continue;
            }
            let boundary = cut
                .iter()
                .filter(|&&door| {
                    neighbors[usize::from(door)]
                        .iter()
                        .any(|&c| c != NONE && self.epoch[usize::from(c)] == self.age)
                })
                .count();
            // The flood stops at the cut, the walls and the cells an
            // earlier component of this cut stamped, and a big component
            // stamps only its first `MAX_ROOM` cells, so a cut that splits
            // nothing can still carve one into pieces a later flood
            // finishes. Such a piece's boundary holds floor the cut does
            // not name, and the room it would make would lie about its
            // doors (the one-door pass drops such a side as big; see its
            // flood). A true component's boundary can only hold cut cells
            // — any other floor cell next to it would be part of it — so
            // every floor cell next to the piece must lie in it or in the
            // cut.
            let sealed = self.queue.iter().all(|&c| {
                neighbors[usize::from(c)].iter().all(|&next| {
                    next == NONE || cut.contains(&next) || self.epoch[usize::from(next)] == self.age
                })
            });
            if !sealed
                || boundary != cut.len()
                || !self.queue.iter().any(|&c| self.marked[usize::from(c)])
                || 2 * self.queue.len() > self.total - cut.len()
            {
                continue;
            }
            let mut cells = ledger.vec(self.queue.len())?;
            cells.extend_from_slice(&self.queue);
            cells.sort_unstable();
            let mut doors = ledger.vec(cut.len())?;
            doors.extend_from_slice(cut);
            doors.sort_unstable();
            ledger.push(out, (doors, cells))?;
        }
        Ok(())
    }
}

/// The heap bytes of the hall part, which has keeper sides like the exact
/// search's heuristic. At most `MAX_CELLS` cells, so no overflow.
fn hall_bytes(board: &Board) -> usize {
    board.tiles().len() * Heuristic::bytes_per_cell(board.labels().len(), true)
}

/// Whether a room with the sorted `cells` encloses `room`, given that `room`
/// does not hold it. Rooms nest or are disjoint (see `candidates`), so one
/// cell of `room` tells.
fn encloses(cells: &[Cell], room: &Room) -> bool {
    cells.binary_search(&room.cells()[0]).is_ok()
}

/// The width of `door` for a room with the sorted `cells`: the door's
/// neighbors in the room, 1 to 3, since one of its sides is never a room.
fn door_width(board: &Board, door: Cell, cells: &[Cell]) -> u8 {
    let neighbors = &board.neighbors()[usize::from(door)];
    neighbors
        .iter()
        .filter(|&&cell| cell != NONE && cells.binary_search(&cell).is_ok())
        .count() as u8
}

/// The width of a boundary, for the build's order and its poison: the
/// doors' neighbors in the room summed, the ways a box may enter it. With
/// one door it is that door's width alone.
fn boundary_width(board: &Board, doors: &[Cell], cells: &[Cell]) -> u8 {
    doors
        .iter()
        .map(|&door| door_width(board, door, cells))
        .sum()
}

/// Whether `a` and `b` share a cell without one holding the other. Both
/// are sorted; a shared run as long as the shorter list means the shorter
/// lies in the longer.
fn crosses(a: &[Cell], b: &[Cell]) -> bool {
    let (mut i, mut j, mut shared) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            shared += 1;
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    shared > 0 && shared != a.len().min(b.len())
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
    /// The door's width (see `RoomBuild`).
    width: u8,
    /// Whether the room draws from the wide work share (see
    /// `WIDE_WORK_PER_CAP`).
    wide: bool,
    /// Whether the room encloses built rooms, which it replaces if its
    /// whole-room table builds.
    replaces: bool,
}

impl Attempt {
    /// After the whole room's table failed, its halves are next, unless the
    /// room would replace enclosed rooms. A half is tried only when it holds
    /// some, but not all, of the room's labels. With none it has no table.
    /// With all it differs from the whole room only in which moves count, so
    /// its key, forward search and kept size are the whole room's, and it
    /// would very likely fail again. An empty half leaves the other with all.
    fn split(&mut self, board: &Board, start: &State, part: u64) {
        if part != self.room.all() || self.replaces {
            return;
        }
        let labels = self.room.labels(board, start, part);
        let (mut parts, mut count) = ([0; 2], 0);
        for half in self.room.halves(board) {
            let some = self.room.labels(board, start, half);
            if !some.is_empty() && some != labels {
                parts[count] = half;
                count += 1;
            }
        }
        (self.parts, self.next, self.count) = (parts, 0, count);
    }
}

/// The resumable build of the room bound, run before the search proper.
///
/// The candidates go narrowest boundary first, then smallest. Each is
/// tried unless:
/// - it lies inside a built room, whose parts already hold its cells;
/// - it crosses a built room, sharing cells without holding it, which only
///   a multi-door candidate can (see `candidates`);
/// - it encloses a room whose build size-failed behind a boundary no wider
///   than its own, so its own would very likely fail too;
/// - or the rooms it encloses reached more than `cap / ENCLOSED_SHARE`
///   states in all.
///
/// A boundary's width is its doors' neighbors in the room summed. Boxes
/// enter a room behind a narrower boundary in fewer ways, so it may hold
/// far fewer states than a room it encloses behind a wider one. On huge,
/// the 26-cell room behind a width-1 door builds in 10.3M states per
/// table, while the 25-cell room inside it, behind a width-3 door, passes
/// the cap or the memory with both halves. Smallest first, the build paid
/// for those failures before the outer room; narrowest first builds the
/// outer room, then skips the inner one, which lies inside it.
///
/// A room that encloses built rooms tries only its whole-room table, and if
/// that builds, replaces them: their tables are freed. Otherwise they stay.
/// A room that encloses none tries its whole-room table, then, if that
/// size-failed or its key was too wide, each of its two halves that holds
/// some but not all of its labels (see `Attempt::split`), and keeps what
/// builds. A table whose root reaches no goal state is skipped. A
/// multi-door room's tables are kept only when they raise S at the start
/// over the rooms already kept (see `conclude`).
///
/// A size failure is a forward search past `cap`, a refused buffer, a table
/// past the kept limit, or a work share running out. It marks the room,
/// so each room enclosing it behind a door at least as wide is skipped.
///
/// A replacement never lowers the tables' sum: R(C, C, LAB_C) >= the sum of
/// R(E, P, LAB) over the enclosed rooms' tables, in every state. Each LAB
/// is in LAB_C, as each P is in C, and the parts are disjoint. An optimal
/// solution of C's relaxed problem, cut down to E's labels and cells, is a
/// solution of E's, by the table lemma's projection; it counts a move on P
/// only where C's counts that move. The hall part may drop, since the
/// moves that end in C outside the enclosed parts become free, so the
/// larger room is a choice, the one measured stronger, not a proven gain.
/// Only the whole-room table holds the enclosed rooms' problems, so only it
/// may replace them.
pub(crate) struct RoomBuild {
    ledger: Ledger,
    /// The keeper's floor, boxes ignored.
    inside: Vec<bool>,
    /// The doors and sorted cells of the candidates, `next..` still to try.
    candidates: Vec<Candidate>,
    next: usize,
    /// The first cell, door width and size of each room that size-failed.
    failed: Vec<(Cell, u8, usize)>,
    /// The built rooms, pairwise disjoint.
    built: Vec<RoomTables>,
    attempt: Option<Attempt>,
    /// The most states a table's forward search may reach.
    cap: usize,
    /// The work units left for the rooms of at most three doors.
    work: usize,
    /// The work units left for the wide candidates (see
    /// `WIDE_WORK_PER_CAP`).
    wide_work: usize,
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
            wide_work: WIDE_WORK_PER_CAP * cap,
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
        // A spent share ends only its own candidates: the rooms' work
        // running out skips the rooms left and goes on to the wide share,
        // and a spent wide share skips the wide candidates left. An
        // attempt whose share ran out ends as one the budget could not
        // finish (see `abandon`), keeping what it built.
        loop {
            if self
                .attempt
                .as_ref()
                .is_some_and(|attempt| self.share_spent(attempt.wide))
            {
                self.abandon(board, start);
                continue;
            }
            if self.attempt.is_none() {
                // Once the wide candidates begin, the rooms' leftover
                // flows to their share: the share is a floor the rooms'
                // budget cannot starve, not a cap on what its leftover
                // would have paid for. The fold is idempotent, for no
                // wide candidate draws from the rooms' pool again.
                if self.next < self.candidates.len()
                    && self.candidates[self.next].0.len() >= WIDE_DOORS
                {
                    self.wide_work += self.work;
                    self.work = 0;
                }
                while self.next < self.candidates.len()
                    && self.share_spent(self.candidates[self.next].0.len() >= WIDE_DOORS)
                {
                    let (doors, cells) = {
                        let candidate = &mut self.candidates[self.next];
                        (take(&mut candidate.0), take(&mut candidate.1))
                    };
                    self.next += 1;
                    self.ledger.release(doors);
                    self.ledger.release(cells);
                }
                if self.next == self.candidates.len() {
                    return Some(self.finish(board, start));
                }
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
                let mut slice = (*units).min(if attempt.wide {
                    self.wide_work
                } else {
                    self.work
                });
                let given = slice;
                let step = build.step(&attempt.moves, &mut slice, &mut self.ledger);
                *units -= given - slice;
                if attempt.wide {
                    self.wide_work -= given - slice;
                } else {
                    self.work -= given - slice;
                }
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
                        attempt.split(board, start, part);
                    }
                }
                attempt.build = None;
            } else if attempt.next < attempt.count {
                let part = attempt.parts[attempt.next];
                attempt.next += 1;
                match TableBuild::new(board, heuristic, &attempt.room, start, part, self.cap) {
                    Some(build) => attempt.build = Some((part, build)),
                    None => attempt.split(board, start, part),
                }
            } else {
                let attempt = self.attempt.take().expect("an attempt");
                self.conclude(board, start, attempt);
            }
        }
    }

    /// Starts on the next candidate, or skips it (see `RoomBuild`).
    fn begin(&mut self, board: &Board) {
        let (doors, cells) = {
            let next = &mut self.candidates[self.next];
            (take(&mut next.0), take(&mut next.1))
        };
        self.next += 1;
        let width = boundary_width(board, &doors, &cells);
        let wide = doors.len() >= WIDE_DOORS;
        // One-door rooms nest or are disjoint, so a room meeting this one
        // holds it if larger and lies inside it if smaller. A multi-door
        // candidate can also cross a built room, sharing cells without
        // holding it or lying inside it; it is skipped, so the built rooms
        // stay pairwise disjoint and their tables add.
        let poisoned = self.failed.iter().any(|&(cell, failed, size)| {
            width >= failed && size < cells.len() && cells.binary_search(&cell).is_ok()
        });
        let inside_built = self.built.iter().any(|b| {
            b.room.cells().len() > cells.len() && b.room.cells().binary_search(&cells[0]).is_ok()
        });
        let crossing = self.built.iter().any(|b| crosses(&cells, b.room.cells()));
        let (mut replaces, mut reached, mut kept) = (false, 0, 0);
        for built in self.built.iter().filter(|b| encloses(&cells, &b.room)) {
            replaces = true;
            reached += built.reached();
            kept += built.kept();
        }
        if inside_built || crossing || poisoned || reached > self.cap / ENCLOSED_SHARE {
            self.ledger.release(doors);
            self.ledger.release(cells);
            return;
        }
        // The room takes over the candidate's charged cells and doors. Its
        // index and moves are small, so they are charged once built.
        let (first, charged) = (
            cells[0],
            (cells.capacity() + doors.capacity()) * size_of::<Cell>(),
        );
        let room = Room::new(board, doors, cells);
        let moves = Moves::new(board, &room, &self.inside);
        let tables = Vec::with_capacity(2);
        let bytes = room.bytes() - charged + moves.bytes() + tables.capacity() * size_of::<Table>();
        if self.ledger.reserve(bytes).is_err() {
            self.ledger.free(charged);
            self.failed.push((first, width, room.cells().len()));
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
            width,
            wide,
            replaces,
        });
    }

    /// Keeps or drops the tables of a finished attempt (see `RoomBuild`).
    /// A multi-door room's tables are kept only when they raise S at the
    /// start over the rooms already kept, with the rooms it would replace
    /// freed: the parts a room's tables count leave the hall part, whose
    /// charges they displace, and a table that charges less than it frees
    /// would lower the bound. The one-door rooms keep the build's old
    /// choice, and the comparison, like the replacement's, is measured at
    /// the start, not proven for every state.
    fn conclude(&mut self, board: &Board, start: &State, attempt: Attempt) {
        let Attempt {
            room,
            moves,
            build,
            tables,
            failed,
            width,
            replaces,
            ..
        } = attempt;
        debug_assert!(build.is_none());
        self.ledger.free(moves.bytes());
        if failed {
            self.failed
                .push((room.cells()[0], width, room.cells().len()));
        }
        let keep = !tables.is_empty()
            && (room.doors().len() == 1 || {
                let with = self.start_sum(board, start, Some((&room, &tables)));
                let without = self.start_sum(board, start, None);
                matches!((with, without), (Some(with), Some(without)) if with > without)
            });
        let Self { built, ledger, .. } = self;
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

    /// S at the start over the rooms kept, with `room`'s tables standing
    /// in for the rooms they enclose: the tables' values plus the hall
    /// part over the parts all of them count, less the start's keeper
    /// charge (see `RoomBound::root`). `None` when a buffer the sum needs
    /// does not fit or a projection is missing; a queued start's hall
    /// part is feasible wherever M is.
    fn start_sum(
        &mut self,
        board: &Board,
        start: &State,
        room: Option<(&Room, &[Table])>,
    ) -> Option<u32> {
        let mut counted = self.ledger.filled(board.neighbors().len(), false).ok()?;
        let mut sum = 0u32;
        let mut add = |room: &Room, tables: &[Table], counted: &mut [bool]| {
            for table in tables {
                sum = sum.saturating_add(table.value(room, start)?);
                for (q, &cell) in room.cells().iter().enumerate() {
                    if bit(table.part(), q) {
                        counted[usize::from(cell)] = true;
                    }
                }
            }
            Some(())
        };
        for built in &self.built {
            if room.is_some_and(|(room, _)| encloses(room.cells(), &built.room)) {
                continue;
            }
            add(&built.room, &built.tables, &mut counted)?;
        }
        if let Some((room, tables)) = room {
            add(room, tables, &mut counted)?;
        }
        let hall = Heuristic::hall(board, start.player, &counted);
        self.ledger.release(counted);
        Some(
            sum.saturating_add(hall.estimate(start)?)
                .saturating_sub(hall.charge(start.player)),
        )
    }

    /// Whether the work share that pays for a candidate of `wide` doors is
    /// spent (see `WIDE_WORK_PER_CAP`).
    fn share_spent(&self, wide: bool) -> bool {
        if wide {
            self.wide_work == 0
        } else {
            self.work == 0
        }
    }

    /// Concludes a running attempt whose work share ran out mid-build, as a
    /// room the budget could not finish: its in-flight build is freed, the
    /// room is marked failed, and what it already built stands to the keep
    /// test. The build goes on to the share's remaining candidates, or the
    /// other share's.
    fn abandon(&mut self, board: &Board, start: &State) {
        if let Some(mut attempt) = self.attempt.take() {
            if let Some((_, mut build)) = attempt.build.take() {
                build.free(&mut self.ledger);
                attempt.failed = true;
            }
            self.conclude(board, start, attempt);
        }
    }

    /// Ends the build, a work share spent or every candidate tried: the
    /// room bound of the rooms built, if any. The walk term is solved
    /// here, once, over the parts the kept tables count — two LPs at the
    /// start's rhs, paid before the search proper begins — but only at
    /// the capped work scale: its LPs cost minutes on the large boards
    /// only a search of a million states or more can amortize, and the
    /// cap reaches `MAX_CAP` exactly there. Smaller searches — every
    /// quick solve, every test's — keep S at the hall's own arm.
    fn finish(&mut self, board: &Board, start: &State) -> Option<RoomBound> {
        self.abandon(board, start);
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
        let walk = if self.cap >= MAX_CAP as usize {
            WalkTerm::build(board, &hall, counted, start)
        } else {
            None
        };
        let portfolio = PortfolioArm::build(board, start, self.cap, &rooms);
        let tables: usize = rooms
            .iter()
            .map(|b| b.room.bytes() + b.tables.capacity() * size_of::<Table>() + b.kept())
            .sum();
        let bytes = (rooms.capacity() * size_of::<RoomTables>() + tables + hall_bytes(board))
            as u64
            + walk.as_ref().map_or(0, WalkTerm::bytes)
            + portfolio.as_ref().map_or(0, PortfolioArm::bytes);
        Some(RoomBound {
            rooms,
            hall,
            walk,
            portfolio,
            bytes,
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

/// The native halves of the portfolio's knob: the environment is its
/// only source, and wasm has none, so the arm never builds there.
#[cfg(not(target_arch = "wasm32"))]
mod native {
    /// The rooms `SOKOMIND_PORTFOLIO_ROOMS` names, from its comma
    /// list. `None` when unset; a malformed list says so before
    /// returning `None` — a silently ignored knob is a debugging trap.
    pub(super) fn portfolio_rooms() -> Option<Vec<usize>> {
        let value = std::env::var_os("SOKOMIND_PORTFOLIO_ROOMS")?;
        let text = value.to_string_lossy();
        match parse_rooms(&text) {
            Some(rooms) => Some(rooms),
            None => {
                eprintln!(
                    "room bound: SOKOMIND_PORTFOLIO_ROOMS is malformed \
                     ({text}); the portfolio arm is off"
                );
                None
            }
        }
    }

    /// Whether the dual cache is armed: the second term's solves cost
    /// what the first's do, so the arm only builds against the cache.
    pub(super) fn dual_cache_armed() -> bool {
        std::env::var_os("SOKOMIND_DUAL_CACHE").is_some()
    }

    /// The rooms a knob names: its comma list, sorted and
    /// deduplicated — a repeated name prices its room once, never
    /// twice.
    pub(super) fn parse_rooms(text: &str) -> Option<Vec<usize>> {
        let rooms: Option<Vec<usize>> = text
            .split(',')
            .map(|token| token.trim().parse::<usize>().ok())
            .collect();
        let mut rooms = rooms?;
        rooms.sort_unstable();
        rooms.dedup();
        Some(rooms)
    }
}

#[cfg(target_arch = "wasm32")]
mod native {
    pub(super) fn portfolio_rooms() -> Option<Vec<usize>> {
        None
    }

    pub(super) fn dual_cache_armed() -> bool {
        false
    }
}

/// The portfolio's second bound configuration: a walk term over a
/// coarser counted mask — the union of the rooms the knob names — with
/// its own hall, so the term prices those rooms' cells jointly with
/// every move outside them while the named rooms' tables price the
/// cells exactly. Each configuration splits the moves its own way and
/// bounds each side, so both are admissible and their per-state max is
/// too; the mask being a union of counted cells by construction is
/// what keeps the second split total — no cell falls outside both its
/// sides.
struct PortfolioArm {
    /// The term over the named rooms' counted union.
    term: WalkTerm,
    /// The term's own hall, over the same mask.
    hall: Heuristic,
    /// The named rooms, whose tables' sum prices the mask's cells.
    rooms: Vec<usize>,
    /// The heap bytes the term and its hall hold.
    heap: u64,
}

impl PortfolioArm {
    /// Builds the second configuration when the knob names rooms of
    /// `rooms`: the union of their counted cells as the mask, the hall
    /// and the term over it. `None` when the knob is unset or
    /// malformed, when a name falls outside the built rooms (said
    /// loudly — the same trap), and below the capped work scale or
    /// without the dual cache armed: the second term's solves cost what
    /// the first's do, and only the cache keeps them off the search.
    fn build(board: &Board, start: &State, cap: usize, rooms: &[RoomTables]) -> Option<Self> {
        if cap < MAX_CAP as usize || !native::dual_cache_armed() {
            return None;
        }
        let names = native::portfolio_rooms()?;
        let mut mask = vec![false; board.neighbors().len()];
        for &index in &names {
            let Some(built) = rooms.get(index) else {
                eprintln!(
                    "room bound: SOKOMIND_PORTFOLIO_ROOMS names room {index} \
                     beyond the {} built; the portfolio arm is off",
                    rooms.len()
                );
                return None;
            };
            for table in &built.tables {
                for (q, &cell) in built.room.cells().iter().enumerate() {
                    if bit(table.part(), q) {
                        mask[usize::from(cell)] = true;
                    }
                }
            }
        }
        let hall = Heuristic::hall(board, start.player, &mask);
        let term = WalkTerm::build(board, &hall, &mask, start)?;
        let heap = term.bytes() + hall_bytes(board) as u64;
        Some(Self {
            term,
            hall,
            rooms: names,
            heap,
        })
    }

    /// The heap bytes the term and its hall hold.
    fn bytes(&self) -> u64 {
        self.heap
    }
}

/// The built room tables, the hall part and the walk term, which the
/// search keeps.
pub(crate) struct RoomBound {
    rooms: Vec<RoomTables>,
    hall: Heuristic,
    /// The dual-transfer walk term over the parts none of them count
    /// (see `walk_term`): `None` when its LP could not be solved, which
    /// leaves S no weaker than the hall's own arm.
    walk: Option<WalkTerm>,
    /// The portfolio's second configuration (see `PortfolioArm`):
    /// `None` unless its knob names rooms, which leaves S as it is.
    portfolio: Option<PortfolioArm>,
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

    /// The portfolio's arm of S at `state`: the named rooms' tables'
    /// sum plus the second term's dual value, ceilinged — the second
    /// configuration's own split of the same moves, maxed in wherever
    /// it is the stronger. `None` when a projection is missing or the
    /// knob named nothing.
    fn portfolio_arm(&self, state: &State) -> Option<u32> {
        let arm = self.portfolio.as_ref()?;
        let tables: Option<u32> = arm
            .rooms
            .iter()
            .map(|&index| {
                let built = &self.rooms[index];
                built
                    .tables
                    .iter()
                    .map(|table| table.value(&built.room, state))
                    .sum::<Option<u32>>()
            })
            .sum();
        Some((f64::from(tables?) + arm.term.value(&arm.hall, state) - 1e-6).ceil() as u32)
    }

    /// S of `state` from scratch, `None` when a projection is missing. The
    /// hall part is less the keeper's charge, which only the start can have:
    /// a push leaves the keeper where a box stood (see `Heuristic::estimate`).
    /// The walk term's arm — the tables' sum plus the term's dual value,
    /// ceilinged — joins the max when the term was built: it prices the
    /// off-counted pushes and walks jointly, where the hall prices the
    /// pushes alone. The portfolio's arm joins the same max when its knob
    /// named rooms.
    pub(crate) fn root(&self, state: &State) -> Option<u32> {
        let tables = self.tables_sum(state)?;
        let hall = self.hall.estimate(state)?;
        let mut s = tables + hall.saturating_sub(self.hall.charge(state.player));
        if let Some(walk) = &self.walk {
            s = s.max(walk.arm(&self.hall, state, tables));
        }
        if let Some(arm) = self.portfolio_arm(state) {
            s = s.max(arm);
        }
        Some(s)
    }

    /// S of `child`, the state box `i` of `parent` reaches when pushed in
    /// direction `d` onto `to`, `None` when a projection is missing. The
    /// hall part updates the parent's incrementally, through `cache`, which
    /// every child of one expansion shares; the walk term's arm joins the
    /// max from scratch, its O(boxes) row lookups per child.
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
        let mut s = tables + hall;
        if let Some(walk) = &self.walk {
            s = s.max(walk.arm(&self.hall, child, tables));
        }
        if let Some(arm) = self.portfolio_arm(child) {
            s = s.max(arm);
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Status, engine::Engine, engine::Policy};

    /// The keeper's floor with boxes ignored, as `RoomBuild::new` floods.
    fn inside(board: &Board, start: &State) -> Vec<bool> {
        let neighbors = board.neighbors();
        let mut inside = vec![false; neighbors.len()];
        let mut queue = vec![start.player];
        inside[usize::from(start.player)] = true;
        while let Some(cell) = queue.pop() {
            for &next in &neighbors[usize::from(cell)] {
                if next != NONE && !inside[usize::from(next)] {
                    inside[usize::from(next)] = true;
                    queue.push(next);
                }
            }
        }
        inside
    }

    /// A ledger whose limits no test buffer reaches.
    fn ledger() -> Ledger {
        Ledger {
            live: 0,
            limit: 1 << 22,
            kept: 0,
            kept_limit: 1 << 22,
        }
    }

    /// The portfolio knob's list: the rooms it names, sorted and
    /// deduplicated — a repeated name prices its room once; anything
    /// that is not a comma list of numbers is malformed and refused.
    #[test]
    fn the_portfolio_knob_parses_room_lists() {
        assert_eq!(native::parse_rooms("0,1"), Some(vec![0, 1]));
        assert_eq!(native::parse_rooms(" 2 , 0 , 2 "), Some(vec![0, 2]));
        assert_eq!(native::parse_rooms("3"), Some(vec![3]));
        assert_eq!(native::parse_rooms(""), None);
        assert_eq!(native::parse_rooms("0,x"), None);
        assert_eq!(native::parse_rooms("-1"), None);
    }

    #[test]
    fn only_overlaps_without_nesting_cross() {
        let list = |cells: &[usize]| cells.iter().map(|&c| c as Cell).collect::<Vec<_>>();
        let a = list(&[1, 2, 3]);
        assert!(crosses(&a, &list(&[2, 3, 4])));
        assert!(!crosses(&a, &list(&[1, 2, 3])));
        assert!(!crosses(&a, &list(&[4, 5, 6])));
        assert!(!crosses(&a, &list(&[1, 2])));
        assert!(!crosses(&a, &list(&[2, 3])));
    }

    /// A pocket behind a two-cell neck: no single cell of the neck splits
    /// the floor, so only the pair pass of `candidates` finds it. The
    /// pocket holds box A and its goal, the main area box B, its goal and
    /// the keeper.
    #[test]
    fn a_pocket_behind_two_doors_is_a_candidate() {
        let board = Board::parse("OOOOOOOO\nO   b  O\nOaA B  O\nOOO ROOO\nOOOOOOOO").unwrap();
        let start = board.initial();
        let mut ledger = ledger();
        let found = candidates(&board, &start, &inside(&board, &start), &mut ledger).unwrap();
        let list = |cells: &[usize]| cells.iter().map(|&c| c as Cell).collect::<Vec<_>>();
        let (pocket, neck) = (list(&[9, 10, 17, 18]), list(&[11, 19]));
        assert!(
            found
                .iter()
                .any(|(doors, cells)| *doors == neck && *cells == pocket),
            "among {} candidates",
            found.len()
        );
    }

    /// The pocket's table prices the push that ends in it and the step to
    /// its stand does not, and Exact proves the board's optimum, so the
    /// bound never passes it.
    #[test]
    fn the_pocket_bound_stays_below_the_optimum() {
        let board = Board::parse("OOOOOOOO\nO   b  O\nOaA B  O\nOOO ROOO\nOOOOOOOO").unwrap();
        let mut engine =
            Engine::new(board.clone(), board.initial(), Policy::EXACT, 1_000, 16).unwrap();
        while engine.status() == Status::Running {
            engine.advance(1 << 20);
        }
        assert_eq!(engine.status(), Status::Solved);
        assert_eq!(engine.best_moves(), Some(3));
    }

    /// A corridor two cells wide: a cut along one column splits nothing,
    /// but the pair pass's first flood stops big past `MAX_ROOM` cells
    /// and stamps them, so a later flood would finish a carved piece of
    /// the same component whose boundary the cut does not name. The piece
    /// is not a candidate; the corridor's across cuts still find its true
    /// pockets.
    #[test]
    fn a_carved_piece_of_an_unsplit_corridor_is_not_a_candidate() {
        let mut rows = vec!["OOOO".to_string()];
        for row in 1..=42 {
            let middle = match row {
                1 => "R ",
                2 => "S ",
                40 => "X ",
                _ => "  ",
            };
            rows.push(format!("O{}O", middle));
        }
        rows.push("OOOO".to_string());
        let board = Board::parse(&rows.join("\n")).unwrap();
        let start = board.initial();
        let mut ledger = ledger();
        let found = candidates(&board, &start, &inside(&board, &start), &mut ledger).unwrap();
        let neighbors = board.neighbors();
        for (doors, cells) in &found {
            for &cell in cells {
                for &next in &neighbors[usize::from(cell)] {
                    assert!(
                        next == NONE || cells.contains(&next) || doors.contains(&next),
                        "a candidate's boundary holds floor its doors do not name"
                    );
                }
            }
        }
        let box_cell = (40 * 4 + 1) as Cell;
        assert!(
            found.iter().any(|(_, cells)| cells.contains(&box_cell)),
            "the pocket behind an across cut holds the box, among {} candidates",
            found.len()
        );
    }

    /// A pocket behind a three-cell neck: no pair of the neck's cells
    /// splits the floor, so only the triple pass of `candidates` finds
    /// it. The pocket holds box A and its goal, the main area box B, its
    /// goal and the keeper.
    #[test]
    fn a_pocket_behind_three_doors_is_a_candidate() {
        let board =
            Board::parse("OOOOOOOO\nOa  B  O\nO A    O\nO   b  O\nOOOO  RO\nOOOOOOOO").unwrap();
        let start = board.initial();
        let mut ledger = ledger();
        let found = candidates(&board, &start, &inside(&board, &start), &mut ledger).unwrap();
        let list = |cells: &[usize]| cells.iter().map(|&c| c as Cell).collect::<Vec<_>>();
        let (pocket, neck) = (list(&[9, 10, 17, 18, 25, 26]), list(&[11, 19, 27]));
        assert!(
            found
                .iter()
                .any(|(doors, cells)| *doors == neck && *cells == pocket),
            "among {} candidates",
            found.len()
        );
    }

    /// A pocket behind a four-cell neck: no pair or triple of the neck's
    /// cells splits the floor, so only the wide pass's two adjacent pairs
    /// find it.
    #[test]
    fn a_pocket_behind_four_doors_is_a_candidate() {
        let board =
            Board::parse("OOOOOOOO\nOa  B  O\nO A    O\nO      O\nO   b  O\nOOOO  RO\nOOOOOOOO")
                .unwrap();
        let start = board.initial();
        let mut ledger = ledger();
        let found = candidates(&board, &start, &inside(&board, &start), &mut ledger).unwrap();
        let list = |cells: &[usize]| cells.iter().map(|&c| c as Cell).collect::<Vec<_>>();
        let (pocket, neck) = (
            list(&[9, 10, 17, 18, 25, 26, 33, 34]),
            list(&[11, 19, 27, 35]),
        );
        assert!(
            found
                .iter()
                .any(|(doors, cells)| *doors == neck && *cells == pocket),
            "among {} candidates",
            found.len()
        );
    }

    /// A pocket behind a five-cell neck: only the wide pass's two
    /// adjacent pairs with one pool cell more find it.
    #[test]
    fn a_pocket_behind_five_doors_is_a_candidate() {
        let board = Board::parse(
            "OOOOOOOO\nOa  B  O\nO A    O\nO      O\nO      O\nO   b  O\nOOOO  RO\nOOOOOOOO",
        )
        .unwrap();
        let start = board.initial();
        let mut ledger = ledger();
        let found = candidates(&board, &start, &inside(&board, &start), &mut ledger).unwrap();
        let list = |cells: &[usize]| cells.iter().map(|&c| c as Cell).collect::<Vec<_>>();
        let (pocket, neck) = (
            list(&[9, 10, 17, 18, 25, 26, 33, 34, 41, 42]),
            list(&[11, 19, 27, 35, 43]),
        );
        assert!(
            found
                .iter()
                .any(|(doors, cells)| *doors == neck && *cells == pocket),
            "among {} candidates",
            found.len()
        );
    }

    /// The wide work share is a floor: a spent rooms' share still keeps
    /// wide rooms on the reserved share alone, and the rooms' leftover
    /// flows to the wide phase, so a spent wide share still keeps them on
    /// that — here the four- and five-door rooms behind the neck either
    /// way, which the old single budget ended the build before reaching
    /// when the rooms spent it all.
    #[test]
    fn the_wide_work_share_is_the_wide_candidates_own() {
        let board =
            Board::parse("OOOOOOOO\nO      O\nOA     O\nOa  B  O\nO   b  O\nOOOO  RO\nOOOOOOOO")
                .unwrap();
        let start = board.initial();
        let heuristic = crate::heuristic::Heuristic::new(&board, Some(start.player));
        let drive = |mut build: RoomBuild| -> Option<RoomBound> {
            loop {
                let mut units = 1 << 22;
                if let Some(bound) = build.step(&board, &start, &heuristic, &mut units) {
                    return bound;
                }
            }
        };

        let mut build = RoomBuild::new(&board, &start, 1_000, 16, 0, 0).unwrap();
        build.work = 0;
        let bound = drive(build).expect("wide rooms build on the wide share");
        assert!(
            !bound.rooms.is_empty()
                && bound
                    .rooms
                    .iter()
                    .all(|built| built.room.doors().len() >= WIDE_DOORS),
            "a spent rooms' share still keeps wide rooms"
        );

        let mut build = RoomBuild::new(&board, &start, 1_000, 16, 0, 0).unwrap();
        build.wide_work = 0;
        let bound = drive(build).expect("the wide candidates build on the rooms' leftover");
        assert!(
            bound
                .rooms
                .iter()
                .any(|built| built.room.doors().len() >= WIDE_DOORS),
            "a spent wide share still keeps wide rooms on the rooms' leftover"
        );
    }

    /// The walk term joins S only at the capped work scale, and where it
    /// does it never passes the optimum: the pocket board driven at a
    /// million states builds a term with a positive value at the start,
    /// and its root bound stays at or below the 3 moves Exact proves.
    #[test]
    fn the_walk_term_joins_the_bound_at_the_capped_scale() {
        let board = Board::parse("OOOOOOOO\nO   b  O\nOaA B  O\nOOO ROOO\nOOOOOOOO").unwrap();
        let start = board.initial();
        let heuristic = crate::heuristic::Heuristic::new(&board, Some(start.player));
        let mut build = RoomBuild::new(&board, &start, 1 << 20, 16, 0, 0).unwrap();
        let bound = loop {
            let mut units = 1 << 22;
            if let Some(bound) = build.step(&board, &start, &heuristic, &mut units) {
                break bound.expect("the pocket board builds tables");
            }
        };
        let walk = bound
            .walk
            .as_ref()
            .expect("the term builds at the capped scale");
        assert!(
            walk.value(&bound.hall, &start) > 0.0,
            "the term prices the start's off-counted moves"
        );
        assert!(bound.root(&start).is_some_and(|s| s <= 3));
    }
}
