//! Route polish: once a Quality search ends with a route, shortens it by
//! rescheduling one box at a time. A job keeps every other box's pushes, in
//! their order, and runs A* over where the chosen box's own pushes go among
//! them: from the start to the goal cell the box ended on, with every walk
//! taken as short as the boxes then allow. Its bound is the current route,
//! so anything it finds is shorter; a found route is replayed before it is
//! kept. Jobs cycle over the boxes until a full pass gains nothing. The
//! design, its soundness and its memory budget are in docs/route-polish.md.
use crate::{SolutionError, Status, heuristic::Heuristic, reach::Reach, stage::filled};
use sokomind_core::{Board, Cell, MAX_ROUTE, NONE, OPPOSITE, State, Step};
use std::{collections::BinaryHeap, mem::size_of};

/// Replays `route` from `start` and returns how many of its moves reach the
/// first solved state: 0 when the start is solved, the route's length when
/// only its last move solves it. Only a push can solve a state, so only
/// pushes are checked. A move the rules refuse is [`SolutionError::Replay`],
/// a route that never solves the board [`SolutionError::Counters`].
pub(crate) fn replay(board: &Board, start: &State, route: &[u8]) -> Result<usize, SolutionError> {
    let mut state = *start;
    if board.solved(&state) {
        return Ok(0);
    }
    for (i, &direction) in route.iter().enumerate() {
        match board.step(&mut state, direction as usize) {
            None => return Err(SolutionError::Replay),
            Some(Step::Push(_)) if board.solved(&state) => return Ok(i + 1),
            Some(_) => {}
        }
    }
    Err(SolutionError::Counters)
}

/// The polisher: the best route so far, the status the search ended with,
/// and the box job running, if any. One [`Polish::step`] is one unit of a
/// job, so the engine slices it by pops like the search.
pub(crate) struct Polish {
    /// Direction indices, replay-verified to reach the first solved state at
    /// their last move.
    route: Vec<u8>,
    ending: Status,
    /// Bytes a job may hold: the arena's share of the budget, which the
    /// engine released before polishing.
    allowance: u64,
    /// The box slot the next job reschedules.
    next: usize,
    /// Jobs started since the route last got shorter, counting the one that
    /// shortened it. A full cycle over the boxes ends the polish.
    idle: usize,
    job: Option<Job>,
}

impl Polish {
    pub(crate) fn new(route: Vec<u8>, ending: Status, allowance: u64) -> Self {
        Self {
            route,
            ending,
            allowance,
            next: 0,
            idle: 0,
            job: None,
        }
    }
    pub(crate) fn moves(&self) -> u32 {
        self.route.len() as u32
    }
    pub(crate) fn route(&self) -> &[u8] {
        &self.route
    }
    /// The status the search ended with before polishing.
    pub(crate) fn ending(&self) -> Status {
        self.ending
    }
    /// Drops the running job; the route stays the best so far.
    pub(crate) fn stop(&mut self) {
        self.job = None;
    }
    /// Runs one unit: starts the next box's job, or advances the running
    /// one. A job that finds a shorter route ends, and its route replaces
    /// this one once replay confirms it, cut at its first solved state.
    /// True once every box's job in a full cycle gained nothing; every
    /// later call returns true again.
    pub(crate) fn step(
        &mut self,
        board: &Board,
        heuristic: &Heuristic,
        start: &State,
        reach: &mut Reach,
    ) -> bool {
        let boxes = board.labels().len();
        let Some(job) = &mut self.job else {
            if self.idle >= boxes {
                return true;
            }
            self.idle += 1;
            let slot = self.next;
            self.next = (slot + 1) % boxes;
            // A box that never moves, or whose job does not fit the
            // allowance, gets no job and counts as idle.
            self.job = Job::new(board, heuristic, start, &self.route, slot, self.allowance);
            return false;
        };
        match job.step(board, heuristic, reach) {
            Progress::Running => {}
            Progress::Failed => self.job = None,
            Progress::Found(mut route) => {
                self.job = None;
                let solved_at = replay(board, start, &route);
                // A route can solve the board before its end when the box
                // reaches its goal while another box is briefly off its own.
                debug_assert!(
                    solved_at.is_ok_and(|n| n <= route.len() && n < self.route.len()),
                    "{solved_at:?}"
                );
                if let Ok(n) = solved_at
                    && n < self.route.len()
                {
                    route.truncate(n);
                    self.route = route;
                    self.idle = 1;
                }
            }
        }
        false
    }
}

