use crate::{
    MAX_STATES, MAX_STATES_RANGE, MEMORY_MIB_RANGE, SearchError, Status, corral::Corral,
    deadlock::Deadlock, heuristic::Heuristic, reach::Reach,
};
use sokomind_core::{Cell, MAX_BOXES, MAX_CELLS, MAX_ROUTE, NONE, State};
use std::{cmp::Reverse, collections::BinaryHeap, mem::size_of};

/// No node: an empty table slot, or the root's parent.
pub(crate) const NIL: u32 = u32::MAX;

/// Bits of a node id in a queue key, in a record's parent field and in a
/// table slot.
const ID_BITS: u32 = 26;
/// Low `ID_BITS` set: extracts an id, and stands for NIL in a record.
const ID_MASK: u32 = (1 << ID_BITS) - 1;
// Every id, the spare node's included, is at most MAX_STATES, so it fits
// both fields with the all-ones value left over for a record's NIL parent.
const _: () = assert!(MAX_STATES < ID_MASK as usize);

/// Largest stored g. A node's depth is at most its id, and one push adds at
/// most `MAX_CELLS` moves (a walk shorter than the board, then the push),
/// but `MAX_STATES * MAX_CELLS` overflows a u32 and no tighter bound holds
/// for every board, so [`Node::push_g`] saturates here instead. A stored
/// `G_SAT` stands for any g at or above it: its key saturates at
/// [`Key::F_SAT`], no game replays its route, and it bounds nothing from
/// above, so the exact search certifies no proof from it. It is one below
/// `u32::MAX`, which the WASM metrics send for "no route", so a saturated
/// best route never reads as none.
pub(crate) const G_SAT: u32 = u32::MAX - 1;
const _: () = assert!((MAX_ROUTE as u64) < Key::F_SAT && Key::F_SAT < G_SAT as u64);

/// Largest h a queue entry may carry: MAX_BOXES push distances, each below
/// `MAX_CELLS`, plus at most `MAX_CELLS` for the root's keeper walk to its
/// first push.
pub(crate) const MAX_QUEUED_H: u32 = ((MAX_BOXES + 1) * MAX_CELLS) as u32;

/// Budget bytes for the longest route and its letter string, `MAX_ROUTE`
/// bytes each, which `solution` allocates on demand. Part of
/// [`FIXED_SCRATCH`], and the most the stage ladder (stage.rs) may hold.
pub(crate) const ROUTE_ALLOWANCE: usize = 2 * MAX_ROUTE;

/// Budget bytes charged to every search whatever the board: room for the
/// longest route and its letter string, which `solution` allocates on
/// demand, `MAX_ROUTE` bytes each, plus a 64 KiB allowance for the
/// fixed-size scratch buffers. The stage ladder (stage.rs) reuses the route
/// allowance: it exists only while the search has no route and is freed
/// before one is recorded, and `Ladder::bytes_for(MAX_CELLS)` is asserted
/// to fit.
const FIXED_SCRATCH: usize = ROUTE_ALLOWANCE + 64 * 1024;

/// Log2 of the records in a full [`Chunk`]. Unit tests use four-record
/// chunks, so that small searches cross many chunk boundaries.
const CHUNK_SHIFT: u32 = if cfg!(test) { 2 } else { 16 };
/// Records in a full [`Chunk`].
const CHUNK: usize = 1 << CHUNK_SHIFT;
/// Table slots a new arena starts with, or fewer when its final table is
/// smaller: 2^16 slots, 256 KiB, so the one table the arena ever frees is
/// at most that. Unit tests start at 64, so that a search of a few dozen
/// states grows it after crossing several chunks.
const INITIAL_TABLE: usize = if cfg!(test) { 64 } else { 1 << 16 };
// A new arena's first insert, the start's, binds one slot of at least four
// and leaves len + 1 = 2, short of the four or more records of a full chunk,
// so it neither grows the table nor adds a chunk.
const _: () = assert!(CHUNK_SHIFT >= 2 && INITIAL_TABLE >= 4 && INITIAL_TABLE.is_power_of_two());

/// Slots in the final table of an arena of at most `records` records, the
/// size [`Arena::reserved_bytes`] charges, which that many ids never fill
/// past half.
const fn final_table(records: usize) -> usize {
    (records * 2).next_power_of_two()
}

#[derive(Clone, Copy)]
pub(crate) struct Node {
    pub state: State,
    pub g: u32,
    pub parent: u32,
    /// Direction of the push that made this node (unused at the root); the
    /// pushed box started at `state.player`.
    pub direction: u8,
    /// The state's estimate, or `u16::MAX` when it is unknown or too large
    /// and must be recomputed. Estimates depend only on the canonical
    /// state, so a cheaper duplicate reuses it. This stack value is decoded
    /// from compact storage only when an expansion or replay needs it.
    pub h: u16,
}

/// A stored node without its state: all a duplicate probe or an incumbent
/// check reads, at the cost of one record load.
#[derive(Clone, Copy)]
pub(crate) struct Meta {
    pub g: u32,
    /// As in [`Node::h`].
    pub h: u16,
    /// Expanded at least once ([`Arena::close`]).
    pub closed: bool,
}

impl Node {
    /// A child's g: the parent's `g`, a walk of `walk` moves, then the push,
    /// saturated at [`G_SAT`].
    pub(crate) fn push_g(g: u32, walk: u16) -> u32 {
        g.saturating_add(u32::from(walk) + 1).min(G_SAT)
    }
    /// The estimate as stored in [`Node::h`].
    pub(crate) fn store_h(h: u32) -> u16 {
        u16::try_from(h).unwrap_or(u16::MAX)
    }
    /// The stored estimate, if it fit.
    pub(crate) fn known_h(&self) -> Option<u32> {
        known(self.h)
    }
}
impl Meta {
    /// The stored estimate, if it fit.
    pub(crate) fn known_h(self) -> Option<u32> {
        known(self.h)
    }
}
fn known(h: u16) -> Option<u32> {
    (h < u16::MAX).then_some(u32::from(h))
}

/// Metadata is fixed-size; box cells live in a parallel column of their
/// [`Chunk`]. Parent indices always refer to immutable appended versions,
/// never overwritten states. Only the two flags ever change after the
/// append, except when [`Arena::clear_keeping`] moves a path down and
/// renumbers its parents.
#[derive(Clone, Copy)]
struct Record {
    g: u32,
    /// Parent id in bits 0..26 ([`ID_MASK`] at the root), bits 26..28
    /// spare and zero, the push direction in bits 28..30, then
    /// [`Record::CLOSED`] and [`Record::SUPERSEDED`].
    link: u32,
    player: Cell,
    h: u16,
}
// No padding and alignment 4 on wasm32 and 64-bit alike.
const _: () = assert!(size_of::<Record>() == 12 && ID_BITS <= Record::DIRECTION_SHIFT);
impl Record {
    const DIRECTION_SHIFT: u32 = 28;
    /// Expanded at least once.
    const CLOSED: u32 = 1 << 30;
    /// No table slot names this record: a later version of the state
    /// replaced it, or [`Arena::clear_keeping`] detached it. Any queued
    /// entry for it is stale.
    const SUPERSEDED: u32 = 1 << 31;
    /// A fresh, open version of `node`.
    fn new(node: &Node) -> Self {
        debug_assert!(node.direction < 4);
        debug_assert!(node.parent == NIL || node.parent < ID_MASK);
        let parent = if node.parent == NIL {
            ID_MASK
        } else {
            node.parent
        };
        Self {
            g: node.g,
            link: parent | (u32::from(node.direction) << Self::DIRECTION_SHIFT),
            player: node.state.player,
            h: node.h,
        }
    }
    fn parent(self) -> u32 {
        let parent = self.link & ID_MASK;
        if parent == ID_MASK { NIL } else { parent }
    }
    fn direction(self) -> u8 {
        ((self.link >> Self::DIRECTION_SHIFT) & 3) as u8
    }
    fn has(self, flag: u32) -> bool {
        self.link & flag != 0
    }
}

