//! One table R(Q, P, LAB) and its resumable build.
//!
//! The relaxed problem of a table keeps only the boxes of labels LAB that
//! are inside room Q. The keeper walks the floor outside Q for free, a box
//! of LAB may enter through the door while the board has one to spare, and
//! a box leaves by a push onto the door. A goal state has the boxes of LAB
//! in Q on exactly Q's goals of their labels. A move counts when it ends
//! with the keeper on a cell of part P. The table holds, for each state
//! reachable from the start's projection that reaches a goal state, the
//! fewest counted moves it takes. docs/room-bound.md proves these are lower
//! bounds that add over disjoint parts, and that a searched state whose
//! projection is missing has no solution.
//!
//! A state's key packs a code per Q cell, `width` bits each: 0 when empty,
//! else the position of the box's label in LAB plus 1. The keeper's node
//! sits above them. A key fits 63 bits, so u64::MAX stays free as `EMPTY`.
//!
//! The build runs in slices of work units, so the search pays for it as it
//! pops (see `TableBuild::step`):
//! 1. A forward breadth-first search from the root's projection stores
//!    every reachable state, up to a cap. It refuses a move that leaves a
//!    box frozen off a goal of its code (`Rules::freezes`): no goal state
//!    follows such a move, so the refused states are ones the table would
//!    not keep. On huge's largest tables this cuts the states reached from
//!    10.25M to 2.69M, with the same values.
//! 2. The goal states seed distance 0.
//! 3. A backward 0/1 breadth-first search from them walks the moves in
//!    reverse. It generates each predecessor from the room's inverse move
//!    lists and keeps those the forward search reached. A predecessor test
//!    restates the forward move's predicate, so the backward search walks
//!    exactly the forward edges, without storing them: a stored edge list
//!    took 493 MiB on huge's largest table.
//! 4. The values of the states that reach a goal go into an open-addressing
//!    table, the only part the search keeps.
use super::room::{Moves, Room};
use super::{Ledger, SizeFailure, bit};
use crate::{arena::mix, heuristic::Heuristic};
use sokomind_core::{Board, MAX_BOXES, NONE, OPPOSITE, State};
use std::mem::{size_of, take};

/// An empty slot of `Values`.
const EMPTY: u64 = u64::MAX;
/// A free slot of the forward search's slot table.
const FREE: u32 = u32::MAX;
/// Bits of a slot that hold a state's id.
const ID_BITS: u32 = 25;
/// Low `ID_BITS` set: extracts an id.
const ID_MASK: u32 = (1 << ID_BITS) - 1;
/// Bits of a slot above its id, which hold its key's [`tag`].
const TAG_BITS: u32 = u32::BITS - ID_BITS;
// Ids stay below the cap, so a bound slot's low bits are never all ones,
// and it never reads as `FREE`.
const _: () = assert!(super::MAX_CAP < ID_MASK as u64);
/// A wall next to a Q cell, in `Rules::near`.
const WALL_NODE: u8 = u8::MAX;
/// A distance the backward search has not reached.
const INFINITE: u16 = u16::MAX;
/// The largest value a table stores. A larger distance is stored as this
/// one: min(R, MAX_VALUE) is still a lower bound, and still consistent,
/// since R(parent) <= cost + R(child) implies the same of the minima.
pub(super) const MAX_VALUE: u16 = INFINITE - 1;
/// States per chunk of the forward search's state list. Chunks never move,
/// so growing the list copies nothing.
const CHUNK: usize = 1 << 16;
/// The forward search's first slot table.
const FIRST_SLOTS: usize = 1 << 10;
/// States one work unit scans when seeding or inserting values, about the
/// work of expanding one state.
const SCAN: usize = 16;

/// The bits `x` needs.
fn bits(x: usize) -> usize {
    (usize::BITS - x.leading_zeros()) as usize
}

/// Hash `h`'s tag, already shifted into a slot's top `TAG_BITS`.
fn tag(h: u64) -> u32 {
    ((h >> (u64::BITS - TAG_BITS)) as u32) << ID_BITS
}

/// A table's values: open addressing over the keys, with linear probing.
#[derive(Default)]
struct Values {
    keys: Vec<u64>,
    values: Vec<u16>,
    mask: usize,
}

impl Values {
    /// The slots for `count` values, at most 3/4 full.
    fn slots(count: usize) -> usize {
        (count * 4 / 3 + 1).next_power_of_two().max(2)
    }