/// One push of another box, which a job keeps in place.
#[derive(Clone, Copy)]
struct Event {
    /// The keeper's cell before the push.
    stand: Cell,
    /// The keeper's cell after it, the one the box left.
    from: Cell,
    /// The box's new cell.
    to: Cell,
    direction: u8,
}

/// What a job unit did.
enum Progress {
    Running,
    /// The job ended with nothing shorter, or could not grow its queue.
    Failed,
    /// A route shorter than the bound, as direction indices.
    Found(Vec<u8>),
}

// Queue keys pack, from the top: the bit-flipped f, the bit-flipped h, and
// the node id. `BinaryHeap` is a max-heap, so it pops the lowest f, then the
// lowest h (the deepest node), then the highest id: one fixed order, so a
// job runs the same however its units are sliced.
const F_BITS: u32 = 17;
const F_MASK: u64 = (1 << F_BITS) - 1;
const ID_BITS: u32 = 30;
const ID_MASK: u64 = (1 << ID_BITS) - 1;
const _: () = assert!(2 * F_BITS + ID_BITS == u64::BITS);
// Every queued f is below the bound, a route length, so it never reaches
// F_MASK, and h <= f.
const _: () = assert!((MAX_ROUTE as u64) < F_MASK);

fn key(f: u32, h: u32, id: usize) -> u64 {
    ((F_MASK - u64::from(f)) << (F_BITS + ID_BITS))
        | ((F_MASK - u64::from(h)) << ID_BITS)
        | id as u64
}

/// A node's tags: 0 when it was reached by another box's push, or is the
/// root, else 1 plus the direction of the box's own push that reached it.
/// The tag and the box's cell fix the keeper's cell.
const TAGS: usize = 5;
/// The fewest entries a job's queue may grow to, below which it is not
/// worth starting.
const MIN_QUEUE: usize = 1024;

/// One box's rescheduling: A* over nodes (k, p, tag), where k of the `m`
/// kept pushes are done, the box is on `p`, and the tag fixes the keeper.
/// A node's cost is the moves so far, walks included. From each node the
/// next kept push goes (unless it would push a box onto this one), or one
/// of the box's own pushes, each after the shortest walk to its stand with
/// every box in place. The goal is (m, goal).
///
/// The heuristic is the larger of two lower bounds, both consistent (see
/// docs/route-polish.md): the m - k kept pushes plus the box's push distance
/// to its goal, and the m - k kept pushes plus the walks they still need
/// with only the other boxes in the way.
struct Job {
    /// The box's goal cell, the one it ended on, and its heuristic column.
    goal: Cell,
    column: usize,
    /// The box's cell in the start, and the keeper's.
    root: Cell,
    player: Cell,
    /// The length of the route to beat.
    bound: u32,
    /// Boxes but this one.
    stride: usize,
    cells: usize,
    /// The kept pushes, in route order.
    events: Vec<Event>,
    /// The other boxes' cells after each count of kept pushes, `stride`
    /// cells per count from 0 to m, in slot order.
    others: Vec<Cell>,
    /// Layer k: walking distance from each cell to event k's stand with the
    /// other boxes after k events in the way, `NONE` when cut off; `cells`
    /// entries per event.
    walks: Vec<u16>,
    /// tails[k]: the walks events k.. need at the least, each from where
    /// the previous push leaves the keeper to its own stand; tails[m] = 0.
    tails: Vec<u32>,
    /// Each cell's index among the live cells, `NONE` for a cell from which
    /// no push sequence reaches the goal.
    live_of: Vec<u16>,
    live: Vec<Cell>,
    /// Per node id `(k * live + p) * TAGS + tag`: the lowest cost found,
    /// shifted up 3 bits over the parent's tag, `u32::MAX` while unreached.
    best: Vec<u32>,
    heap: BinaryHeap<u64>,
    /// The most entries the queue may grow to within the allowance.
    heap_cap: usize,
    /// Scratch: the boxes a flood treats as walls.
    blocked: Vec<Cell>,
    /// Setup units left: one per layer, from the last, then the root.
    pending: usize,
}