/// Queue entry `(f, h, id)` packed into one integer, f in the top 20 bits,
/// h in the next 18 and the id in the low `ID_BITS` (26), so integer order
/// is exactly tuple order: lowest f first, then lowest h, then oldest.
/// Values past a field saturate; see [`Key::F_SAT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key(u64);
impl Key {
    const H_BITS: u32 = 18;
    const F_SHIFT: u32 = Self::H_BITS + ID_BITS;
    /// Largest stored f. A key reading `F_SAT` stands for any f at or above
    /// it: never more than the true f, so a frontier read from keys stays a
    /// lower bound, and only nodes past MAX_ROUTE can saturate (see the
    /// policy asserts in engine.rs).
    pub(crate) const F_SAT: u64 = u64::MAX >> Self::F_SHIFT;
    /// Largest stored h.
    const H_SAT: u32 = (1 << Self::H_BITS) - 1;
    pub(crate) fn new(f: u64, h: u32, id: u32) -> Self {
        debug_assert!(id < ID_MASK);
        let h = u64::from(h.min(Self::H_SAT));
        Self((f.min(Self::F_SAT) << Self::F_SHIFT) | (h << ID_BITS) | u64::from(id))
    }
    /// The queued f, exact below [`Key::F_SAT`].
    pub(crate) fn f(self) -> u64 {
        self.0 >> Self::F_SHIFT
    }
    /// The queued h, exact up to [`MAX_QUEUED_H`].
    pub(crate) fn h(self) -> u32 {
        ((self.0 >> ID_BITS) as u32) & Self::H_SAT
    }
    pub(crate) fn id(self) -> u32 {
        (self.0 as u32) & ID_MASK
    }
}
const _: () = assert!(MAX_QUEUED_H <= Key::H_SAT);

/// 8 bytes on wasm32 and 64-bit alike.
type Entry = Reverse<Key>;
const _: () = assert!(size_of::<Entry>() == 8);

fn hash(player: Cell, boxes: &[Cell]) -> usize {
    let mut h = (player as u64).wrapping_add(0x9e3779b97f4a7c15);
    for &cell in boxes {
        h ^= cell as u64;
        h = h.wrapping_mul(0x100000001b3);
        h ^= h >> 29;
    }
    (h ^ (h >> 32)) as usize
}

/// What the arena counts as it inserts and queues. Each field is the
/// [`crate::SearchStats`] counter of the same name; the engine counts the
/// rest. [`Arena::clear`] and [`Arena::clear_keeping`] keep them, so a
/// restarted search counts both arenas, and a kept record only once.
#[derive(Default)]
pub(crate) struct TableCounters {
    pub unique_states: u32,
    pub duplicate_improvements: u32,
    pub reopened_states: u32,
    pub peak_queue: u32,
}

/// Consecutive records and their box cells, reserved exactly when the arena
/// first needs them and never reallocated, so an appended record never
/// moves; only [`Arena::clear_keeping`] copies a path down. Id `id` lives in
/// chunk `id >> CHUNK_SHIFT` at offset `id & (CHUNK - 1)`: a shift and a
/// mask, since every chunk but the last holds exactly [`CHUNK`] records.
struct Chunk {
    records: Vec<Record>,
    /// The boxes of each record in order, `boxes` cells apiece.
    box_cells: Vec<Cell>,
}
impl Chunk {
    /// An empty chunk with room for `records` records of `boxes` boxes, or
    /// the name of the buffer the allocator refused.
    fn new(records: usize, boxes: usize) -> Result<Self, &'static str> {
        let mut chunk = Self {
            records: Vec::new(),
            box_cells: Vec::new(),
        };
        chunk
            .records
            .try_reserve_exact(records)
            .map_err(|_| "node arena")?;
        chunk
            .box_cells
            .try_reserve_exact(records * boxes)
            .map_err(|_| "box arena")?;
        Ok(chunk)
    }
    /// The box cells of the record at `offset`.
    fn cells(&self, offset: usize, boxes: usize) -> &[Cell] {
        &self.box_cells[offset * boxes..(offset + 1) * boxes]
    }
}