    /// The bytes `count` values take.
    fn bytes_for(count: usize) -> usize {
        Self::slots(count) * (size_of::<u64>() + size_of::<u16>())
    }

    fn allocate(&mut self, count: usize, ledger: &mut Ledger) -> Result<(), SizeFailure> {
        let slots = Self::slots(count);
        self.keys = ledger.filled(slots, EMPTY)?;
        self.values = ledger.filled(slots, 0)?;
        self.mask = slots - 1;
        Ok(())
    }

    fn insert(&mut self, key: u64, value: u16) {
        let mut at = mix(key) as usize & self.mask;
        while self.keys[at] != EMPTY {
            at = (at + 1) & self.mask;
        }
        self.keys[at] = key;
        self.values[at] = value;
    }

    fn get(&self, key: u64) -> Option<u16> {
        let mut at = mix(key) as usize & self.mask;
        loop {
            match self.keys[at] {
                k if k == key => return Some(self.values[at]),
                EMPTY => return None,
                _ => at = (at + 1) & self.mask,
            }
        }
    }

    fn bytes(&self) -> usize {
        self.keys.capacity() * size_of::<u64>() + self.values.capacity() * size_of::<u16>()
    }

    fn release(&mut self, ledger: &mut Ledger) {
        ledger.release(take(&mut self.keys));
        ledger.release(take(&mut self.values));
    }
}

/// The forward search's states by id, and a slot table of ids.
#[derive(Default)]
struct States {
    /// The keys by id, `CHUNK` per chunk. Only the first chunk grows; the
    /// rest are allocated full.
    chunks: Vec<Vec<u64>>,
    /// Open addressing over the keys, with linear probing: `FREE`, or an
    /// id in the low `ID_BITS` with its key's [`tag`] above. A probe reads a
    /// slot's key only when the tags match, one time in 128 for another
    /// key. On a large table a key read is a cache miss, and a probe passes
    /// a few slots, since the table may be 3/4 full. Never fuller.
    slots: Vec<u32>,
    len: usize,
}

impl States {
    fn len(&self) -> usize {
        self.len
    }

    fn get(&self, id: usize) -> u64 {
        self.chunks[id / CHUNK][id % CHUNK]
    }

    /// The id of `key`, whose hash is `h`, or the free slot where it goes.
    fn probe(&self, key: u64, h: u64) -> Result<u32, usize> {
        let (mask, tag) = (self.slots.len() - 1, tag(h));
        let mut at = h as usize & mask;
        loop {
            match self.slots[at] {
                FREE => return Err(at),
                slot if slot & !ID_MASK == tag && self.get((slot & ID_MASK) as usize) == key => {
                    return Ok(slot & ID_MASK);
                }
                _ => at = (at + 1) & mask,
            }
        }
    }

    fn find(&self, key: u64) -> Option<u32> {
        self.probe(key, mix(key)).ok()
    }

    /// Adds `key` when it is new, failing past `cap` states.
    fn add(&mut self, key: u64, cap: usize, ledger: &mut Ledger) -> Result<(), SizeFailure> {
        let h = mix(key);
        let Err(at) = self.probe(key, h) else {
            return Ok(());
        };
        if self.len >= cap {
            return Err(SizeFailure);
        }
        if self.chunks.last().is_none_or(|chunk| chunk.len() == CHUNK) {
            // The chunk goes in empty first, so a refused allocation leaves
            // nothing uncounted.
            let first = self.chunks.is_empty();
            ledger.push(&mut self.chunks, Vec::new())?;
            if !first {
                *self.chunks.last_mut().expect("the new chunk") = ledger.vec(CHUNK)?;
            }
        }
        let last = self.chunks.last_mut().expect("a chunk with room");
        ledger.push(last, key)?;
        self.slots[at] = tag(h) | self.len as u32;
        self.len += 1;
        if self.len * 4 > self.slots.len() * 3 {
            self.grow(ledger)?;
        }
        Ok(())
    }

    /// Doubles the slot table. The rehash's work, at most one probe per
    /// state per doubling, is counted in the units of the expansions that
    /// added the states.
    fn grow(&mut self, ledger: &mut Ledger) -> Result<(), SizeFailure> {
        let mut slots = ledger.filled(2 * self.slots.len(), FREE)?;
        let mask = slots.len() - 1;
        for id in 0..self.len {
            let h = mix(self.get(id));
            let mut at = h as usize & mask;
            while slots[at] != FREE {
                at = (at + 1) & mask;
            }
            slots[at] = tag(h) | id as u32;
        }
        ledger.release(std::mem::replace(&mut self.slots, slots));
        Ok(())
    }