impl Job {
    /// The job for box `slot` of `route`, or `None` when the box never
    /// moves, its goal has no column, the route does not replay, or the
    /// job's buffers and a queue of [`MIN_QUEUE`] entries do not fit in
    /// `allowance` bytes.
    fn new(
        board: &Board,
        heuristic: &Heuristic,
        start: &State,
        route: &[u8],
        slot: usize,
        allowance: u64,
    ) -> Option<Self> {
        let boxes = board.labels().len();
        let cells = board.tiles().len();
        let mut state = *start;
        let (mut own, mut total) = (0, 0);
        for &direction in route {
            if let Step::Push(i) = board.step(&mut state, direction as usize)? {
                total += 1;
                own += usize::from(i == slot);
            }
        }
        if own == 0 {
            return None;
        }
        let goal = state.boxes[slot];
        let column = heuristic
            .goal_cells()
            .iter()
            .position(|&cell| cell == goal)?;
        debug_assert!(heuristic.group(slot).contains(&column));
        let root = start.boxes[slot];
        let live_count = (0..cells)
            .filter(|&cell| heuristic.distance(cell as Cell, column) != NONE)
            .count();
        let m = total - own;
        let stride = boxes - 1;
        let ids = (m + 1) * live_count * TAGS;
        if ids > ID_MASK as usize + 1 || heuristic.distance(root, column) == NONE {
            return None;
        }
        let bytes = |count: usize, size: usize| count as u64 * size as u64;
        let need = bytes(m, size_of::<Event>())
            + bytes((m + 1) * stride, size_of::<Cell>())
            + bytes(m * cells, size_of::<u16>())
            + bytes(m + 1, size_of::<u32>())
            + bytes(cells, size_of::<u16>())
            + bytes(live_count, size_of::<Cell>())
            + bytes(ids, size_of::<u32>())
            + bytes(boxes, size_of::<Cell>());
        // A growing queue briefly holds its old and new buffers.
        let entry = 2 * size_of::<u64>() as u64;
        let heap_cap = usize::try_from(allowance.checked_sub(need)? / entry).ok()?;
        if heap_cap < MIN_QUEUE {
            return None;
        }
        let mut events = Vec::new();
        events.try_reserve_exact(m).ok()?;
        let mut others = Vec::new();
        others.try_reserve_exact((m + 1) * stride).ok()?;
        let mut live_of = filled(cells, NONE).ok()?;
        let mut live = Vec::new();
        live.try_reserve_exact(live_count).ok()?;
        for cell in 0..cells as Cell {
            if heuristic.distance(cell, column) != NONE {
                live_of[cell as usize] = live.len() as u16;
                live.push(cell);
            }
        }
        let mut blocked = Vec::new();
        blocked.try_reserve_exact(boxes).ok()?;
        let snapshot = |state: &State, others: &mut Vec<Cell>| {
            let slots = state.boxes[..boxes].iter().enumerate();
            others.extend(slots.filter(|&(i, _)| i != slot).map(|(_, &cell)| cell));
        };
        let mut state = *start;
        snapshot(&state, &mut others);
        for &direction in route {
            let stand = state.player;
            if let Some(Step::Push(i)) = board.step(&mut state, direction as usize)
                && i != slot
            {
                events.push(Event {
                    stand,
                    from: state.player,
                    to: state.boxes[i],
                    direction,
                });
                snapshot(&state, &mut others);
            }
        }
        Some(Self {
            goal,
            column,
            root,
            player: start.player,
            bound: route.len() as u32,
            stride,
            cells,
            events,
            others,
            walks: filled(m * cells, NONE).ok()?,
            tails: filled(m + 1, 0).ok()?,
            live_of,
            live,
            best: filled(ids, u32::MAX).ok()?,
            heap: BinaryHeap::new(),
            heap_cap,
            blocked,
            pending: m + 1,
        })
    }