/// Node arena, priority queue, and open-addressed index table, all within
/// the budget that sized them ([`Arena::reserved_bytes`]). Only the queue
/// is reserved whole up front. Records grow a [`Chunk`] at a time, and the
/// table has two sizes and grows once, from the first to the final, each
/// allocated one insert ahead of need. The spare's insert therefore never
/// allocates, and a search whose table binds at most `INITIAL_TABLE / 2`
/// states allocates only the queue, its chunks and that first table. The
/// table stores arena indices, never duplicated box arrays.
pub(crate) struct Arena {
    /// Room for every record so far and, while there are at most
    /// `node_limit`, one more, unless a growth allocation was refused; never
    /// more than `node_limit + 1` records in all. Both clears keep every
    /// chunk, so afterwards the room can far exceed the records.
    chunks: Vec<Chunk>,
    /// Records appended since the last clear: the next node's id.
    len: usize,
    counters: TableCounters,
    heap: BinaryHeap<Entry>,
    /// Open-addressed slots, each a node id or [`NIL`] when empty. Ids stay
    /// below [`ID_MASK`], so a slot's top 32 - `ID_BITS` = 6 bits are spare:
    /// too few for a 12-bit hash tag, which is therefore not implemented. A
    /// 6-bit tag is worth trying only if profiling shows table finds dominate.
    ///
    /// A power of two of one of two sizes. It starts at [`INITIAL_TABLE`]
    /// slots, or at its final size when that is smaller, and once more than
    /// half of it is bound it grows straight to the final size,
    /// [`final_table`] of `node_limit + 1`, which it then keeps. Empty only
    /// after a failed growth.
    table: Vec<u32>,
    /// Ids the table binds: the records not superseded.
    resident: usize,
    /// Boxes per state, the prefix of `State::boxes` that is hashed.
    boxes: usize,
    node_limit: usize,
    reserved_bytes: usize,
    scaled_down: bool,
    /// A growth allocation failed below the limit; see [`Arena::starved`].
    starved: bool,
    /// Unit tests only: every growth allocation fails, as if the allocator
    /// refused it, so the starved path runs without exhausting memory.
    #[cfg(test)]
    refusing: bool,
    /// Bumped by every insert and clear, from 1. Debug builds only, like
    /// `found`.
    #[cfg(debug_assertions)]
    generation: u32,
    /// The generation of the last [`Arena::find`], 0 before any, which
    /// [`Arena::insert`] checks is current: no slot outlives an insert. An
    /// atomic, not a `Cell`, so a debug arena is `Sync` like a release one.
    #[cfg(debug_assertions)]
    found: std::sync::atomic::AtomicU32,
}
impl Arena {
    pub(crate) fn new(
        cells: usize,
        boxes: usize,
        max_states: usize,
        memory_mib: usize,
    ) -> Result<Self, SearchError> {
        if !MAX_STATES_RANGE.contains(&max_states) || !MEMORY_MIB_RANGE.contains(&memory_mib) {
            return Err(SearchError::Limits);
        }
        // Flood, deadlock and corral buffers and the heuristic tables per
        // cell, plus the route, its string and the fixed scratch. Bytes are
        // counted in u64, since usize is 32 bits on wasm32, and saturate, so
        // a board too large to count never fits instead of wrapping.
        let per_cell = Reach::BYTES_PER_CELL
            + Deadlock::BYTES_PER_CELL
            + Corral::BYTES_PER_CELL
            + Heuristic::bytes_per_cell(boxes);
        let fixed_bytes = (cells as u64)
            .saturating_mul(per_cell as u64)
            .saturating_add(FIXED_SCRATCH as u64);
        let budget = memory_mib as u64 * 1024 * 1024;
        // A record, its box cells and a queue entry, since the queue holds at
        // most one entry per record.
        let per_state =
            (size_of::<Record>() + boxes * size_of::<Cell>() + size_of::<Entry>()) as u64;
        // One spare node keeps a solution found at the exact limit reachable.
        // count is at most MAX_STATES, so the table's final size fits a
        // usize on wasm32 and only the sum with the fixed bytes can
        // overflow. These are the full-size buffers, the most the arena ever
        // grows to.
        let bytes_for = |count: usize| {
            let records = count + 1;
            fixed_bytes.saturating_add(
                records as u64 * per_state + final_table(records) as u64 * size_of::<u32>() as u64,
            )
        };
        // Exact largest limit that fits the budget, instead of stepping down 10%.
        let mut low = 0;
        let mut high = max_states;
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            if bytes_for(mid) <= budget {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        if low == 0 {
            return Err(SearchError::BudgetTooSmall);
        }
        let limit = low;
        // At most the budget, so it and every count allocated below or by
        // later growth fit a usize on wasm32 too.
        let reserved_bytes = bytes_for(limit) as usize;
        let records = limit + 1;
        // The chunk list, one entry per CHUNK records, is the one buffer the
        // budget does not charge: no budget holds more than 2^23 records, so
        // it is at most 128 entries, 6 KiB, though the four-record chunks of
        // unit tests make it far longer.
        let mut chunks = Vec::new();
        chunks
            .try_reserve_exact(records.div_ceil(CHUNK))
            .map_err(|_| SearchError::Allocation("node arena"))?;
        chunks.push(Chunk::new(records.min(CHUNK), boxes).map_err(SearchError::Allocation)?);
        // Every enqueue follows an insert, so heap.len() <= len <= limit + 1
        // and this never grows. It is reserved whole because a doubling
        // queue would briefly hold its old and new buffers at once, past the
        // budget near the limit.
        let mut heap = BinaryHeap::new();
        heap.try_reserve_exact(records)
            .map_err(|_| SearchError::Allocation("search queue"))?;
        let table_size = final_table(records).min(INITIAL_TABLE);
        let mut table = Vec::new();
        table
            .try_reserve_exact(table_size)
            .map_err(|_| SearchError::Allocation("state table"))?;
        table.resize(table_size, NIL);
        Ok(Self {
            boxes,
            reserved_bytes,
            scaled_down: limit < max_states,
            node_limit: limit,
            chunks,
            len: 0,
            counters: TableCounters::default(),
            heap,
            table,
            resident: 0,
            starved: false,
            #[cfg(test)]
            refusing: false,
            #[cfg(debug_assertions)]
            generation: 1,
            #[cfg(debug_assertions)]
            found: std::sync::atomic::AtomicU32::new(0),
        })
    }
    /// The status of a search the full arena stops: [`Status::MemoryLimit`]
    /// when the budget scaled the limit down or a growth allocation failed
    /// below it, [`Status::StateLimit`] otherwise.
    pub(crate) fn limit_status(&self) -> Status {
        if self.scaled_down || self.starved {
            Status::MemoryLimit
        } else {
            Status::StateLimit
        }
    }
    /// Bytes the budget charges: the fixed buffers and the arena at its full
    /// limit. The arena grows toward this but never past it, so it is a
    /// ceiling on the search's buffers, the chunk list's few KiB aside, not
    /// the amount allocated.
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    /// Whether only the spare insert may follow: the arena holds its limit,
    /// or a growth allocation failed ([`Arena::starved`]).
    pub(crate) fn is_full(&self) -> bool {
        self.len >= self.node_limit || self.starved
    }
    /// Whether a growth allocation failed below the limit. The arena is
    /// then full, and the table may be gone, so the caller must stop before
    /// its next [`Arena::find`]. Records and queue are intact, so every
    /// bound and route read from them still holds.
    pub(crate) fn starved(&self) -> bool {
        self.starved
    }
    /// The chunk holding `id`, and `id`'s offset in it.
    #[inline]
    fn chunk(&self, id: u32) -> (&Chunk, usize) {
        let id = id as usize;
        (&self.chunks[id >> CHUNK_SHIFT], id & (CHUNK - 1))
    }
    #[inline]
    fn record(&self, id: u32) -> Record {
        let (chunk, offset) = self.chunk(id);
        chunk.records[offset]
    }
    #[inline]
    fn record_mut(&mut self, id: u32) -> &mut Record {
        let id = id as usize;
        &mut self.chunks[id >> CHUNK_SHIFT].records[id & (CHUNK - 1)]
    }
    /// The state's node, if any, and the table slot that holds it or would
    /// hold a new node for it. The slot is valid until the next insert.
    pub(crate) fn find(&self, state: &State) -> (usize, Option<u32>) {
        #[cfg(debug_assertions)]
        self.found
            .store(self.generation, std::sync::atomic::Ordering::Relaxed);
        let boxes = &state.boxes[..self.boxes];
        let mask = self.table.len() - 1;
        let mut slot = hash(state.player, boxes) & mask;
        loop {
            let id = self.table[slot];
            if id == NIL {
                return (slot, None);
            }
            let (chunk, offset) = self.chunk(id);
            if chunk.records[offset].player == state.player
                && chunk.cells(offset, self.boxes) == boxes
            {
                return (slot, Some(id));
            }
            slot = (slot + 1) & mask;
        }
    }
    /// Appends `node` and binds `slot`, which [`Arena::find`] returned for its
    /// state with no insert since (debug builds check), replacing any older
    /// node there, which becomes superseded. Only one node may go past the
    /// limit, into the spare slot: a solution discovered at the exact moment
    /// the arena filled. Below the limit it then allocates what the next
    /// insert needs ([`Arena::grow`]), so the spare's insert never does.
    pub(crate) fn insert(&mut self, node: Node, slot: usize) -> u32 {
        debug_assert!(self.len <= self.node_limit);
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                *self.found.get_mut(),
                self.generation,
                "a slot outlived an insert or a clear"
            );
            self.generation = self.generation.wrapping_add(1);
        }
        let id = self.len as u32;
        let previous = self.table[slot];
        if previous == NIL {
            self.counters.unique_states += 1;
            self.resident += 1;
        } else {
            self.counters.duplicate_improvements += 1;
            let previous = self.record_mut(previous);
            let reopened = previous.has(Record::CLOSED);
            // Its queued entry, if any, now pops as stale.
            previous.link |= Record::SUPERSEDED;
            if reopened {
                self.counters.reopened_states += 1;
            }
        }
        let chunk = &mut self.chunks[self.len >> CHUNK_SHIFT];
        debug_assert!(chunk.records.len() < chunk.records.capacity());
        chunk
            .box_cells
            .extend_from_slice(&node.state.boxes[..self.boxes]);
        chunk.records.push(Record::new(&node));
        self.table[slot] = id;
        self.len += 1;
        // Nearly every insert stops at these checks, so the growth itself
        // stays out of line.
        if self.len <= self.node_limit
            && !self.starved
            && (self.resident * 2 > self.table.len()
                || self.len + 1 == (self.chunks.len() << CHUNK_SHIFT))
        {
            self.grow();
        }
        id
    }
    /// Allocates ahead what the next insert needs: the final table once more
    /// than half of the first is bound, and the next chunk once the last has
    /// one record left, never more than the `node_limit + 1` records in all.
    /// Growth therefore stops at exactly the buffers
    /// [`Arena::reserved_bytes`] charges, and a full arena still has room
    /// for the spare without allocating. A refused allocation sets
    /// [`Arena::starved`] and grows nothing more.
    #[cold]
    #[inline(never)]
    fn grow(&mut self) {
        if self.resident * 2 > self.table.len() {
            // The table's one growth, straight to its final size: the
            // node_limit + 1 ids at most never bind more than half of that,
            // so this branch never runs again.
            let size = final_table(self.node_limit + 1);
            debug_assert!(self.table.len() < size);
            // Free the first table before the final one is allocated, so the
            // two never coexist; the records rebuild it. On wasm32 linear
            // memory never shrinks, so this freed table, at most
            // INITIAL_TABLE slots (256 KiB), can stay allocated past the
            // budget.
            self.table = Vec::new();
            if self.refusing() || self.table.try_reserve_exact(size).is_err() {
                self.starved = true;
                return;
            }
            self.table.resize(size, NIL);
            self.rebind();
        }
        let reserved = self.chunks.len() << CHUNK_SHIFT;
        if self.len + 1 == reserved && reserved <= self.node_limit {
            match Chunk::new((self.node_limit + 1 - reserved).min(CHUNK), self.boxes) {
                Ok(chunk) if !self.refusing() => self.chunks.push(chunk),
                _ => self.starved = true,
            }
        }
    }
    /// Whether growth allocations fail without asking the allocator: never,
    /// outside unit tests.
    #[cfg(not(test))]
    fn refusing(&self) -> bool {
        false
    }
    /// Whether a unit test made growth allocations fail; see
    /// `Arena::refuse_growth`.
    #[cfg(test)]
    fn refusing(&self) -> bool {
        self.refusing
    }
    /// Binds every record not superseded in an empty table, in id order.
    fn rebind(&mut self) {
        let mask = self.table.len() - 1;
        let mut bound = 0;
        for (k, chunk) in self.chunks.iter().enumerate() {
            let first = k << CHUNK_SHIFT;
            for (offset, record) in chunk.records.iter().enumerate() {
                if record.has(Record::SUPERSEDED) {
                    continue;
                }
                let mut slot = hash(record.player, chunk.cells(offset, self.boxes)) & mask;
                while self.table[slot] != NIL {
                    slot = (slot + 1) & mask;
                }
                self.table[slot] = (first + offset) as u32;
                bound += 1;
            }
        }
        debug_assert_eq!(bound, self.resident);
    }
    pub(crate) fn node(&self, id: u32) -> Node {
        let (chunk, offset) = self.chunk(id);
        let record = chunk.records[offset];
        let mut state = State {
            player: record.player,
            boxes: [NONE; MAX_BOXES],
        };
        state.boxes[..self.boxes].copy_from_slice(chunk.cells(offset, self.boxes));
        Node {
            state,
            g: record.g,
            parent: record.parent(),
            direction: record.direction(),
            h: record.h,
        }
    }
    /// The node's g, stored h and closed flag, without decoding its state.
    pub(crate) fn meta(&self, id: u32) -> Meta {
        let record = self.record(id);
        Meta {
            g: record.g,
            h: record.h,
            closed: record.has(Record::CLOSED),
        }
    }
    /// Whether a later version of the node's state has replaced it, or
    /// [`Arena::clear_keeping`] detached it: exactly when [`Arena::find`] on
    /// its state would not return this id.
    pub(crate) fn is_superseded(&self, id: u32) -> bool {
        self.record(id).has(Record::SUPERSEDED)
    }
    pub(crate) fn counters(&self) -> &TableCounters {
        &self.counters
    }
    /// Marks the node expanded.
    pub(crate) fn close(&mut self, id: u32) {
        self.record_mut(id).link |= Record::CLOSED;
    }
    /// Queues `id` at `f`, with `h` as the tie-break; see [`Key`].
    ///
    /// Every queued key must read `min(g + weight * h, F_SAT)` for the
    /// node's stored g at the current weight, with `h <= MAX_QUEUED_H`;
    /// [`Arena::reweight`] asserts it.
    pub(crate) fn enqueue(&mut self, f: u64, h: u32, id: u32) {
        debug_assert!(h <= MAX_QUEUED_H);
        self.heap.push(Reverse(Key::new(f, h, id)));
        self.counters.peak_queue = self.counters.peak_queue.max(self.heap.len() as u32);
    }
    /// The lowest queued entry, which may be stale: its node, the h it was
    /// queued with, and its f.
    pub(crate) fn dequeue(&mut self) -> Option<Key> {
        self.heap.pop().map(|Reverse(key)| key)
    }
    /// Lowest queued f, stale entries included.
    pub(crate) fn min_f(&self) -> Option<u64> {
        self.heap.peek().map(|Reverse(key)| key.f())
    }
    /// Forgets every node and queued entry in place. Every chunk, the queue
    /// and the table keep their allocations, so refilling to the size
    /// already reached allocates nothing; the limit and counters are kept,
    /// and so is a starved arena's flag when its failed growth left no
    /// table to search.
    pub(crate) fn clear(&mut self) {
        for chunk in &mut self.chunks {
            chunk.records.clear();
            chunk.box_cells.clear();
        }
        self.len = 0;
        self.resident = 0;
        self.heap.clear();
        self.table.fill(NIL);
        self.starved = self.table.is_empty();
        #[cfg(debug_assertions)]
        {
            self.generation = self.generation.wrapping_add(1);
        }
    }
    /// Empties the arena like [`Arena::clear`], except that the path from
    /// the root to `id` stays at the front, root first, as detached records:
    /// each keeps its g, h, keeper, push direction, flags and box cells, with
    /// its parent renumbered, and is marked superseded. No table slot binds
    /// one and no queue entry names one, so [`Arena::find`] never returns
    /// it and nothing pops it; the path only carries a route to replay and
    /// its length as a bound. No counter changes, since each record was
    /// counted when it was inserted. Returns `id`'s new id, one below the
    /// path's length, so the next insert gets the path's length. Returns
    /// `None`, with nothing changed, when the path would leave no room below
    /// the limit for one more record, the start a fresh search needs, or is
    /// longer than the table that serves as scratch.
    ///
    /// Afterwards the kept path's records are detached from the table, so a
    /// caller may insert a live copy of a kept state: [`Arena::find`] on it
    /// returns no previous record. The stage ladder's reseed (stage/ladder.rs)
    /// relies on that. Costs `O(table slots)`, for the table's refill with
    /// [`NIL`], plus the path's length.
    pub(crate) fn clear_keeping(&mut self, id: u32) -> Option<u32> {
        // Count the path first, so a refusal changes nothing. A parent is
        // always older than its child, since records are only ever appended.
        let mut len = 1;
        let mut link = self.record(id).parent();
        while link != NIL {
            len += 1;
            link = self.record(link).parent();
        }
        // The table, refilled below, is the scratch for the path's old ids,
        // root first, so nothing is allocated. It never shrinks, and the
        // path fits it unless a failed growth freed it: a path in this
        // search visits distinct states, every one bound in the table now,
        // and a path an earlier restart kept fit the table then.
        if len >= self.node_limit || len > self.table.len() {
            return None;
        }
        let mut old = id;
        for new in (0..len).rev() {
            self.table[new] = old;
            old = self.record(old).parent();
        }
        // Root first: the record at old id `old` moves down to `new` <= `old`,
        // possibly into an earlier chunk, over a record already moved or not
        // kept, never one still to move.
        let boxes = self.boxes;
        let mut cells = [NONE; MAX_BOXES];
        for new in 0..len {
            let (chunk, offset) = self.chunk(self.table[new]);
            let parent = if new == 0 { ID_MASK } else { new as u32 - 1 };
            let mut record = chunk.records[offset];
            record.link = (record.link & !ID_MASK) | parent | Record::SUPERSEDED;
            cells[..boxes].copy_from_slice(chunk.cells(offset, boxes));
            let (chunk, offset) = (&mut self.chunks[new >> CHUNK_SHIFT], new & (CHUNK - 1));
            chunk.records[offset] = record;
            chunk.box_cells[offset * boxes..(offset + 1) * boxes].copy_from_slice(&cells[..boxes]);
        }
        for (k, chunk) in self.chunks.iter_mut().enumerate() {
            let kept = len.saturating_sub(k << CHUNK_SHIFT).min(CHUNK);
            chunk.records.truncate(kept);
            chunk.box_cells.truncate(kept * boxes);
        }
        self.len = len;
        self.resident = 0;
        self.heap.clear();
        self.table.fill(NIL);
        // The table is whole, so even a starved arena may refill.
        self.starved = false;
        #[cfg(debug_assertions)]
        {
            self.generation = self.generation.wrapping_add(1);
        }
        Some(len as u32 - 1)
    }
    /// Re-keys every entry queued at `g + from * h` as `g + to * h`, in the
    /// reserved allocation, so the order is as if each entry had been queued
    /// at `to`. g comes from the entry's record, exact up to [`G_SAT`],
    /// which is past [`Key::F_SAT`], so an entry whose old key saturated
    /// re-keys correctly and a `G_SAT` g saturates the new key too.
    pub(crate) fn reweight(&mut self, from: u32, to: u32) {
        let mut entries = std::mem::take(&mut self.heap).into_vec();
        for Reverse(key) in &mut entries {
            let (g, h) = (u64::from(self.record(key.id()).g), key.h());
            debug_assert_eq!(
                key.f(),
                (g + u64::from(from) * u64::from(h)).min(Key::F_SAT)
            );
            *key = Key::new(g + u64::from(to) * u64::from(h), h, key.id());
        }
        self.heap = BinaryHeap::from(entries);
    }
}