    fn release(&mut self, ledger: &mut Ledger) {
        for chunk in take(&mut self.chunks) {
            ledger.release(chunk);
        }
        ledger.release(take(&mut self.slots));
        self.len = 0;
    }
}

/// What a table's searches read: the key layout and the predicates of the
/// relaxed problem's moves.
struct Rules {
    part: u64,
    /// Q's cell count, and the bits a code takes.
    n: usize,
    width: usize,
    /// Per Q index, then `n` for the door, bit c set when a lone box of
    /// code c there can still reach a goal of its label.
    alive: Vec<u64>,
    /// Per code, the Q indices where a lone box of that code is alive but
    /// can't leave Q over live cells: more such boxes than goals in Q is a
    /// dead state.
    stuck: Vec<u64>,
    /// Per code, its label's goals in Q.
    goals_in: Vec<u32>,
    /// Per code, its label's boxes on the board.
    total: Vec<u32>,
    /// Every goal in Q of a label in LAB holding a box of its code.
    goal_layout: u64,
    /// Per Q index, the node of its neighbor in each direction: a Q index,
    /// `n` for the door, or `WALL_NODE`.
    near: Vec<[u8; 4]>,
}

impl Rules {
    fn get(&self, layout: u64, q: usize) -> usize {
        ((layout >> (q * self.width)) & ((1 << self.width) - 1)) as usize
    }

    fn put(&self, layout: u64, q: usize, c: usize) -> u64 {
        layout | ((c as u64) << (q * self.width))
    }

    fn clear(&self, layout: u64, q: usize) -> u64 {
        layout & !(((1 << self.width) - 1) << (q * self.width))
    }

    fn key(&self, layout: u64, node: usize) -> u64 {
        layout | ((node as u64) << (self.n * self.width))
    }

    fn layout(&self, key: u64) -> u64 {
        key & ((1 << (self.n * self.width)) - 1)
    }

    fn node(&self, key: u64) -> usize {
        (key >> (self.n * self.width)) as usize
    }

    /// Whether a move that ends with the keeper on node `u` counts.
    fn counts(&self, u: usize) -> bool {
        u < self.n && bit(self.part, u)
    }

    /// Whether `layout` holds more boxes of code `c` on its stuck cells
    /// than Q has goals of its label.
    fn dead(&self, layout: u64, c: usize) -> bool {
        let mut cells = self.stuck[c];
        let mut on = 0;
        while cells != 0 {
            let q = cells.trailing_zeros() as usize;
            cells &= cells - 1;
            on += u32::from(self.get(layout, q) == c);
        }
        on > self.goals_in[c]
    }

    fn count(&self, layout: u64, c: usize) -> u32 {
        (0..self.n).filter(|&q| self.get(layout, q) == c).count() as u32
    }

    /// Whether the box on Q index `q` can never move: on each axis, a side
    /// is a wall or a box of `fixed`, or neither side is a cell the box may
    /// be pushed onto, or a side holds a box frozen itself, with this one
    /// taken for a wall. No box of such a set can move first, since each is
    /// blocked while the others stay, so none ever moves. Recursion adds a
    /// box to `fixed` each time, so it ends.
    fn frozen(&self, layout: u64, q: usize, fixed: u64) -> bool {
        let (c, fixed, near) = (self.get(layout, q), fixed | (1 << q), self.near[q]);
        let wall = |x: u8| x == WALL_NODE || (usize::from(x) < self.n && bit(fixed, x.into()));
        let boxed = |x: u8| usize::from(x) < self.n && self.get(layout, x.into()) != 0;
        [[near[0], near[1]], [near[2], near[3]]]
            .into_iter()
            .all(|sides| {
                // Past the first test neither side is a wall, so both index
                // `alive`.
                sides.into_iter().any(wall)
                    || sides
                        .into_iter()
                        .all(|x| !bit(self.alive[usize::from(x)], c))
                    || sides
                        .into_iter()
                        .any(|x| boxed(x) && self.frozen(layout, x.into(), fixed))
            })
    }