    /// Runs one unit: a setup step, or one pop. Each unit floods for
    /// itself, so no flood outlives it.
    fn step(&mut self, board: &Board, heuristic: &Heuristic, reach: &mut Reach) -> Progress {
        if self.pending > 0 {
            self.pending -= 1;
            return self.prepare(board, heuristic, reach);
        }
        let Some(key) = self.heap.pop() else {
            return Progress::Failed;
        };
        let id = (key & ID_MASK) as usize;
        let f = F_MASK - (key >> (F_BITS + ID_BITS));
        let h = F_MASK - ((key >> ID_BITS) & F_MASK);
        let g = (f - h) as u32;
        // Stale: a cheaper path to the node was queued since.
        if self.best[id] >> 3 != g {
            return Progress::Running;
        }
        let (k, li, tag) = self.split(id);
        let p = self.live[li];
        let m = self.events.len();
        if k == m && p == self.goal {
            return self.rebuild(board, reach, id, g);
        }
        let q = self.keeper(board, k, p, tag);
        self.flood(board, reach, k, p, q);
        if k < m {
            let event = self.events[k];
            let walk = reach.distance(event.stand);
            if event.to != p
                && walk != NONE
                && let Some(h) = self.estimate(heuristic, k + 1, p, event.from)
                && !self.relax(k + 1, li, 0, g + u32::from(walk) + 1, h, tag)
            {
                return Progress::Failed;
            }
        }
        for d in 0..4 {
            let to = board.neighbors()[p as usize][d];
            let stand = board.neighbors()[p as usize][OPPOSITE[d]];
            if to == NONE || stand == NONE || self.live_of[to as usize] == NONE || reach.blocked(to)
            {
                continue;
            }
            let walk = reach.distance(stand);
            if walk == NONE {
                continue;
            }
            let Some(h) = self.estimate(heuristic, k, to, p) else {
                continue;
            };
            let li = self.live_of[to as usize] as usize;
            if !self.relax(k, li, d + 1, g + u32::from(walk) + 1, h, tag) {
                return Progress::Failed;
            }
        }
        Progress::Running
    }

    /// One setup unit: builds the walk layer `pending - 1`, from the last
    /// layer down so each tail can add the one after it, then queues the
    /// root once every layer is built.
    fn prepare(&mut self, board: &Board, heuristic: &Heuristic, reach: &mut Reach) -> Progress {
        if self.pending == 0 {
            let li = self.live_of[self.root as usize] as usize;
            return match self.estimate(heuristic, 0, self.root, self.player) {
                Some(h) if self.relax(0, li, 0, 0, h, 0) => Progress::Running,
                _ => Progress::Failed,
            };
        }
        let k = self.pending - 1;
        let event = self.events[k];
        reach.fill_from(board, event.stand, self.others(k));
        let entry = self.entry(k);
        let layer = &mut self.walks[k * self.cells..(k + 1) * self.cells];
        for &cell in reach.reached_cells() {
            layer[cell as usize] = reach.distance(cell);
        }
        // The route walked here, so this is never NONE; the guard keeps a
        // broken route from adding NONE to the tails.
        let walk = layer[entry as usize];
        if walk == NONE {
            return Progress::Failed;
        }
        self.tails[k] = self.tails[k + 1] + u32::from(walk);
        Progress::Running
    }

    /// Where the keeper stands when layer `k` begins: the start's cell, or
    /// where kept push k - 1 left it.
    fn entry(&self, k: usize) -> Cell {
        if k == 0 {
            self.player
        } else {
            self.events[k - 1].from
        }
    }

    /// The keeper's cell at node (k, p, tag).
    fn keeper(&self, board: &Board, k: usize, p: Cell, tag: usize) -> Cell {
        if tag == 0 {
            self.entry(k)
        } else {
            board.neighbors()[p as usize][OPPOSITE[tag - 1]]
        }
    }