#[cfg(test)]
impl Arena {
    /// Makes every later growth allocation fail, as an allocator out of
    /// memory would refuse it, or lets them reach the allocator again.
    pub(crate) fn refuse_growth(&mut self, refuse: bool) {
        self.refusing = refuse;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::Lcg;
    use std::collections::HashMap;

    #[test]
    fn compact_versions_preserve_parents_and_exact_keeper_identity() {
        for count in [1, 8, MAX_BOXES] {
            let mut arena = Arena::new(100, count, 10, 4).unwrap();
            let mut state = State {
                player: 1,
                boxes: [NONE; MAX_BOXES],
            };
            for (i, cell) in state.boxes[..count].iter_mut().enumerate() {
                *cell = i as Cell + 10;
            }
            let (slot, _) = arena.find(&state);
            let old = arena.insert(
                Node {
                    state,
                    g: 10,
                    parent: NIL,
                    direction: 0,
                    h: 3,
                },
                slot,
            );
            arena.close(old);
            let mut child = state;
            child.player = 2;
            let (slot, found) = arena.find(&child);
            assert_eq!(found, None);
            let descendant = arena.insert(
                Node {
                    state: child,
                    g: 11,
                    parent: old,
                    direction: 1,
                    h: 2,
                },
                slot,
            );
            let (slot, _) = arena.find(&state);
            let improved = arena.insert(
                Node {
                    state,
                    g: 8,
                    parent: NIL,
                    direction: 0,
                    h: 3,
                },
                slot,
            );
            assert_eq!(arena.find(&state).1, Some(improved));
            assert_eq!(arena.find(&child).1, Some(descendant));
            assert_eq!(arena.node(descendant).parent, old);
            assert_eq!(arena.node(descendant).direction, 1);
            assert_eq!(arena.node(improved).parent, NIL);
            assert_eq!(arena.node(old).g, 10);
            assert_eq!(arena.node(old).state, state);
            assert_eq!(arena.node(improved).g, 8);
            // Only the replaced version is stale, and closing is per version.
            let flags = |id| (arena.is_superseded(id), arena.meta(id).closed);
            assert_eq!(flags(old), (true, true));
            assert_eq!(flags(improved), (false, false));
            assert_eq!(flags(descendant), (false, false));
            assert_eq!(arena.counters.unique_states, 2);
            assert_eq!(arena.counters.duplicate_improvements, 1);
            assert_eq!(arena.counters.reopened_states, 1);
        }
    }

    #[test]
    fn a_budget_under_the_fixed_buffers_is_its_own_error() {
        // The per-cell buffers of a million cells alone overflow 4 MiB.
        assert_eq!(
            Arena::new(1 << 20, 1, 10, 4).err(),
            Some(SearchError::BudgetTooSmall)
        );
        assert_eq!(Arena::new(100, 1, 0, 4).err(), Some(SearchError::Limits));
        // A cell count too large to count in bytes saturates instead of
        // wrapping to a small total, on wasm32 and 64-bit alike.
        for memory_mib in [*MEMORY_MIB_RANGE.start(), *MEMORY_MIB_RANGE.end()] {
            assert_eq!(
                Arena::new(usize::MAX, MAX_BOXES, MAX_STATES, memory_mib).err(),
                Some(SearchError::BudgetTooSmall)
            );
        }
    }

    #[test]
    fn reweight_orders_the_queue_as_if_queued_at_the_new_weight() {
        let mut arena = Arena::new(100, 1, 10, 4).unwrap();
        // (g, h) per id: the order at weight 5 differs from the order at 3.
        // Re-keying reads each g from its record, so the nodes are real.
        let queued: [(u32, u32); 6] = [(0, 4), (9, 1), (4, 3), (12, 0), (7, 2), (1, 4)];
        for (i, &(g, h)) in queued.iter().enumerate() {
            let mut state = State {
                player: 1,
                boxes: [NONE; MAX_BOXES],
            };
            state.boxes[0] = 10 + i as Cell;
            let (slot, _) = arena.find(&state);
            let node = Node {
                state,
                g,
                parent: NIL,
                direction: 0,
                h: Node::store_h(h),
            };
            let id = arena.insert(node, slot);
            arena.enqueue(u64::from(g + 5 * h), h, id);
        }
        let capacity = arena.heap.capacity();
        arena.reweight(5, 3);
        assert_eq!(arena.heap.capacity(), capacity);
        let mut expected: Vec<_> = queued
            .iter()
            .enumerate()
            .map(|(id, &(g, h))| (u64::from(g + 3 * h), h, id as u32))
            .collect();
        expected.sort();
        assert_eq!(arena.min_f(), Some(expected[0].0));
        let order: Vec<_> = std::iter::from_fn(|| arena.dequeue())
            .map(|key| (key.id(), key.h()))
            .collect();
        let wanted: Vec<_> = expected.iter().map(|&(_, h, id)| (id, h)).collect();
        assert_eq!(order, wanted);
        assert_eq!(arena.counters.peak_queue, queued.len() as u32);
    }

    /// Clearing keeps every allocation the arena has grown, the queue's
    /// included, so refilling to the size it reached grows nothing.
    #[test]
    fn clear_forgets_every_state_and_keeps_the_capacity() {
        let mut arena = Arena::new(100, 1, 40, 4).unwrap();
        let at = |cell: usize| {
            let mut state = State {
                player: 1,
                boxes: [NONE; MAX_BOXES],
            };
            state.boxes[0] = 10 + cell as Cell;
            Node {
                state,
                g: 0,
                parent: NIL,
                direction: 0,
                h: 1,
            }
        };
        let fill = |arena: &mut Arena, cells: std::ops::Range<usize>| {
            for cell in cells {
                let node = at(cell);
                let (slot, found) = arena.find(&node.state);
                assert_eq!(found, None);
                let id = arena.insert(node, slot);
                arena.enqueue(1, 1, id);
            }
        };
        let capacity = |arena: &Arena| {
            let chunks: Vec<_> = arena
                .chunks
                .iter()
                .map(|chunk| (chunk.records.capacity(), chunk.box_cells.capacity()))
                .collect();
            (
                chunks,
                arena.heap.capacity(),
                arena.table.len(),
                arena.reserved_bytes(),
            )
        };
        // Past several chunks and the table's growth, at the 33rd state.
        fill(&mut arena, 0..36);
        let before = capacity(&arena);
        assert!(before.0.len() > 2 && before.2 == final_table(arena.node_limit + 1));
        arena.clear();
        assert_eq!(capacity(&arena), before);
        assert_eq!((arena.len(), arena.dequeue()), (0, None));
        // Ids restart at 0; the counters keep counting.
        let (slot, found) = arena.find(&at(0).state);
        assert_eq!(found, None);
        assert_eq!(arena.insert(at(0), slot), 0);
        assert_eq!(arena.counters.unique_states, 37);
        fill(&mut arena, 1..36);
        assert_eq!(capacity(&arena), before);
    }

    /// Records grow a chunk at a time, one ahead of need, and the table
    /// grows once, straight to its final size, when more than half of it is
    /// bound, and neither changes an id, a lookup or the queue: after every
    /// insert each id resolves to its node, each state finds its current
    /// id, and the queue's minimum matches a plain list of keys. The growth
    /// rebinds records from several chunks and skips superseded ones. The
    /// full arena then takes the spare without growing, and a path kept
    /// from it moves down across chunks.
    #[test]
    fn growth_keeps_ids_and_duplicates() {
        const LIMIT: usize = 150;
        const FINAL: usize = final_table(LIMIT + 1);
        type Current = HashMap<(Cell, [Cell; MAX_BOXES]), u32>;
        let mut arena = Arena::new(100, 2, LIMIT, 4).unwrap();
        let mut rng = Lcg(0xa4e7a);
        let key = |state: &State| (state.player, state.boxes);
        // Every node inserted, by id, each state's current id, and every key
        // queued and not yet dequeued.
        let mut nodes: Vec<Node> = Vec::new();
        let mut current = Current::new();
        let mut queued: Vec<Key> = Vec::new();
        let mut table = INITIAL_TABLE;
        let check = |arena: &Arena, nodes: &[Node], current: &Current| {
            for (id, node) in nodes.iter().enumerate() {
                let id = id as u32;
                let stored = arena.node(id);
                assert_eq!(stored.state, node.state);
                assert_eq!(
                    (stored.g, stored.parent, stored.direction, stored.h),
                    (node.g, node.parent, node.direction, node.h)
                );
                assert_eq!(arena.is_superseded(id), current[&key(&node.state)] != id);
            }
            for &id in current.values() {
                assert_eq!(arena.find(&nodes[id as usize].state).1, Some(id));
            }
        };
        while !arena.is_full() {
            // Four keepers by 40 box pairs: 160 states, so draws repeat.
            let mut state = State {
                player: rng.below(4) as Cell,
                boxes: [NONE; MAX_BOXES],
            };
            let cell = 10 + rng.below(40) as Cell;
            state.boxes[..2].copy_from_slice(&[cell, cell + 50]);
            let (slot, found) = arena.find(&state);
            assert_eq!(found, current.get(&key(&state)).copied());
            let g = rng.below(1000) as u32;
            // As in the engine, only a cheaper duplicate is stored.
            if found.is_some_and(|id| nodes[id as usize].g <= g) {
                continue;
            }
            let node = Node {
                state,
                g,
                parent: if nodes.is_empty() {
                    NIL
                } else {
                    rng.below(nodes.len()) as u32
                },
                direction: rng.below(4) as u8,
                h: rng.below(100) as u16,
            };
            let id = arena.insert(node, slot);
            assert_eq!(id as usize, nodes.len());
            nodes.push(node);
            current.insert(key(&state), id);
            if rng.below(3) > 0 {
                let queue = Key::new(u64::from(g + u32::from(node.h)), u32::from(node.h), id);
                arena.enqueue(queue.f(), queue.h(), id);
                queued.push(queue);
            }
            if rng.below(4) == 0 {
                let lowest = queued.iter().copied().min();
                queued.retain(|&queue| Some(queue) != lowest);
                assert_eq!(arena.dequeue(), lowest);
            }
            assert_eq!(arena.min_f(), queued.iter().map(|queue| queue.f()).min());
            if current.len() * 2 > table {
                // From this seed the growth comes at the 35th insert, with
                // ten chunks allocated and two records superseded.
                assert_eq!(table, INITIAL_TABLE);
                assert!(arena.chunks.len() > 2 && nodes.len() > current.len());
                table = FINAL;
            }
            assert_eq!(arena.table.len(), table);
            assert_eq!(
                arena.chunks.len(),
                (arena.len() + 2).min(LIMIT + 1).div_ceil(CHUNK)
            );
            check(&arena, &nodes, &current);
        }
        assert_eq!((arena.len(), table), (LIMIT, FINAL));
        // Every chunk is allocated, exactly LIMIT + 1 records in all, and the
        // spare's insert grows nothing.
        let capacity = |arena: &Arena| {
            let chunks: Vec<_> = arena
                .chunks
                .iter()
                .map(|chunk| (chunk.records.capacity(), chunk.box_cells.capacity()))
                .collect();
            (chunks, arena.table.len())
        };
        let full = capacity(&arena);
        let records: usize = full.0.iter().map(|&(records, _)| records).sum();
        let cells: usize = full.0.iter().map(|&(_, cells)| cells).sum();
        assert_eq!((records, cells), (LIMIT + 1, (LIMIT + 1) * 2));
        let mut state = nodes[0].state;
        state.player = 9;
        let (slot, found) = arena.find(&state);
        assert_eq!(found, None);
        let spare = Node {
            state,
            g: 7,
            parent: LIMIT as u32 - 1,
            direction: 2,
            h: 0,
        };
        assert_eq!(arena.insert(spare, slot), LIMIT as u32);
        nodes.push(spare);
        current.insert(key(&state), LIMIT as u32);
        assert_eq!(capacity(&arena), full);
        check(&arena, &nodes, &current);
        // The spare's path, root first, moves down from the last chunks.
        let mut path = vec![LIMIT as u32];
        let mut parent = spare.parent;
        while parent != NIL {
            path.push(parent);
            parent = nodes[parent as usize].parent;
        }
        path.reverse();
        assert!(path.len() >= 3);
        assert_eq!(
            arena.clear_keeping(LIMIT as u32),
            Some(path.len() as u32 - 1)
        );
        assert_eq!((arena.len(), arena.dequeue()), (path.len(), None));
        for (new, &old) in path.iter().enumerate() {
            let (id, node) = (new as u32, nodes[old as usize]);
            let stored = arena.node(id);
            assert_eq!(stored.state, node.state);
            assert_eq!(
                (stored.g, stored.parent, stored.direction, stored.h),
                (
                    node.g,
                    id.checked_sub(1).unwrap_or(NIL),
                    node.direction,
                    node.h
                )
            );
            assert!(arena.is_superseded(id));
            assert_eq!(arena.find(&node.state).1, None);
        }
        let kept: usize = arena.chunks.iter().map(|chunk| chunk.records.len()).sum();
        assert_eq!(kept, path.len());
        assert_eq!(capacity(&arena), full);
    }

    /// A kept path stays at the front as detached records: root first,
    /// parents renumbered, every field and flag but superseded as it was,
    /// in no table slot and no queue. The counters are unchanged, the next
    /// insert follows the path, and a path that leaves no room for it is
    /// refused.
    #[test]
    fn clear_keeping_detaches_the_path_and_forgets_the_rest() {
        let at = |cell: Cell| {
            let mut state = State {
                player: 1,
                boxes: [NONE; MAX_BOXES],
            };
            state.boxes[..2].copy_from_slice(&[cell, cell + 1]);
            state
        };
        let mut arena = Arena::new(100, 2, 10, 4).unwrap();
        // (cell, g, parent, direction) per id: a root, a child the path
        // leaves behind, and the path's next two nodes, 0 -> 2 -> 3.
        let nodes = [(10, 0, NIL, 0), (20, 3, 0, 1), (30, 4, 0, 2), (40, 9, 2, 3)];
        for &(cell, g, parent, direction) in &nodes {
            let state = at(cell);
            let (slot, _) = arena.find(&state);
            let node = Node {
                state,
                g,
                parent,
                direction,
                h: 5,
            };
            let id = arena.insert(node, slot);
            arena.enqueue(u64::from(g) + 5, 5, id);
        }
        arena.close(0);
        arena.close(2);
        let counters = |arena: &Arena| {
            let counters = arena.counters();
            (
                counters.unique_states,
                counters.duplicate_improvements,
                counters.reopened_states,
                counters.peak_queue,
            )
        };
        let before = counters(&arena);
        assert_eq!(arena.clear_keeping(3), Some(2));
        assert_eq!((arena.len(), arena.dequeue()), (3, None));
        assert_eq!(counters(&arena), before);
        // Old ids 0, 2 and 3 are now 0, 1 and 2.
        for (new, (old, parent)) in [(0, NIL), (2, 0), (3, 1)].into_iter().enumerate() {
            let (cell, g, _, direction) = nodes[old];
            let id = new as u32;
            let node = arena.node(id);
            assert_eq!(node.state, at(cell));
            assert_eq!(
                (node.g, node.parent, node.direction, node.h),
                (g, parent, direction, 5)
            );
            assert_eq!(arena.meta(id).closed, old != 3);
            assert!(arena.is_superseded(id));
            assert_eq!(arena.find(&node.state).1, None);
        }
        // The child left behind is forgotten too, and counts as new again.
        let (slot, found) = arena.find(&at(20));
        assert_eq!(found, None);
        let node = Node {
            state: at(20),
            g: 3,
            parent: NIL,
            direction: 0,
            h: 5,
        };
        assert_eq!(arena.insert(node, slot), 3);
        assert_eq!(arena.counters.unique_states, before.0 + 1);

        // At a limit of 3, a path of 3 records leaves no room for a fourth.
        let mut full = Arena::new(100, 2, 3, 4).unwrap();
        for (id, cell) in [10, 20, 30].into_iter().enumerate() {
            let state = at(cell);
            let (slot, _) = full.find(&state);
            let node = Node {
                state,
                g: id as u32,
                parent: id.checked_sub(1).map_or(NIL, |parent| parent as u32),
                direction: 0,
                h: 5,
            };
            full.insert(node, slot);
        }
        assert_eq!(full.clear_keeping(2), None);
        assert_eq!((full.len(), full.find(&at(30)).1), (3, Some(2)));
        assert_eq!(full.clear_keeping(1), Some(1));
    }

    /// A refused growth allocation starves the arena: it is full, at
    /// [`Status::MemoryLimit`], with every record kept. A refused chunk
    /// leaves the table whole, so a kept path or a clear lets the arena
    /// fill and grow again. A refused final table is gone, so the arena
    /// keeps no path and stays starved through a clear.
    #[test]
    fn a_refused_growth_starves_the_arena() {
        let at = |cell: usize| {
            let mut state = State {
                player: 1,
                boxes: [NONE; MAX_BOXES],
            };
            state.boxes[0] = 10 + cell as Cell;
            Node {
                state,
                g: 0,
                parent: NIL,
                direction: 0,
                h: 1,
            }
        };
        let fill = |arena: &mut Arena, cells: std::ops::Range<usize>| {
            for cell in cells {
                let node = at(cell);
                let (slot, found) = arena.find(&node.state);
                assert_eq!(found, None);
                let id = arena.insert(node, slot);
                arena.enqueue(1, 1, id);
            }
        };
        let mut arena = Arena::new(100, 1, 40, 4).unwrap();
        assert_eq!(arena.limit_status(), Status::StateLimit);
        // The four-record first chunk of unit tests asks for the next at the
        // third insert.
        arena.refuse_growth(true);
        fill(&mut arena, 0..3);
        assert!(arena.starved() && arena.is_full());
        assert_eq!(arena.limit_status(), Status::MemoryLimit);
        assert_eq!(
            (arena.len(), arena.chunks.len(), arena.table.len()),
            (3, 1, INITIAL_TABLE)
        );
        assert_eq!(arena.find(&at(2).state).1, Some(2));
        // Once allocations succeed again, the kept path refills, past
        // several chunks and the table's growth.
        arena.refuse_growth(false);
        assert_eq!(arena.clear_keeping(2), Some(0));
        assert!(!arena.starved() && !arena.is_full());
        fill(&mut arena, 0..36);
        assert_eq!(
            (arena.len(), arena.chunks.len(), arena.table.len()),
            (37, 10, final_table(arena.node_limit + 1))
        );
        // The 39th insert asks for the last chunk; a clear lets the arena
        // refill to its limit, which it then reaches as a state limit.
        arena.refuse_growth(true);
        fill(&mut arena, 36..38);
        assert!(arena.starved() && arena.len() == 39);
        arena.refuse_growth(false);
        arena.clear();
        assert!(!arena.starved() && !arena.is_full());
        fill(&mut arena, 0..40);
        assert!(!arena.starved() && arena.is_full());
        assert_eq!((arena.len(), arena.chunks.len()), (40, 11));
        assert_eq!(arena.limit_status(), Status::StateLimit);

        // The table grows at the 33rd insert, when more than half of its
        // first 64 slots would be bound, and frees the first one before it
        // asks for the final one.
        let mut arena = Arena::new(100, 1, 40, 4).unwrap();
        fill(&mut arena, 0..32);
        arena.refuse_growth(true);
        fill(&mut arena, 32..33);
        assert!(arena.starved() && arena.is_full() && arena.table.is_empty());
        assert_eq!(arena.limit_status(), Status::MemoryLimit);
        assert_eq!(arena.node(32).state, at(32).state);
        arena.refuse_growth(false);
        assert_eq!(arena.clear_keeping(32), None);
        assert_eq!(arena.len(), 33);
        arena.clear();
        assert!(arena.starved() && arena.is_full() && arena.len() == 0);
    }

    /// Every record field at its extremes, with every flag combination.
    #[test]
    fn records_round_trip_every_field_at_its_extremes() {
        let flags = [
            0,
            Record::CLOSED,
            Record::SUPERSEDED,
            Record::CLOSED | Record::SUPERSEDED,
        ];
        let last_cell = (MAX_CELLS - 1) as Cell;
        // Straight through the packing, for ids no small arena reaches.
        for parent in [NIL, 0, 1, MAX_STATES as u32, ID_MASK - 1] {
            for direction in 0..4 {
                for (g, h) in [(0, 0), (1, u16::MAX - 1), (u32::MAX, u16::MAX)] {
                    for player in [0, last_cell] {
                        let node = Node {
                            state: State {
                                player,
                                boxes: [NONE; MAX_BOXES],
                            },
                            g,
                            parent,
                            direction,
                            h,
                        };
                        for &set in &flags {
                            let mut record = Record::new(&node);
                            assert_eq!(record.link & (Record::CLOSED | Record::SUPERSEDED), 0);
                            record.link |= set;
                            assert_eq!(
                                (record.g, record.parent(), record.direction()),
                                (g, parent, direction)
                            );
                            assert_eq!((record.player, record.h), (player, h));
                            assert_eq!(record.has(Record::CLOSED), set & Record::CLOSED != 0);
                            assert_eq!(
                                record.has(Record::SUPERSEDED),
                                set & Record::SUPERSEDED != 0
                            );
                        }
                    }
                }
            }
        }
        // Through the arena API, with every box cell at an extreme.
        let mut arena = Arena::new(MAX_CELLS, MAX_BOXES, 4, 4).unwrap();
        let mut state = State {
            player: last_cell,
            boxes: [NONE; MAX_BOXES],
        };
        for (i, cell) in state.boxes.iter_mut().enumerate() {
            *cell = if i.is_multiple_of(2) {
                i as Cell
            } else {
                last_cell - i as Cell
            };
        }
        let mut child = state;
        child.player = 0;
        let root = Node {
            state,
            g: u32::MAX,
            parent: NIL,
            direction: 3,
            h: u16::MAX,
        };
        let (slot, _) = arena.find(&state);
        let first = arena.insert(root, slot);
        arena.close(first);
        let (slot, _) = arena.find(&child);
        let leaf = Node {
            state: child,
            g: 0,
            parent: first,
            direction: 0,
            h: 0,
        };
        let second = arena.insert(leaf, slot);
        let (slot, _) = arena.find(&state);
        let improved = Node {
            g: u32::MAX - 1,
            h: u16::MAX - 1,
            ..root
        };
        let third = arena.insert(improved, slot);
        for (id, node, closed, superseded) in [
            (first, root, true, true),
            (second, leaf, false, false),
            (third, improved, false, false),
        ] {
            let stored = arena.node(id);
            assert_eq!(stored.state, node.state);
            assert_eq!(
                (stored.g, stored.parent, stored.direction, stored.h),
                (node.g, node.parent, node.direction, node.h)
            );
            let meta = arena.meta(id);
            assert_eq!((meta.g, meta.h, meta.closed), (node.g, node.h, closed));
            assert_eq!(meta.known_h(), node.known_h());
            assert_eq!(arena.is_superseded(id), superseded);
        }
        assert_eq!(arena.meta(first).known_h(), None);
        assert_eq!(arena.meta(third).known_h(), Some(u32::from(u16::MAX - 1)));
    }

    /// Key order is `(f, h, id)` order over every value that can be queued,
    /// and saturation keeps each field in range and after every exact f.
    #[test]
    fn keys_order_like_tuples_and_saturate_safely() {
        let fs = [0, 1, MAX_ROUTE as u64, Key::F_SAT - 1, Key::F_SAT];
        let hs = [0, 1, MAX_QUEUED_H - 1, MAX_QUEUED_H, Key::H_SAT];
        let ids = [0, 1, MAX_STATES as u32 - 1, MAX_STATES as u32, ID_MASK - 1];
        let mut keys = Vec::new();
        for f in fs {
            for h in hs {
                for id in ids {
                    let key = Key::new(f, h, id);
                    assert_eq!((key.f(), key.h(), key.id()), (f, h, id));
                    keys.push(((f, h, id), key));
                }
            }
        }
        for (a, key_a) in &keys {
            for (b, key_b) in &keys {
                assert_eq!(key_a.cmp(key_b), a.cmp(b), "{a:?} {b:?}");
            }
        }
        let last_exact = Key::new(Key::F_SAT - 1, Key::H_SAT, ID_MASK - 1);
        for (f, h, id) in [
            (Key::F_SAT + 1, 0, 0),
            (u64::MAX, MAX_QUEUED_H, 1),
            (u64::MAX, u32::MAX, ID_MASK - 1),
        ] {
            let key = Key::new(f, h, id);
            assert_eq!(
                (key.f(), key.h(), key.id()),
                (Key::F_SAT, h.min(Key::H_SAT), id)
            );
            assert!(key > last_exact);
        }
    }

    /// The key's field widths, with the largest key any node can carry one
    /// below `u64::MAX`, and the worst f a queued node can reach exact.
    #[test]
    fn keys_split_into_twenty_eighteen_and_twenty_six_bits() {
        assert_eq!(
            (Key::F_SAT, Key::H_SAT, ID_MASK),
            ((1 << 20) - 1, (1 << 18) - 1, (1 << 26) - 1)
        );
        let largest = Key::new(Key::F_SAT, Key::H_SAT, ID_MASK - 1);
        assert_eq!(largest.0, u64::MAX - 1);
        // MAX_ROUTE plus Policy::FAST's weight, the largest, times the largest
        // h (checked by scripts/mirrors.test.mjs).
        let worst = MAX_ROUTE as u64 + 5 * u64::from(MAX_QUEUED_H);
        assert_eq!(worst, 775_840);
        let key = Key::new(worst, MAX_QUEUED_H, MAX_STATES as u32);
        assert_eq!(
            (key.f(), key.h(), key.id()),
            (worst, MAX_QUEUED_H, MAX_STATES as u32)
        );
        // A saturated g saturates its key, past every exact f.
        let saturated = Key::new(u64::from(G_SAT), 0, 0);
        assert_eq!(saturated.f(), Key::F_SAT);
        assert!(saturated > Key::new(Key::F_SAT - 1, Key::H_SAT, ID_MASK - 1));
    }

    #[test]
    fn push_g_adds_the_walk_and_the_push_and_saturates() {
        assert_eq!(Node::push_g(0, 0), 1);
        assert_eq!(Node::push_g(10, 5), 16);
        assert_eq!(
            Node::push_g(MAX_ROUTE as u32, u16::MAX - 1),
            MAX_ROUTE as u32 + u32::from(u16::MAX)
        );
        assert_eq!(Node::push_g(G_SAT - 2, 0), G_SAT - 1);
        assert_eq!(Node::push_g(G_SAT - 1, 0), G_SAT);
        assert_eq!(Node::push_g(G_SAT - 1, u16::MAX - 1), G_SAT);
        assert_eq!(Node::push_g(G_SAT, 0), G_SAT);
        assert_eq!(Node::push_g(u32::MAX, u16::MAX), G_SAT);
    }

    /// A state reserves a 12-byte record, two bytes per box, an 8-byte queue
    /// entry and 8 to 16 bytes of table, so no budget holds a full
    /// `MAX_STATES`: 256 MiB holds 8,388,607 states at one box, where the
    /// next state would double the table, and 2,788,993 at `MAX_BOXES` on
    /// `MAX_CELLS` cells (`MAX_STATES`'s doc).
    #[test]
    fn memory_binds_a_full_limit_at_every_budget() {
        let top = *MEMORY_MIB_RANGE.end();
        for (cells, boxes, fits) in [
            (4, 1, 8_388_607),
            (MAX_CELLS, 1, 8_388_607),
            (4, MAX_BOXES, 2_793_036),
            (MAX_CELLS, MAX_BOXES, 2_788_993),
        ] {
            let arena = Arena::new(cells, boxes, MAX_STATES, top).unwrap();
            assert_eq!(arena.node_limit, fits, "{cells} cells, {boxes} boxes");
            assert_eq!(arena.limit_status(), Status::MemoryLimit);
            assert!(arena.reserved_bytes() <= top << 20);
        }
    }

    /// The exact edge at 64 MiB for the largest board and box count: 691,841
    /// states fit with 24 bytes to spare, and one more scales the request.
    #[test]
    fn a_limit_one_past_the_budget_is_scaled_to_it() {
        let fits = Arena::new(MAX_CELLS, MAX_BOXES, 691_841, 64).unwrap();
        assert_eq!(fits.node_limit, 691_841);
        assert_eq!(fits.limit_status(), Status::StateLimit);
        assert_eq!(fits.reserved_bytes(), (64 << 20) - 24);
        drop(fits);
        let scaled = Arena::new(MAX_CELLS, MAX_BOXES, 691_842, 64).unwrap();
        assert_eq!(scaled.node_limit, 691_841);
        assert_eq!(scaled.limit_status(), Status::MemoryLimit);
    }
}