    /// Whether the box just put on Q index `t` leaves a box frozen off a
    /// goal of its code, on `t` or next to it. That box never moves, and a
    /// goal state holds every box of Q on a goal of its code, so the state
    /// reaches no goal state, and neither does any state it reaches. Only a
    /// box next to `t` may freeze anew; one further along a frozen chain
    /// is left to the backward search, which never reaches it either.
    fn freezes(&self, layout: u64, t: usize) -> bool {
        std::iter::once(t as u8).chain(self.near[t]).any(|x| {
            let x = usize::from(x);
            x < self.n && {
                let c = self.get(layout, x);
                c != 0 && c != self.get(self.goal_layout, x) && self.frozen(layout, x, 0)
            }
        })
    }

    /// Calls `emit(key, counts)` for each move from state `key`: the state
    /// it reaches, and whether it counts. A state's keeper never stands on
    /// a box, and every move keeps it so.
    fn successors(
        &self,
        moves: &Moves,
        key: u64,
        mut emit: impl FnMut(u64, bool) -> Result<(), SizeFailure>,
    ) -> Result<(), SizeFailure> {
        let (n, layout, u) = (self.n, self.layout(key), self.node(key));
        for &v in moves.steps(u) {
            let v = usize::from(v);
            if v == n || self.get(layout, v) == 0 {
                emit(self.key(layout, v), self.counts(v))?;
            }
        }
        for &(x, ti) in moves.pushes(u) {
            let (x, ti) = (usize::from(x), usize::from(ti));
            let c = self.get(layout, x);
            if c == 0 || !bit(self.alive[ti], c) {
                continue;
            }
            let cleared = self.clear(layout, x);
            let next = if ti == n {
                cleared
            } else {
                let next = self.put(cleared, ti, c);
                if self.get(layout, ti) != 0 || self.dead(next, c) || self.freezes(next, ti) {
                    continue;
                }
                next
            };
            emit(self.key(next, x), self.counts(x))?;
        }
        for &ti in moves.entries(u) {
            let ti = usize::from(ti);
            if self.get(layout, ti) != 0 {
                continue;
            }
            for (c, &total) in self.total.iter().enumerate().skip(1) {
                if !bit(self.alive[ti] & self.alive[n], c) || self.count(layout, c) >= total {
                    continue;
                }
                let next = self.put(layout, ti, c);
                if !self.dead(next, c) && !self.freezes(next, ti) {
                    emit(self.key(next, n), false)?;
                }
            }
        }
        Ok(())
    }

    /// Calls `visit(key, counts)` for each state with a move onto state
    /// `key`, and whether that move counts: `successors` read backward, each
    /// forward predicate restated for the predecessor but `freezes`. The
    /// backward search calls this only on states that reach a goal state,
    /// which freeze no box off its goal, so the forward search took every
    /// move onto them. A predecessor need not be reachable from the root;
    /// the caller looks it up.
    ///
    /// The keeper's node `u` before a move is never the box's cell before
    /// or after it, so whether `u` is free reads the same in `key`.
    fn predecessors(
        &self,
        moves: &Moves,
        key: u64,
        mut visit: impl FnMut(u64, bool) -> Result<(), SizeFailure>,
    ) -> Result<(), SizeFailure> {
        let (n, layout, w) = (self.n, self.layout(key), self.node(key));
        let free = |u: usize| u == n || self.get(layout, u) == 0;
        // Steps are symmetric, and `w` is free, as the keeper stands on it.
        for &u in moves.steps(w) {
            let u = usize::from(u);
            if free(u) {
                visit(self.key(layout, u), self.counts(w))?;
            }
        }
        if w < n {
            // A push from `u` moved a box from `w` onto `ti`.
            for &(u, ti) in moves.pulls(w) {
                let (u, ti) = (usize::from(u), usize::from(ti));
                if !free(u) {
                    continue;
                }
                if ti == n {
                    for c in 1..self.total.len() {
                        if bit(self.alive[n], c) {
                            visit(self.key(self.put(layout, w, c), u), self.counts(w))?;
                        }
                    }
                } else {
                    let c = self.get(layout, ti);
                    if c == 0 || !bit(self.alive[ti], c) || self.dead(layout, c) {
                        continue;
                    }
                    let before = self.put(self.clear(layout, ti), w, c);
                    visit(self.key(before, u), self.counts(w))?;
                }
            }
        } else {
            // An entry from `u` put a box on `ti`. Its count check holds:
            // this state is reachable, so it holds at most `total[c]` boxes
            // of code c, and the predecessor one fewer.
            for &(u, ti) in moves.unentries() {
                let (u, ti) = (usize::from(u), usize::from(ti));
                let c = self.get(layout, ti);
                if c == 0
                    || !bit(self.alive[ti] & self.alive[n], c)
                    || self.dead(layout, c)
                    || !free(u)
                {
                    continue;
                }
                visit(self.key(self.clear(layout, ti), u), false)?;
            }
        }
        Ok(())
    }
}