    /// The other boxes' cells after `k` kept pushes.
    fn others(&self, k: usize) -> &[Cell] {
        &self.others[k * self.stride..(k + 1) * self.stride]
    }

    /// Floods from `keeper` with the other boxes after `k` kept pushes and
    /// this box, on `p`, in the way.
    fn flood(&mut self, board: &Board, reach: &mut Reach, k: usize, p: Cell, keeper: Cell) {
        self.blocked.clear();
        self.blocked
            .extend_from_slice(&self.others[k * self.stride..(k + 1) * self.stride]);
        self.blocked.push(p);
        reach.fill_from(board, keeper, &self.blocked);
    }

    /// The node id's layer, live-cell index and tag.
    fn split(&self, id: usize) -> (usize, usize, usize) {
        let at = id / TAGS;
        (at / self.live.len(), at % self.live.len(), id % TAGS)
    }

    /// A lower bound on the moves left from layer `k` with the box on `p`
    /// and the keeper on `q`, or `None` when the keeper can no longer reach
    /// the next kept push's stand.
    fn estimate(&self, heuristic: &Heuristic, k: usize, p: Cell, q: Cell) -> Option<u32> {
        let m = self.events.len();
        let kept = (m - k) as u32;
        let pushes = u32::from(heuristic.distance(p, self.column));
        if k == m {
            return Some(kept + pushes);
        }
        let walk = self.walks[k * self.cells + q as usize];
        if walk == NONE {
            return None;
        }
        Some(kept + pushes.max(u32::from(walk) + self.tails[k + 1]))
    }

    /// Records cost `g` for node (k, li, tag), reached from a node tagged
    /// `parent`, and queues it, unless `g + h` cannot beat the bound or the
    /// node already has a cost no higher. False when the queue could not
    /// grow within the allowance, which ends the job.
    fn relax(&mut self, k: usize, li: usize, tag: usize, g: u32, h: u32, parent: usize) -> bool {
        if g + h >= self.bound {
            return true;
        }
        let id = (k * self.live.len() + li) * TAGS + tag;
        if self.best[id] >> 3 <= g {
            return true;
        }
        let capacity = self.heap.capacity();
        if self.heap.len() == capacity {
            let extra = capacity.max(64).min(self.heap_cap.saturating_sub(capacity));
            if extra == 0 || self.heap.try_reserve_exact(extra).is_err() {
                return false;
            }
        }
        self.best[id] = (g << 3) | parent as u32;
        self.heap.push(key(g + h, h, id));
        true
    }

    /// The route to goal node `id`, of cost `g`: each step back re-floods
    /// as its expansion did and appends the push, then the walk before it.
    fn rebuild(&mut self, board: &Board, reach: &mut Reach, mut id: usize, g: u32) -> Progress {
        let mut route = Vec::new();
        if route.try_reserve_exact(g as usize).is_err() {
            return Progress::Failed;
        }
        loop {
            let (k, li, tag) = self.split(id);
            if k == 0 && tag == 0 {
                break;
            }
            let parent = (self.best[id] & 7) as usize;
            let p = self.live[li];
            let lives = self.live.len();
            if tag == 0 {
                let event = self.events[k - 1];
                route.push(event.direction);
                let q = self.keeper(board, k - 1, p, parent);
                self.flood(board, reach, k - 1, p, q);
                reach.append_walk_reversed(board, event.stand, &mut route);
                id = ((k - 1) * lives + li) * TAGS + parent;
            } else {
                let d = tag - 1;
                route.push(d as u8);
                let from = board.neighbors()[p as usize][OPPOSITE[d]];
                let q = self.keeper(board, k, from, parent);
                self.flood(board, reach, k, from, q);
                let stand = board.neighbors()[from as usize][OPPOSITE[d]];
                reach.append_walk_reversed(board, stand, &mut route);
                id = (k * lives + self.live_of[from as usize] as usize) * TAGS + parent;
            }
        }
        route.reverse();
        debug_assert_eq!(route.len(), g as usize);
        Progress::Found(route)
    }
}