/// One built table R(Q, P, LAB), over the room its build was given.
pub(super) struct Table {
    part: u64,
    /// Per box slot, its label's code in LAB, 0 when not in LAB.
    code: [u8; MAX_BOXES],
    boxes: usize,
    width: usize,
    values: Values,
    /// The states the forward search reached.
    reached: usize,
}

impl Table {
    pub(super) fn part(&self) -> u64 {
        self.part
    }

    pub(super) fn reached(&self) -> usize {
        self.reached
    }

    /// The value of `state`'s projection, or `None` when it has no
    /// solution.
    pub(super) fn value(&self, room: &Room, state: &State) -> Option<u32> {
        let mut key = 0u64;
        for (&code, &cell) in self.code[..self.boxes].iter().zip(&state.boxes) {
            if code == 0 {
                continue;
            }
            if let Some(q) = room.index(cell) {
                key |= u64::from(code) << (q * self.width);
            }
        }
        key |= (room.node(state.player) as u64) << (room.cells().len() * self.width);
        self.values.get(key).map(u32::from)
    }

    /// The heap bytes the table holds, all charged to the ledger.
    pub(super) fn bytes(&self) -> usize {
        self.values.bytes()
    }

    pub(super) fn free(mut self, ledger: &mut Ledger) {
        self.values.release(ledger);
    }
}

/// What a slice of `TableBuild::step` ended with.
pub(super) enum Step {
    /// The units ran out first.
    Running,
    /// The table, or `None` when the root's projection has no solution.
    Done(Option<Table>),
}

#[derive(Clone, Copy)]
enum Phase {
    Start,
    /// Expanding state `at`.
    Forward {
        at: usize,
    },
    /// Scanning state `at` for goal states.
    Seed {
        at: usize,
    },
    /// Popping states at distance `level`.
    Back {
        level: u16,
    },
    /// Inserting the value of state `at`.
    Insert {
        at: usize,
    },
    Built,
    Unreached,
}

/// A resumable build of one table: the phases of the module docs, in
/// slices of work units.
pub(super) struct TableBuild {
    rules: Rules,
    code: [u8; MAX_BOXES],
    boxes: usize,
    root: u64,
    /// The most states the forward search may reach.
    cap: usize,
    states: States,
    /// Per state, its distance to a goal state, or `INFINITE`.
    dist: Vec<u16>,
    /// The backward search's worklists: states at the current level, then
    /// at the next.
    cur: Vec<u32>,
    next: Vec<u32>,
    /// The states with a finite distance.
    finite: usize,
    values: Values,
    phase: Phase,
}

impl TableBuild {
    /// The build of R(room, part, LAB), LAB the labels with a start box or
    /// a goal on `part`. `None` when there are no such labels, or a key
    /// would need more than 63 bits.
    pub(super) fn new(
        board: &Board,
        heuristic: &Heuristic,
        room: &Room,
        start: &State,
        part: u64,
        cap: usize,
    ) -> Option<Self> {
        debug_assert!(cap as u64 <= super::MAX_CAP);
        let labels = room.labels(board, start, part);
        let (k, n) = (labels.len(), room.cells().len());
        let width = bits(k);
        if k == 0 || n * width + bits(room.nodes() - 1) > 63 {
            return None;
        }
        let boxes = board.labels().len();
        let mut code = [0u8; MAX_BOXES];
        for (slot, &label) in code.iter_mut().zip(board.labels()) {
            *slot = labels
                .iter()
                .position(|&l| l == label)
                .map_or(0, |c| c as u8 + 1);
        }
        // Per code, a box slot of its label for the dead table, and the
        // board's box count of that label. Every label in LAB has a box, so
        // code 0's entry is never read.
        let slots = &code[..boxes];
        let rep: Vec<usize> = (0..=k)
            .map(|c| slots.iter().position(|&x| usize::from(x) == c).unwrap_or(0))
            .collect();
        let total: Vec<u32> = (0..=k)
            .map(|c| slots.iter().filter(|&&x| usize::from(x) == c).count() as u32)
            .collect();
        let alive: Vec<u64> = room
            .cells()
            .iter()
            .copied()
            .chain([room.door()])
            .map(|cell| {
                (1..=k)
                    .filter(|&c| !heuristic.dead(rep[c], cell))
                    .fold(0, |mask, c| mask | (1 << c))
            })
            .collect();
        // A cell next to Q is in Q or is the door, node `n`.
        let neighbors = board.neighbors();
        let near: Vec<[u8; 4]> = room
            .cells()
            .iter()
            .map(|&x| {
                neighbors[usize::from(x)].map(|y| match y {
                    NONE => WALL_NODE,
                    y => room.node(y) as u8,
                })
            })
            .collect();
        let stuck: Vec<u64> = (0..=k)
            .map(|c| {
                let live = |q: usize| bit(alive[q], c);
                let mut free = 0u64;
                loop {
                    let mut changed = false;
                    for (q, &x) in room.cells().iter().enumerate() {
                        if bit(free, q) || !live(q) {
                            continue;
                        }
                        let near = &neighbors[usize::from(x)];
                        let leaves = (0..OPPOSITE.len()).any(|d| {
                            let (s, t) = (near[OPPOSITE[d]], near[d]);
                            s != NONE && t != NONE && {
                                let ti = room.node(t);
                                live(ti) && (ti == n || bit(free, ti))
                            }
                        });
                        if leaves {
                            free |= 1 << q;
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }
                (0..n)
                    .filter(|&q| live(q) && !bit(free, q))
                    .fold(0, |stuck, q| stuck | (1 << q))
            })
            .collect();
        let mut goals_in = vec![0; k + 1];
        let mut goal_layout = 0u64;
        for &(cell, label) in board.goals() {
            let (Some(q), Some(c)) = (room.index(cell), labels.iter().position(|&l| l == label))
            else {
                continue;
            };
            goals_in[c + 1] += 1;
            goal_layout |= (c as u64 + 1) << (q * width);
        }
        let rules = Rules {
            part,
            n,
            width,
            alive,
            stuck,
            goals_in,
            total,
            goal_layout,
            near,
        };
        let mut root = 0u64;
        for (&c, &cell) in slots.iter().zip(&start.boxes) {
            if c != 0
                && let Some(q) = room.index(cell)
            {
                root = rules.put(root, q, usize::from(c));
            }
        }
        let root = rules.key(root, room.node(start.player));
        Some(Self {
            rules,
            code,
            boxes,
            root,
            cap,
            states: States::default(),
            dist: Vec::new(),
            cur: Vec::new(),
            next: Vec::new(),
            finite: 0,
            values: Values::default(),
            phase: Phase::Start,
        })
    }

    /// Runs the build for at most `units` work units, taking those it uses
    /// from `units`. A unit expands, pops or scans one state, or scans
    /// `SCAN` states, and the search pays for it as part of a pop. On an
    /// error or `Done` the build has freed its buffers, the table aside,
    /// and must not be stepped again.
    pub(super) fn step(
        &mut self,
        moves: &Moves,
        units: &mut usize,
        ledger: &mut Ledger,
    ) -> Result<Step, SizeFailure> {
        let step = self.run(moves, units, ledger);
        if !matches!(step, Ok(Step::Running)) {
            self.free(ledger);
        }
        step
    }

    fn run(
        &mut self,
        moves: &Moves,
        units: &mut usize,
        ledger: &mut Ledger,
    ) -> Result<Step, SizeFailure> {
        loop {
            self.phase = match self.phase {
                Phase::Built => return Ok(Step::Done(Some(self.table()))),
                Phase::Unreached => return Ok(Step::Done(None)),
                _ if *units == 0 => return Ok(Step::Running),
                Phase::Start => {
                    *units -= 1;
                    self.states.slots = ledger.filled(FIRST_SLOTS, FREE)?;
                    self.states.add(self.root, self.cap, ledger)?;
                    Phase::Forward { at: 0 }
                }
                Phase::Forward { at } => self.forward(moves, at, units, ledger)?,
                Phase::Seed { at } => self.seed(at, units, ledger)?,
                Phase::Back { level } => self.back(moves, level, units, ledger)?,
                Phase::Insert { at } => self.insert(at, units),
            };
        }
    }

    fn forward(
        &mut self,
        moves: &Moves,
        mut at: usize,
        units: &mut usize,
        ledger: &mut Ledger,
    ) -> Result<Phase, SizeFailure> {
        let Self {
            rules, states, cap, ..
        } = self;
        while at < states.len() {
            if *units == 0 {
                return Ok(Phase::Forward { at });
            }
            *units -= 1;
            let key = states.get(at);
            rules.successors(moves, key, |next, _| states.add(next, *cap, ledger))?;
            at += 1;
        }
        self.dist = ledger.filled(self.states.len(), INFINITE)?;
        Ok(Phase::Seed { at: 0 })
    }

    fn seed(
        &mut self,
        mut at: usize,
        units: &mut usize,
        ledger: &mut Ledger,
    ) -> Result<Phase, SizeFailure> {
        let len = self.states.len();
        while at < len {
            if *units == 0 {
                return Ok(Phase::Seed { at });
            }
            *units -= 1;
            let end = (at + SCAN).min(len);
            for id in at..end {
                if self.rules.layout(self.states.get(id)) == self.rules.goal_layout {
                    self.dist[id] = 0;
                    self.finite += 1;
                    ledger.push(&mut self.cur, id as u32)?;
                }
            }
            at = end;
        }
        Ok(Phase::Back { level: 0 })
    }

    /// The backward search, levels by distance. A state enters `cur` or
    /// `next` only when its distance drops, so at most twice: once at the
    /// next level, then maybe at the current one. A pop whose distance has
    /// since dropped is stale and skipped.
    fn back(
        &mut self,
        moves: &Moves,
        mut level: u16,
        units: &mut usize,
        ledger: &mut Ledger,
    ) -> Result<Phase, SizeFailure> {
        let Self {
            rules,
            states,
            dist,
            cur,
            next,
            finite,
            ..
        } = self;
        loop {
            if *units == 0 {
                return Ok(Phase::Back { level });
            }
            let Some(id) = cur.pop() else {
                if next.is_empty() {
                    break;
                }
                // Distances stop at MAX_VALUE, so `next` is empty there.
                std::mem::swap(cur, next);
                level += 1;
                continue;
            };
            *units -= 1;
            if dist[id as usize] != level {
                continue;
            }
            rules.predecessors(moves, states.get(id as usize), |key, counts| {
                let Some(p) = states.find(key) else {
                    return Ok(());
                };
                let (p, d) = (p as usize, (level + u16::from(counts)).min(MAX_VALUE));
                if d < dist[p] {
                    *finite += usize::from(dist[p] == INFINITE);
                    dist[p] = d;
                    ledger.push(if d == level { &mut *cur } else { &mut *next }, p as u32)?;
                }
                Ok(())
            })?;
        }
        // The root is state 0.
        if self.dist[0] == INFINITE {
            return Ok(Phase::Unreached);
        }
        let Self {
            states, cur, next, ..
        } = self;
        ledger.release(take(&mut states.slots));
        ledger.release(take(cur));
        ledger.release(take(next));
        if !ledger.fits_kept(Values::bytes_for(self.finite)) {
            return Err(SizeFailure);
        }
        self.values.allocate(self.finite, ledger)?;
        Ok(Phase::Insert { at: 0 })
    }

    fn insert(&mut self, mut at: usize, units: &mut usize) -> Phase {
        let len = self.states.len();
        while at < len {
            if *units == 0 {
                return Phase::Insert { at };
            }
            *units -= 1;
            let end = (at + SCAN).min(len);
            for id in at..end {
                if self.dist[id] != INFINITE {
                    self.values.insert(self.states.get(id), self.dist[id]);
                }
            }
            at = end;
        }
        Phase::Built
    }

    fn table(&mut self) -> Table {
        Table {
            part: self.rules.part,
            code: self.code,
            boxes: self.boxes,
            width: self.rules.width,
            values: take(&mut self.values),
            reached: self.states.len(),
        }
    }

    /// Frees every buffer the build holds. A build abandoned mid-run must
    /// be freed, so the ledger gets its bytes back.
    pub(super) fn free(&mut self, ledger: &mut Ledger) {
        self.states.release(ledger);
        ledger.release(take(&mut self.dist));
        ledger.release(take(&mut self.cur));
        ledger.release(take(&mut self.next));
        self.values.release(ledger);
    }
}
