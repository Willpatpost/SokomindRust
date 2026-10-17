//! Route polish: once a Quality search ends with a route, or as an episode
//! while a long one waits, shortens it by rescheduling one box, or two, at
//! a time. A job keeps every other box's
//! pushes, in their order, and runs A* over where the freed boxes' own
//! pushes go among them: from the start to the goal cells they ended on,
//! either way round when the two share a label, with every walk taken as
//! short as the boxes then allow. Its bound is the current route, so
//! anything it finds is shorter; a found route is replayed before it is
//! kept. Single-box jobs cycle over the boxes until a full pass gains
//! nothing. Where that pass ends depends on the order the boxes are visited
//! in, so the polish makes several runs from the search's route, each in
//! its own order, and keeps the shortest route. Pair jobs then take the
//! best route on, single-box jobs running again after each gain, until a
//! full pass over the pairs gains nothing. The end polish then kicks the
//! route out of that local optimum: two boxes of one label trade goals,
//! under a bound a little above the best route, and the single-box and pair
//! jobs descend from the kicked route; a full round of kicks that leaves
//! the best route as it was ends the polish, and so does the kicks' work
//! running out, [`KICK_WORK`] units for each state of the search's state
//! limit. The design, its soundness and its memory budget are in
//! docs/route-polish.md.
use crate::{
    MAX_STATES, SolutionError, Status, arena::mix, heuristic::Heuristic, reach::Reach,
    stage::filled,
};
use sokomind_core::{Board, Cell, MAX_ROUTE, NONE, OPPOSITE, State, Step};
use std::{collections::BinaryHeap, mem::size_of};
#[cfg(not(target_arch = "wasm32"))]
use std::{
    collections::VecDeque,
    sync::{Arc, mpsc},
};

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

/// The most runs a polish makes. A board with few boxes has fewer box
/// orders, and makes one run per order. `SOKOMIND_POLISH_RUNS` raises the
/// cap for a native experiment (unset, or on wasm, the default stands).
/// The runs phase keeps the best route any run reaches, but the pair and
/// kick phases after it work from that route, so a raised cap can change
/// their track and with it the polish's answer, for better or worse: on
/// Grand Hall the default reaches 424 moves and every order 432.
const RUNS: usize = 8;
/// The most expansions one pair job makes. A pair's nodes multiply by the
/// second box's cells, so a pair job could otherwise run on a large board
/// for far longer than every single-box job together. On Grand Hall every
/// pair job ends within it, found or exhausted (docs/route-polish.md).
const PAIR_POPS: usize = 1 << 20;
/// How much longer than the best route a kick's route may be. On Grand
/// Hall, 22 of the 55 kicks from 436 moves found a route within 436 + 64,
/// of 436 to 482 moves, and 9 of those led to a route below 436
/// (docs/route-polish.md).
const KICK_SLACK: u32 = 64;
/// The kick phase's polish units for each state of the search's state
/// limit, so that with no time limit the kicks cost a set multiple of the
/// search they follow. Uncapped, Grand Hall's kicks at 60M states (64 MiB)
/// shorten the route from 436 to 434 moves after 18.2M units, to 429 after
/// 308M, and end after 502M: 16 per state, 960M, leaves them uncapped
/// there, while 1 per state stops them at 432. At the gate's 20,000 states
/// the catalog keeps 6 of the 10 shorter Quality routes uncapped kicks find
/// (5,163 moves in all, against 5,182 without kicks and 5,107 uncapped),
/// and at bench:observe's 1M states Grand Hall's 16M units end before its
/// first gain, so it stays at 436 (job 49509, docs/route-polish.md).
const KICK_WORK: usize = 16;
// The most kick work fits wasm32's usize, so WASM caps the kicks where
// native does.
const _: () = assert!(MAX_STATES * KICK_WORK <= u32::MAX as usize);

/// The strides that visit each of `boxes` slots once per cycle: those
/// coprime to the box count.
fn strides(boxes: usize) -> impl Iterator<Item = usize> {
    (1..=boxes).filter(move |&stride| gcd(stride, boxes) == 1)
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The run cap for `boxes` slots: every order, clamped to the cap
/// [`RUNS`] names — or, on a native experiment, to the higher one
/// `SOKOMIND_POLISH_RUNS` names.
fn run_cap(boxes: usize) -> usize {
    (boxes * strides(boxes).count()).clamp(1, native_run_cap())
}

/// [`RUNS`], or the higher cap `SOKOMIND_POLISH_RUNS` names.
#[cfg(not(target_arch = "wasm32"))]
fn native_run_cap() -> usize {
    std::env::var("SOKOMIND_POLISH_RUNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(RUNS, |asked| asked.max(RUNS))
}

/// [`RUNS`]: wasm has no environment to name a higher cap.
#[cfg(target_arch = "wasm32")]
fn native_run_cap() -> usize {
    RUNS
}

/// Run `run`'s box order, as its first slot and the stride between slots.
/// Run 0 takes the slots in order. Later runs take the coprime strides in
/// turn, then move the first slot on, so the first `boxes * φ(boxes)` runs
/// all differ.
fn order(boxes: usize, run: usize) -> (usize, usize) {
    let count = strides(boxes).count();
    let stride = strides(boxes).nth(run % count).unwrap_or(1);
    (run / count % boxes, stride)
}

/// Whether the boxes in slots `pair` share a label, so either may end on
/// either one's goal.
fn same_label(heuristic: &Heuristic, [a, b]: [usize; 2]) -> bool {
    heuristic.group(a).contains(&b)
}

/// Every box pair, in the order the pair phase frees them: those whose
/// boxes end `route` nearest each other first, by Manhattan distance, ties
/// in slot order. Boxes that end close are the likeliest to gain from trading
/// order or goals. Empty when the allocator refuses.
fn pairs(board: &Board, start: &State, route: &[u8]) -> Vec<[usize; 2]> {
    let boxes = board.labels().len();
    let mut end = *start;
    for &direction in route {
        let _ = board.step(&mut end, direction as usize);
    }
    let width = board.width();
    let apart = |[a, b]: [usize; 2]| {
        let (a, b) = (usize::from(end.boxes[a]), usize::from(end.boxes[b]));
        (a / width).abs_diff(b / width) + (a % width).abs_diff(b % width)
    };
    let mut pairs = Vec::new();
    if pairs
        .try_reserve_exact(boxes * boxes.saturating_sub(1) / 2)
        .is_err()
    {
        return pairs;
    }
    pairs.extend((0..boxes).flat_map(|a| (a + 1..boxes).map(move |b| [a, b])));
    pairs.sort_unstable_by_key(|&pair| (apart(pair), pair));
    pairs
}

/// How many threads the polish's kick round races on, from
/// `SOKOMIND_POLISH_WORKERS`. Unset, or on wasm, means 1: the
/// sequential path exactly. Native only: wasm has no environment to
/// read and stays single-threaded.
#[cfg(not(target_arch = "wasm32"))]
fn workers() -> usize {
    std::env::var("SOKOMIND_POLISH_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&workers| workers > 1)
        .unwrap_or(1)
}

/// [`workers()`] on wasm: no environment, so no pool.
#[cfg(target_arch = "wasm32")]
fn workers() -> usize {
    1
}

/// A fixed pool of worker threads for the polish's kick rounds,
/// native only: wasm's polish stays single-threaded, and the pool is
/// what `workers() > 1` builds. Tasks are closures that run to
/// completion and report through the sender they capture; the driver
/// polls the receiving end, so no step of the polish ever blocks on
/// the pool. The pool outlives the rounds it serves — a task may
/// hold only what it owns or shares through an `Arc` — and ends when
/// the polish does, dropping its queue and joining its threads.
#[cfg(not(target_arch = "wasm32"))]
struct Pool {
    queue: Arc<PoolQueue>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// A queued task: a closure that runs to completion on its worker.
#[cfg(not(target_arch = "wasm32"))]
type Task = Box<dyn FnOnce() + Send>;

/// The pool's shared task queue: the tasks waiting, and whether the
/// pool is ending, woken by a condition variable.
#[cfg(not(target_arch = "wasm32"))]
struct PoolQueue {
    tasks: std::sync::Mutex<(VecDeque<Task>, bool)>,
    wake: std::sync::Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
impl Pool {
    /// A pool of `size` threads, `None` when fewer than two were
    /// asked for: one thread is no pool.
    fn new(size: usize) -> Option<Self> {
        if size <= 1 {
            return None;
        }
        let queue = Arc::new(PoolQueue {
            tasks: std::sync::Mutex::new((VecDeque::new(), false)),
            wake: std::sync::Condvar::new(),
        });
        let mut threads = Vec::new();
        for _ in 0..size {
            let queue = Arc::clone(&queue);
            threads.push(std::thread::spawn(move || {
                loop {
                    let task = {
                        let mut guard = queue.tasks.lock().unwrap();
                        loop {
                            if let Some(task) = guard.0.pop_front() {
                                break task;
                            }
                            if guard.1 {
                                return;
                            }
                            guard = queue.wake.wait(guard).unwrap();
                        }
                    };
                    task();
                }
            }));
        }
        Some(Self { queue, threads })
    }

    /// Queues `task` for the next idle worker.
    fn submit(&self, task: Box<dyn FnOnce() + Send>) {
        self.queue.tasks.lock().unwrap().0.push_back(task);
        self.queue.wake.notify_one();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Pool {
    /// Wakes every worker to end, then joins them; a running task
    /// finishes first, so a drop waits out the round's longest kick.
    fn drop(&mut self) {
        self.queue.tasks.lock().unwrap().1 = true;
        self.queue.wake.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// One kick trial's report: its place in the round's dispatch order,
/// and the route the trial descended to with the units it ran — no
/// route when the kick could not start or found nothing to descend.
#[cfg(not(target_arch = "wasm32"))]
struct KickResult {
    order: usize,
    route: Option<Vec<u8>>,
    units: u64,
}

/// One kick round in flight on the pool: the completions still to
/// come, those that arrived, the round's pairs as indices into the
/// pair list (so an improvement restarts the next round after its
/// winner), and the pool the trials run on, which joins its workers
/// when the round goes.
#[cfg(not(target_arch = "wasm32"))]
struct Round {
    done: mpsc::Receiver<KickResult>,
    kicked: Vec<usize>,
    /// Held for its drop: ending the round joins the pool's workers.
    #[expect(dead_code)]
    pool: Pool,
    pending: usize,
    arrived: Vec<KickResult>,
}

/// Runs `job` to its end, one unit per step, on `reach`: the route it
/// found, cut at its first solved state, with the units it ran, or no
/// route with the units a failed or unreplayable job spent. A task's
/// whole job, where [`Polish::step`] slices one unit per call.
fn run_job(
    board: &Board,
    heuristic: &Heuristic,
    start: &State,
    job: &mut Job,
    reach: &mut Reach,
) -> (Option<Vec<u8>>, u64) {
    let mut units = 0;
    loop {
        units += 1;
        match job.step(board, heuristic, reach) {
            Progress::Running => {}
            Progress::Failed => return (None, units),
            Progress::Found(route) => {
                let Ok(n) = replay(board, start, &route) else {
                    return (None, units);
                };
                let mut route = route;
                route.truncate(n);
                return (Some(route), units);
            }
        }
    }
}

/// The descent a kicked route gets: every box's single-box job in turn,
/// from the route each gain leaves, until a full cycle over the boxes
/// gains nothing, then every pair's job the same way, until a full pass
/// over the pairs gains nothing — the polish's own spine, run to its
/// end on one route instead of sliced through [`Polish::step`]. Each
/// job keeps every other box's pushes in their order, so only a
/// shorter route that replays can replace the working one. Returns the
/// converged route and the units the descent ran, gains and failures
/// alike.
fn descend(
    board: &Board,
    heuristic: &Heuristic,
    start: &State,
    mut route: Vec<u8>,
    allowance: u64,
    reach: &mut Reach,
) -> (Vec<u8>, u64) {
    let boxes = board.labels().len();
    let mut units = 0;
    // The single-box cycle, in slot order, from wherever the last job
    // left it: a gain counts its own job and restarts the cycle.
    let (mut next, mut idle) = (0, 0);
    // The pair pass, over the pairs of the route the cycle converged
    // on, computed once: a pair's gain sends the box cycle back to
    // work before the next pair runs.
    let (mut pair_list, mut pair, mut pair_idle, mut in_pairs) = (Vec::new(), 0, 0, false);
    loop {
        if idle < boxes {
            let slot = next;
            next = (slot + 1) % boxes;
            idle += 1;
            let Some(mut job) = Job::new(board, heuristic, start, &route, &[slot], allowance)
            else {
                continue;
            };
            let (found, ran) = run_job(board, heuristic, start, &mut job, reach);
            units += ran;
            if let Some(found) = found
                && found.len() < route.len()
            {
                route = found;
                idle = 1;
            }
            continue;
        }
        if !in_pairs {
            in_pairs = true;
            pair_list = pairs(board, start, &route);
        }
        if pair_idle < pair_list.len() {
            let slots = pair_list[pair];
            pair = (pair + 1) % pair_list.len();
            pair_idle += 1;
            let Some(mut job) = Job::new(board, heuristic, start, &route, &slots, allowance) else {
                continue;
            };
            let (found, ran) = run_job(board, heuristic, start, &mut job, reach);
            units += ran;
            if let Some(found) = found
                && found.len() < route.len()
            {
                route = found;
                (idle, pair_idle) = (0, 1);
            }
            continue;
        }
        break;
    }
    (route, units)
}

/// One kick trial for the parallel round: the same-label `pair` of
/// `best` made to trade goals under the kick's slack bound, and the
/// kicked route then descended. `None` when the kick cannot start or
/// finds no route to descend; otherwise the descended route and the
/// trial's units — an improvement only when the descent ends shorter
/// than `best`, which the round's merge decides.
fn kick_trial(
    board: &Board,
    heuristic: &Heuristic,
    start: &State,
    best: &[u8],
    pair: [usize; 2],
    allowance: u64,
) -> Option<(Vec<u8>, u64)> {
    let mut job = Job::kick(board, heuristic, start, best, pair, allowance)?;
    let mut reach = Reach::new(board.tiles().len());
    let (kicked, units) = run_job(board, heuristic, start, &mut job, &mut reach);
    let kicked = kicked?;
    let (descended, ran) = descend(board, heuristic, start, kicked, allowance, &mut reach);
    Some((descended, units + ran))
}

/// The polisher: the search's route, the best route so far, the status the
/// search ended with, and the run or pair phase and the job under way. One
/// [`Polish::step`] is one unit of a job, so the engine slices it by pops
/// like the search.
pub(crate) struct Polish {
    /// The route the search handed over, which every run starts from.
    seed: Vec<u8>,
    /// The shortest route so far.
    best: Vec<u8>,
    /// The route the jobs work on. Every route held is direction indices,
    /// replay-verified to reach the first solved state at its last move.
    route: Vec<u8>,
    ending: Status,
    /// Bytes a job may hold: what the engine gives the polish, less the
    /// seed, the best route and the pair list. That is the arena's share of
    /// the budget once the search has ended and released the arena, or the
    /// part of the share the arena does not hold while the search waits.
    allowance: u64,
    /// The run under way, from 0, and how many the polish makes. Once the
    /// last run has ended, `run` is `runs`: the pair phase.
    run: usize,
    runs: usize,
    /// The box slot the next single-box job reschedules, and the stride to
    /// the one after it.
    next: usize,
    stride: usize,
    /// Single-box jobs started since the route last got shorter, counting
    /// one that shortened it. A full cycle over the boxes ends a run, and in
    /// the pair phase lets the next pair job start.
    idle: usize,
    /// The pair phase's box pairs, in job order, the index of the next one,
    /// and the pair jobs started since the route last got shorter, counting
    /// one that shortened it. A full pass over the pairs ends the polish.
    pairs: Vec<[usize; 2]>,
    pair: usize,
    pair_idle: usize,
    /// The kick phase, once a full pass over the pairs has gained nothing:
    /// how many of the pairs share a label (none in an episode), the index
    /// in `pairs` to look for the next one to kick from, and the kicks
    /// started since the best route last got shorter. A full round of
    /// kicks ends the polish.
    swaps: usize,
    kick: usize,
    kick_idle: usize,
    /// Units the kick phase may still run, the kicks and the jobs that
    /// descend from them, and whether it has begun. Every unit after the
    /// first kick starts takes one, and the polish ends when none is left.
    kick_work: usize,
    kicking: bool,
    job: Option<Job>,
    /// The kick round in flight on the pool, when workers race the
    /// kicks: `None` on the sequential path, between rounds, and on
    /// wasm.
    #[cfg(not(target_arch = "wasm32"))]
    round: Option<Round>,
}

impl Polish {
    /// The polisher for `route`, which the search ended with `ending`, or
    /// [`Status::Running`] for an episode. Its jobs may hold `allowance`
    /// bytes less the routes it keeps, and its kicks run [`KICK_WORK`]
    /// units for each of the search's `max_states`, which an episode, never
    /// kicking, gives as 0.
    pub(crate) fn new(
        route: Vec<u8>,
        ending: Status,
        allowance: u64,
        boxes: usize,
        max_states: usize,
    ) -> Self {
        // The seed and the best route are each at most the route's length.
        let held = 2 * route.len() as u64;
        Self {
            seed: route.clone(),
            best: route.clone(),
            route,
            ending,
            allowance: allowance.saturating_sub(held),
            run: 0,
            runs: run_cap(boxes),
            next: 0,
            stride: 1,
            idle: 0,
            pairs: Vec::new(),
            pair: 0,
            pair_idle: 0,
            swaps: 0,
            kick: 0,
            kick_idle: 0,
            kick_work: max_states.saturating_mul(KICK_WORK),
            kicking: false,
            job: None,
            #[cfg(not(target_arch = "wasm32"))]
            round: None,
        }
    }
    /// Whether a polish of a route of `moves` moves can start any job
    /// within `allowance` bytes. When false, every job is refused, since
    /// each holds a store of [`Nodes::MIN`] nodes and a queue of
    /// [`MIN_QUEUE`] entries beside the seed and the best route; when true,
    /// a job may still be refused for its own buffers.
    pub(crate) fn fits(allowance: u64, moves: u32) -> bool {
        allowance >= 2 * u64::from(moves) + Nodes::bytes(Nodes::MIN) + MIN_QUEUE as u64 * ENTRY
    }
    pub(crate) fn moves(&self) -> u32 {
        self.best.len() as u32
    }
    pub(crate) fn best(&self) -> &[u8] {
        &self.best
    }
    /// The best route, the polish done with.
    pub(crate) fn into_best(self) -> Vec<u8> {
        self.best
    }
    /// The status the search ended with before polishing, or
    /// [`Status::Running`] when the search has not ended and goes on once
    /// the polish does.
    pub(crate) fn ending(&self) -> Status {
        self.ending
    }
    /// Drops the running job; the best route stays.
    pub(crate) fn stop(&mut self) {
        self.job = None;
    }
    /// Runs one unit: starts the next run from the seed, starts the pair
    /// phase on the best route, starts the next job or kick, or advances
    /// the running one. A job that finds a shorter route ends, and its
    /// route replaces the one the jobs work on once replay confirms it, cut
    /// at its first solved state; a kick's route replaces it even when
    /// longer. True once every run has ended, a full pass over the pairs
    /// has gained nothing, and then a full round of kicks has not shortened
    /// the best route or the kick work has run out; every later call
    /// returns true again.
    pub(crate) fn step(
        &mut self,
        board: &Board,
        heuristic: &Heuristic,
        start: &State,
        reach: &mut Reach,
    ) -> bool {
        // A kick round in flight on the pool: poll one completion, and
        // merge the round when the last is in. A poll that finds nothing
        // spends no kick unit — the wait is not work.
        #[cfg(not(target_arch = "wasm32"))]
        if self.round.is_some() {
            return self.poll_round();
        }
        if self.kicking {
            let Some(left) = self.kick_work.checked_sub(1) else {
                self.job = None;
                return true;
            };
            self.kick_work = left;
        }
        let boxes = board.labels().len();
        let Some(job) = &mut self.job else {
            // A job that cannot start, because its boxes never move or it
            // does not fit the allowance, counts as idle.
            if self.idle < boxes {
                self.idle += 1;
                let slot = self.next;
                self.next = (slot + self.stride) % boxes;
                self.job = Job::new(
                    board,
                    heuristic,
                    start,
                    &self.route,
                    &[slot],
                    self.allowance,
                );
            } else if self.run + 1 < self.runs {
                self.run += 1;
                self.route.clone_from(&self.seed);
                (self.next, self.stride) = order(boxes, self.run);
                self.idle = 0;
            } else if self.run < self.runs {
                // The best route is where the run that found it ended, so
                // no single-box job gains on it until a pair job does.
                self.run = self.runs;
                self.route.clone_from(&self.best);
                self.pairs = pairs(board, start, &self.best);
                let held = self.pairs.capacity() * size_of::<[usize; 2]>();
                self.allowance = self.allowance.saturating_sub(held as u64);
                (self.next, self.stride) = (0, 1);
                self.pair_idle = 0;
                // An episode hands back to the search once the pairs gain
                // nothing; only the end polish goes on to kick.
                if self.ending != Status::Running {
                    let shared = self
                        .pairs
                        .iter()
                        .filter(|&&pair| same_label(heuristic, pair));
                    self.swaps = shared.count();
                }
            } else if self.pair_idle < self.pairs.len() {
                self.pair_idle += 1;
                let pair = self.pairs[self.pair];
                self.pair = (self.pair + 1) % self.pairs.len();
                self.job = Job::new(board, heuristic, start, &self.route, &pair, self.allowance);
            } else if self.kick_idle < self.swaps {
                // No single-box or pair job gains on the route: a local
                // optimum. A kick makes the next same-label pair trade
                // goals in the best route, and the jobs then descend from
                // the kicked route as they did from the runs' best.
                self.kick_idle += 1;
                self.kicking = true;
                // The workers race the whole round at once; the
                // sequential path below takes the kicks one at a time.
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(round) = self.start_round(board, heuristic, start) {
                    self.round = Some(round);
                    return false;
                }
                let pair = loop {
                    let pair = self.pairs[self.kick];
                    self.kick = (self.kick + 1) % self.pairs.len();
                    if same_label(heuristic, pair) {
                        break pair;
                    }
                };
                self.route.clone_from(&self.best);
                self.job = Job::kick(board, heuristic, start, &self.best, pair, self.allowance);
            } else {
                return true;
            }
            return false;
        };
        let (freed, kick) = (job.freed, job.kick);
        match job.step(board, heuristic, reach) {
            Progress::Running => {}
            Progress::Failed => self.job = None,
            Progress::Found(mut route) => {
                self.job = None;
                let solved_at = replay(board, start, &route);
                // A route can solve the board before its end when a box
                // reaches its goal while another box is briefly off its own.
                debug_assert!(
                    solved_at.is_ok_and(|n| n <= route.len() && (kick || n < self.route.len())),
                    "{solved_at:?}"
                );
                if let Ok(n) = solved_at
                    && (kick || n < self.route.len())
                {
                    route.truncate(n);
                    self.route = route;
                    // Every job may gain on the new route. After a pair's
                    // gain every single-box job runs before the next pair,
                    // and after a kick before every pair.
                    (self.idle, self.pair_idle) = match (freed, kick) {
                        (1, _) => (1, 0),
                        (_, false) => (0, 1),
                        (_, true) => (0, 0),
                    };
                    if self.route.len() < self.best.len() {
                        self.best.clone_from(&self.route);
                        self.kick_idle = 0;
                    }
                }
            }
        }
        false
    }

    /// Dispatches the next kick round on the pool: every same-label
    /// pair the round would visit, from the current kick index and in
    /// the sequential visit order, each a whole kick trial on its own
    /// thread's scratch. `None` on the sequential path — fewer than
    /// two workers, or no pairs to kick — which takes the kicks one
    /// at a time instead.
    #[cfg(not(target_arch = "wasm32"))]
    fn start_round(
        &mut self,
        board: &Board,
        heuristic: &Heuristic,
        start: &State,
    ) -> Option<Round> {
        let workers = workers();
        if workers <= 1 || self.swaps == 0 {
            return None;
        }
        let pool = Pool::new(workers)?;
        // The round's pairs: the same-label pairs from the kick index,
        // wrapping, a full cycle of the round — the sequential visit
        // order, recorded as indices into the pair list so an
        // improvement restarts the next round after its winner.
        let mut kicked = Vec::with_capacity(self.swaps);
        let mut at = self.kick;
        while kicked.len() < self.swaps {
            let index = at;
            at = (at + 1) % self.pairs.len();
            if same_label(heuristic, self.pairs[index]) {
                kicked.push(index);
            }
        }
        self.kick = at;
        let (send, done) = mpsc::channel();
        // The trials share one board clone and one fresh heuristic:
        // only the weighted policies polish, so the heuristic never
        // counts keeper sides and is cheap to build.
        let board = Arc::new(board.clone());
        let heuristic = Arc::new(Heuristic::new(&board, None));
        let best = Arc::new(self.best.clone());
        let (state, allowance) = (*start, self.allowance);
        for (order, &index) in kicked.iter().enumerate() {
            let pair = self.pairs[index];
            let (send, board) = (send.clone(), Arc::clone(&board));
            let (heuristic, best) = (Arc::clone(&heuristic), Arc::clone(&best));
            pool.submit(Box::new(move || {
                let (route, units) =
                    match kick_trial(&board, &heuristic, &state, &best, pair, allowance) {
                        Some((route, units)) => (Some(route), units),
                        None => (None, 0),
                    };
                let _ = send.send(KickResult {
                    order,
                    route,
                    units,
                });
            }));
        }
        let pending = kicked.len();
        Some(Round {
            done,
            kicked,
            pool,
            pending,
            arrived: Vec::new(),
        })
    }

    /// Polls one completion of the round in flight: a result buffers,
    /// and the last one merges the round. A poll that finds nothing
    /// spends no kick unit — the wait is not work. True once a merged
    /// round ends the polish.
    #[cfg(not(target_arch = "wasm32"))]
    fn poll_round(&mut self) -> bool {
        let Some(mut round) = self.round.take() else {
            return false;
        };
        match round.done.try_recv() {
            Ok(result) => {
                round.pending -= 1;
                round.arrived.push(result);
                if round.pending == 0 {
                    return self.merge_round(round);
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            // Every sender is gone: whatever arrived is the round.
            Err(mpsc::TryRecvError::Disconnected) => return self.merge_round(round),
        }
        self.round = Some(round);
        false
    }

    /// Merges a finished round: the first improver in dispatch order —
    /// the one the sequential path would have found first — becomes the
    /// best route, the next round starting after its pair, and the
    /// budget charges the trials the sequential path would have run to
    /// reach the same place. A round nothing improved, or a charge
    /// that empties the budget, ends the polish.
    #[cfg(not(target_arch = "wasm32"))]
    fn merge_round(&mut self, mut round: Round) -> bool {
        round.arrived.sort_by_key(|result| result.order);
        let winner = round.arrived.iter().position(|result| {
            result
                .route
                .as_ref()
                .is_some_and(|route| route.len() < self.best.len())
        });
        // The budget charges what the sequential path would have spent
        // to reach the same place: every trial up to and including the
        // first improver, or the whole round when nothing improved. The
        // trials past the improver ran from the base the improvement
        // replaced, so the sequential path never runs them — they are
        // the round's speculation, spent in wall time but not budget.
        let charge = match winner {
            Some(at) => round.arrived[..=at]
                .iter()
                .fold(0u64, |sum, result| sum.saturating_add(result.units)),
            None => round
                .arrived
                .iter()
                .fold(0u64, |sum, result| sum.saturating_add(result.units)),
        };
        let spent = (charge as usize).min(self.kick_work);
        self.kick_work -= spent;
        if let Some(at) = winner
            && let Some(route) = round.arrived[at].route.take()
        {
            self.route = route;
            self.best.clone_from(&self.route);
            self.kick_idle = 0;
            self.kick = (round.kicked[at] + 1) % self.pairs.len();
        } else {
            // The full round left the best as it was.
            self.kick_idle = self.swaps;
        }
        self.kick_work == 0 || self.kick_idle >= self.swaps
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
    /// The job ended with nothing shorter, or could not grow its store or
    /// queue, or used up its expansions.
    Failed,
    /// A route shorter than the bound, as direction indices.
    Found(Vec<u8>),
}

// Queue keys pack, from the top: the bit-flipped f, the bit-flipped h, and
// the node id. `BinaryHeap` is a max-heap, so it pops the lowest f, then the
// lowest h (the deepest node), then the highest id, the node reached last:
// one fixed order, so a job runs the same however its units are sliced.
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

/// A queue entry's bytes.
const ENTRY: u64 = size_of::<u64>() as u64;
/// The fewest entries a job's queue starts with, below which the job is not
/// worth starting.
const MIN_QUEUE: usize = 1024;

/// The most boxes one job frees.
const FREED: usize = 2;
/// A node's tags: 0 when it was reached by another box's push, or is the
/// root, else `1 + 4i + d` after freed box `i`'s own push in direction `d`.
/// The tag and the freed boxes' cells fix the keeper's cell.
const TAGS: usize = 1 + 4 * FREED;
const TAG_BITS: u32 = 4;
const TAG_MASK: u32 = (1 << TAG_BITS) - 1;
const K_SHIFT: u32 = 2 * Cell::BITS + TAG_BITS;
const _: () = assert!(TAGS <= 1 << TAG_BITS);
// A node's k is at most the route's pushes, and a cost below the bound, so
// both fit above their shifts.
const _: () = assert!((MAX_ROUTE as u64) < 1 << (u64::BITS - K_SHIFT));
const _: () = assert!((MAX_ROUTE as u64) < 1 << (u32::BITS - TAG_BITS));

/// A job's node: `k` of the kept pushes done, the freed boxes on `at` (the
/// second cell `NONE` in a single-box job), and the keeper fixed by `tag`.
#[derive(Clone, Copy)]
struct Node {
    k: usize,
    at: [Cell; FREED],
    tag: usize,
}

impl Node {
    /// The node as one key: k, then each cell, then the tag.
    fn pack(self) -> u64 {
        ((self.k as u64) << K_SHIFT)
            | (u64::from(self.at[0]) << (Cell::BITS + TAG_BITS))
            | (u64::from(self.at[1]) << TAG_BITS)
            | self.tag as u64
    }
    fn unpack(key: u64) -> Self {
        Self {
            k: (key >> K_SHIFT) as usize,
            at: [
                (key >> (Cell::BITS + TAG_BITS)) as Cell,
                (key >> TAG_BITS) as Cell,
            ],
            tag: (key as u32 & TAG_MASK) as usize,
        }
    }
}

/// The nodes a job has reached, by id in the order first reached, and an
/// open-addressed table from a node's key to its id. Ids never move when
/// the table grows, so the queue holds them in its keys' low bits.
struct Nodes {
    /// Per id: the node's key.
    keys: Vec<u64>,
    /// Per id: the lowest cost found, shifted up [`TAG_BITS`] over the tag
    /// of the node it was reached from.
    costs: Vec<u32>,
    /// Per slot: an id plus 1, or 0 when empty. A power of two of slots,
    /// at most half of them full.
    slots: Vec<u32>,
}

impl Nodes {
    /// The nodes a new store holds.
    const MIN: usize = 1024;

    /// A store for [`Self::MIN`] nodes, `None` when the allocator refuses.
    fn new() -> Option<Self> {
        let mut keys = Vec::new();
        keys.try_reserve_exact(Self::MIN).ok()?;
        let mut costs = Vec::new();
        costs.try_reserve_exact(Self::MIN).ok()?;
        Some(Self {
            keys,
            costs,
            slots: filled(2 * Self::MIN, 0).ok()?,
        })
    }
    /// Bytes a store for `nodes` nodes holds: a key and a cost per node,
    /// and two slots.
    fn bytes(nodes: usize) -> u64 {
        nodes as u64 * (size_of::<u64>() + size_of::<u32>() + 2 * size_of::<u32>()) as u64
    }
    fn held(&self) -> u64 {
        Self::bytes(self.slots.len() / 2)
    }
    /// The slot holding `key`'s id, or the empty slot where it would go.
    fn find(&self, key: u64) -> usize {
        probe(&self.slots, &self.keys, key)
    }
    /// The id in `slot`, `None` when it is empty.
    fn id(&self, slot: usize) -> Option<usize> {
        self.slots[slot].checked_sub(1).map(|id| id as usize)
    }
    /// Whether a new node needs the store to grow first.
    fn full(&self) -> bool {
        2 * self.keys.len() == self.slots.len()
    }
    /// Stores `key` with `cost` in the empty `slot` that [`Self::find`]
    /// gave, which the store has room for, and returns its id.
    fn insert(&mut self, slot: usize, key: u64, cost: u32) -> usize {
        let id = self.keys.len();
        self.keys.push(key);
        self.costs.push(cost);
        self.slots[slot] = id as u32 + 1;
        id
    }
    /// Doubles what the store holds. False when the new buffers, while the
    /// old ones are still held, would take more than `room` bytes, when ids
    /// would outgrow the queue's id bits, or when the allocator refuses.
    fn grow(&mut self, room: u64) -> bool {
        let nodes = self.slots.len();
        if nodes > ID_MASK as usize + 1 || Self::bytes(nodes) > room {
            return false;
        }
        let Ok(mut slots) = filled(2 * nodes, 0) else {
            return false;
        };
        let more = nodes - self.keys.len();
        if self.keys.try_reserve_exact(more).is_err() || self.costs.try_reserve_exact(more).is_err()
        {
            return false;
        }
        for (id, &key) in self.keys.iter().enumerate() {
            let slot = probe(&slots, &self.keys, key);
            slots[slot] = id as u32 + 1;
        }
        self.slots = slots;
        true
    }
}

/// Linear probing from `key`'s hash: the slot holding its id, or the first
/// empty one.
fn probe(slots: &[u32], keys: &[u64], key: u64) -> usize {
    let mask = slots.len() - 1;
    let mut slot = mix(key) as usize & mask;
    loop {
        match slots[slot] {
            0 => return slot,
            id if keys[id as usize - 1] == key => return slot,
            _ => slot = (slot + 1) & mask,
        }
    }
}

/// Rescheduling one or two boxes: A* over nodes (k, at, tag), where k of
/// the `m` kept pushes are done, the freed boxes are on `at`, and the tag
/// fixes the keeper. A node's cost is the moves so far, walks included.
/// From each node the next kept push goes (unless it would push a box onto
/// a freed one), or one of the freed boxes' own pushes, each after the
/// shortest walk to its stand with every box in place. The goal is k = m
/// with the freed boxes on the goals they ended on, or on each other's
/// when they share a label (a kick: on each other's only).
///
/// The heuristic is the larger of two lower bounds, both consistent (see
/// docs/route-polish.md): the m - k kept pushes plus the freed boxes' push
/// distances to their goals, and the m - k kept pushes plus the walks they
/// still need with only the other boxes in the way.
struct Job {
    /// How many boxes the job frees, 1 or 2.
    freed: usize,
    /// The freed boxes' goal cells, the ones they ended on (for a kick,
    /// each other's), and their heuristic columns, in slot order.
    goals: [Cell; FREED],
    columns: [usize; FREED],
    /// Whether the two freed boxes share a label, so either may take
    /// either goal.
    swap: bool,
    /// Whether the job is a kick: two boxes of one label made to take each
    /// other's goals, under a bound above the route's length.
    kick: bool,
    /// The freed boxes' cells in the start, and the keeper's.
    root: [Cell; FREED],
    player: Cell,
    /// The length of the route to beat, or for a kick that plus
    /// [`KICK_SLACK`]: a found route is shorter.
    bound: u32,
    /// Expansions left: unbounded for one box, [`PAIR_POPS`] for two.
    budget: usize,
    /// Boxes but the freed ones.
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
    nodes: Nodes,
    heap: BinaryHeap<u64>,
    /// The bytes the job may hold, and those of its buffers that never
    /// grow; the node store and the queue take the rest as they grow.
    allowance: u64,
    fixed: u64,
    /// Scratch: the boxes a flood treats as walls.
    blocked: Vec<Cell>,
    /// Setup units left: one per layer, from the last, then the root.
    pending: usize,
}

impl Job {
    /// The job for the boxes in slots `freed` of `route`, or `None` when
    /// one of them never moves, a goal has no column, the route does not
    /// replay, or the job's buffers, a store of [`Nodes::MIN`] nodes and a
    /// queue of [`MIN_QUEUE`] entries do not fit in `allowance` bytes.
    fn new(
        board: &Board,
        heuristic: &Heuristic,
        start: &State,
        route: &[u8],
        freed: &[usize],
        allowance: u64,
    ) -> Option<Self> {
        debug_assert!((1..=FREED).contains(&freed.len()));
        let boxes = board.labels().len();
        let cells = board.tiles().len();
        let mut state = *start;
        let (mut own, mut total) = ([0usize; FREED], 0);
        for &direction in route {
            if let Step::Push(i) = board.step(&mut state, direction as usize)? {
                total += 1;
                if let Some(j) = freed.iter().position(|&slot| slot == i) {
                    own[j] += 1;
                }
            }
        }
        if own[..freed.len()].contains(&0) {
            return None;
        }
        let (mut goals, mut columns, mut root) = ([NONE; FREED], [0; FREED], [NONE; FREED]);
        for (j, &slot) in freed.iter().enumerate() {
            goals[j] = state.boxes[slot];
            columns[j] = heuristic
                .goal_cells()
                .iter()
                .position(|&cell| cell == goals[j])?;
            debug_assert!(heuristic.group(slot).contains(&columns[j]));
            root[j] = start.boxes[slot];
        }
        let m = total - own.iter().sum::<usize>();
        let stride = boxes - freed.len();
        let bytes = |count: usize, size: usize| count as u64 * size as u64;
        let fixed = bytes(m, size_of::<Event>())
            + bytes((m + 1) * stride, size_of::<Cell>())
            + bytes(m * cells, size_of::<u16>())
            + bytes(m + 1, size_of::<u32>())
            + bytes(boxes, size_of::<Cell>());
        if fixed + Nodes::bytes(Nodes::MIN) + bytes(MIN_QUEUE, size_of::<u64>()) > allowance {
            return None;
        }
        let mut events = Vec::new();
        events.try_reserve_exact(m).ok()?;
        let mut others = Vec::new();
        others.try_reserve_exact((m + 1) * stride).ok()?;
        let mut blocked = Vec::new();
        blocked.try_reserve_exact(boxes).ok()?;
        let mut heap = BinaryHeap::new();
        heap.try_reserve_exact(MIN_QUEUE).ok()?;
        let snapshot = |state: &State, others: &mut Vec<Cell>| {
            let slots = state.boxes[..boxes].iter().enumerate();
            others.extend(
                slots
                    .filter(|(i, _)| !freed.contains(i))
                    .map(|(_, &cell)| cell),
            );
        };
        let mut state = *start;
        snapshot(&state, &mut others);
        for &direction in route {
            let stand = state.player;
            if let Some(Step::Push(i)) = board.step(&mut state, direction as usize)
                && !freed.contains(&i)
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
            freed: freed.len(),
            goals,
            columns,
            swap: freed.len() == 2 && same_label(heuristic, [freed[0], freed[1]]),
            kick: false,
            root,
            player: start.player,
            bound: route.len() as u32,
            budget: if freed.len() == 1 {
                usize::MAX
            } else {
                PAIR_POPS
            },
            stride,
            cells,
            events,
            others,
            walks: filled(m * cells, NONE).ok()?,
            tails: filled(m + 1, 0).ok()?,
            nodes: Nodes::new()?,
            heap,
            allowance,
            fixed,
            blocked,
            pending: m + 1,
        })
    }

    /// The kick for the same-label boxes in slots `pair` of `route`: their
    /// pair job, with each made to end on the other's goal and the bound
    /// raised by [`KICK_SLACK`]. It finds the shortest route in which they
    /// trade goals, which may be longer than `route`. `None` as for
    /// [`Job::new`].
    fn kick(
        board: &Board,
        heuristic: &Heuristic,
        start: &State,
        route: &[u8],
        pair: [usize; 2],
        allowance: u64,
    ) -> Option<Self> {
        let mut job = Self::new(board, heuristic, start, route, &pair, allowance)?;
        debug_assert!(job.swap);
        job.goals.swap(0, 1);
        job.columns.swap(0, 1);
        job.swap = false;
        job.kick = true;
        job.bound = (job.bound + KICK_SLACK).min(MAX_ROUTE as u32);
        Some(job)
    }

    /// Runs one unit: a setup step, or one pop. Each unit floods for
    /// itself, so no flood outlives it.
    fn step(&mut self, board: &Board, heuristic: &Heuristic, reach: &mut Reach) -> Progress {
        if self.pending > 0 {
            self.pending -= 1;
            return self.prepare(board, heuristic, reach);
        }
        let Some(entry) = self.heap.pop() else {
            return Progress::Failed;
        };
        let id = (entry & ID_MASK) as usize;
        let f = F_MASK - (entry >> (F_BITS + ID_BITS));
        let h = F_MASK - ((entry >> ID_BITS) & F_MASK);
        let g = (f - h) as u32;
        // Stale: a cheaper path to the node was queued since.
        if self.nodes.costs[id] >> TAG_BITS != g {
            return Progress::Running;
        }
        let Some(budget) = self.budget.checked_sub(1) else {
            return Progress::Failed;
        };
        self.budget = budget;
        let node = Node::unpack(self.nodes.keys[id]);
        let m = self.events.len();
        if node.k == m && self.arrived(&node.at) {
            return self.rebuild(board, reach, id, g);
        }
        let q = self.keeper(board, node);
        self.flood(board, reach, node.k, &node.at, q);
        if node.k < m {
            let event = self.events[node.k];
            let walk = reach.distance(event.stand);
            if !node.at[..self.freed].contains(&event.to)
                && walk != NONE
                && let Some(h) = self.estimate(heuristic, node.k + 1, &node.at, event.from)
            {
                let next = Node {
                    k: node.k + 1,
                    tag: 0,
                    ..node
                };
                if !self.relax(next, g + u32::from(walk) + 1, h, node.tag) {
                    return Progress::Failed;
                }
            }
        }
        for (i, &p) in node.at[..self.freed].iter().enumerate() {
            for (d, &opposite) in OPPOSITE.iter().enumerate() {
                let to = board.neighbors()[p as usize][d];
                let stand = board.neighbors()[p as usize][opposite];
                if to == NONE || stand == NONE || reach.blocked(to) {
                    continue;
                }
                let walk = reach.distance(stand);
                if walk == NONE {
                    continue;
                }
                let mut at = node.at;
                at[i] = to;
                let Some(h) = self.estimate(heuristic, node.k, &at, p) else {
                    continue;
                };
                let next = Node {
                    k: node.k,
                    at,
                    tag: 1 + 4 * i + d,
                };
                if !self.relax(next, g + u32::from(walk) + 1, h, node.tag) {
                    return Progress::Failed;
                }
            }
        }
        Progress::Running
    }

    /// One setup unit: builds the walk layer `pending - 1`, from the last
    /// layer down so each tail can add the one after it, then queues the
    /// root once every layer is built.
    fn prepare(&mut self, board: &Board, heuristic: &Heuristic, reach: &mut Reach) -> Progress {
        if self.pending == 0 {
            let root = Node {
                k: 0,
                at: self.root,
                tag: 0,
            };
            return match self.estimate(heuristic, 0, &self.root, self.player) {
                Some(h) if self.relax(root, 0, h, 0) => Progress::Running,
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

    /// The keeper's cell at `node`.
    fn keeper(&self, board: &Board, node: Node) -> Cell {
        match node.tag {
            0 => self.entry(node.k),
            tag => board.neighbors()[node.at[(tag - 1) / 4] as usize][OPPOSITE[(tag - 1) % 4]],
        }
    }

    /// The other boxes' cells after `k` kept pushes.
    fn others(&self, k: usize) -> &[Cell] {
        &self.others[k * self.stride..(k + 1) * self.stride]
    }

    /// Floods from `keeper` with the other boxes after `k` kept pushes and
    /// the freed boxes, on `at`, in the way.
    fn flood(
        &mut self,
        board: &Board,
        reach: &mut Reach,
        k: usize,
        at: &[Cell; FREED],
        keeper: Cell,
    ) {
        self.blocked.clear();
        self.blocked
            .extend_from_slice(&self.others[k * self.stride..(k + 1) * self.stride]);
        self.blocked.extend_from_slice(&at[..self.freed]);
        reach.fill_from(board, keeper, &self.blocked);
    }

    /// Whether the freed boxes on `at` are on their goals, or on each
    /// other's when they share a label.
    fn arrived(&self, at: &[Cell; FREED]) -> bool {
        at[..self.freed] == self.goals[..self.freed]
            || (self.swap && at[0] == self.goals[1] && at[1] == self.goals[0])
    }

    /// The fewest pushes the freed boxes on `at` still need, each alone
    /// with the keeper anywhere, summed; with a shared label, the cheaper
    /// of the two ways to match them to the goals. `None` when no way is
    /// finite. Each push moves one box, so the sum is a lower bound, and it
    /// drops by at most one per push, as does the cheaper of two such sums.
    fn pushes(&self, heuristic: &Heuristic, at: &[Cell; FREED]) -> Option<u32> {
        let sum = |columns: [usize; FREED]| -> Option<u32> {
            (0..self.freed)
                .map(|i| match heuristic.distance(at[i], columns[i]) {
                    NONE => None,
                    distance => Some(u32::from(distance)),
                })
                .sum()
        };
        let direct = sum(self.columns);
        if !self.swap {
            return direct;
        }
        direct
            .into_iter()
            .chain(sum([self.columns[1], self.columns[0]]))
            .min()
    }

    /// A lower bound on the moves left from layer `k` with the freed boxes
    /// on `at` and the keeper on `q`, or `None` when no push sequence takes
    /// them to their goals or the keeper can no longer reach the next kept
    /// push's stand.
    fn estimate(
        &self,
        heuristic: &Heuristic,
        k: usize,
        at: &[Cell; FREED],
        q: Cell,
    ) -> Option<u32> {
        let m = self.events.len();
        let kept = (m - k) as u32;
        let pushes = self.pushes(heuristic, at)?;
        if k == m {
            return Some(kept + pushes);
        }
        let walk = self.walks[k * self.cells + q as usize];
        if walk == NONE {
            return None;
        }
        Some(kept + pushes.max(u32::from(walk) + self.tails[k + 1]))
    }

    /// Bytes the job may still allocate: its allowance less what it holds.
    fn room(&self) -> u64 {
        let held = self.fixed + self.nodes.held() + ENTRY * self.heap.capacity() as u64;
        self.allowance.saturating_sub(held)
    }

    /// Records cost `g` for `node`, reached from a node tagged `parent`,
    /// and queues it, unless `g + h` cannot beat the bound or the node
    /// already has a cost no higher. False when the store or the queue
    /// could not grow within the allowance, which ends the job.
    fn relax(&mut self, node: Node, g: u32, h: u32, parent: usize) -> bool {
        if g + h >= self.bound {
            return true;
        }
        let packed = node.pack();
        let cost = (g << TAG_BITS) | parent as u32;
        let mut slot = self.nodes.find(packed);
        let id = match self.nodes.id(slot) {
            Some(id) if self.nodes.costs[id] >> TAG_BITS <= g => return true,
            Some(id) => {
                self.nodes.costs[id] = cost;
                id
            }
            None => {
                if self.nodes.full() {
                    let room = self.room();
                    if !self.nodes.grow(room) {
                        return false;
                    }
                    slot = self.nodes.find(packed);
                }
                self.nodes.insert(slot, packed, cost)
            }
        };
        let capacity = self.heap.capacity();
        if self.heap.len() == capacity {
            // A growing queue briefly holds its old and new buffers, and
            // the old one is already counted as held.
            let most = usize::try_from(self.room() / ENTRY).unwrap_or(usize::MAX);
            let extra = capacity.max(64).min(most.saturating_sub(capacity));
            if extra == 0 || self.heap.try_reserve_exact(extra).is_err() {
                return false;
            }
        }
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
            let node = Node::unpack(self.nodes.keys[id]);
            if node.k == 0 && node.tag == 0 {
                break;
            }
            let parent = (self.nodes.costs[id] & TAG_MASK) as usize;
            let (previous, stand) = if node.tag == 0 {
                let event = self.events[node.k - 1];
                route.push(event.direction);
                let previous = Node {
                    k: node.k - 1,
                    tag: parent,
                    ..node
                };
                (previous, event.stand)
            } else {
                let (i, d) = ((node.tag - 1) / 4, (node.tag - 1) % 4);
                route.push(d as u8);
                let from = board.neighbors()[node.at[i] as usize][OPPOSITE[d]];
                let mut at = node.at;
                at[i] = from;
                let previous = Node {
                    k: node.k,
                    at,
                    tag: parent,
                };
                (previous, board.neighbors()[from as usize][OPPOSITE[d]])
            };
            let q = self.keeper(board, previous);
            self.flood(board, reach, previous.k, &previous.at, q);
            reach.append_walk_reversed(board, stand, &mut route);
            // Every node on the path was reached, so the store holds it.
            let Some(found) = self.nodes.id(self.nodes.find(previous.pack())) else {
                return Progress::Failed;
            };
            id = found;
        }
        route.reverse();
        debug_assert_eq!(route.len(), g as usize);
        Progress::Found(route)
    }
}

#[cfg(test)]
mod tests {
    use super::{Heuristic, Reach, descend, replay};
    use sokomind_core::Board;

    /// A detour route around the direct line: the descent's single-box
    /// job keeps every other push — here none — and re-schedules the
    /// box's own, finding the three pushes of the direct run where the
    /// route took five around the block.
    #[test]
    fn a_descended_detour_finds_the_direct_line() {
        let board = Board::parse(
            "OOOOOOOOOO\n\
             O        O\n\
             O R X  S O\n\
             O        O\n\
             O        O\n\
             OOOOOOOOOO",
        )
        .unwrap();
        let start = board.initial();
        // The box taken the long way: pushed down, along the row below,
        // and up onto its goal — 12 moves, 5 pushes.
        let route: Vec<u8> = "urrdldrrrdru"
            .bytes()
            .map(|c| match c {
                b'u' => 0,
                b'd' => 1,
                b'l' => 2,
                _ => 3,
            })
            .collect();
        assert!(replay(&board, &start, &route).is_ok());
        let heuristic = Heuristic::new(&board, None);
        let mut reach = Reach::new(board.tiles().len());
        let (descended, units) = descend(&board, &heuristic, &start, route, 1 << 20, &mut reach);
        assert_eq!(descended.len(), 4);
        assert!(units > 0);
    }

    /// The pool runs every queued task exactly once, and ends cleanly
    /// once its senders are gone.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_pool_runs_every_task() {
        use std::sync::{Arc, Mutex};
        let pool = super::Pool::new(4).unwrap();
        let count = Arc::new(Mutex::new(0));
        let (send, recv) = std::sync::mpsc::channel();
        for i in 0..16 {
            let (count, send) = (Arc::clone(&count), send.clone());
            pool.submit(Box::new(move || {
                *count.lock().unwrap() += 1;
                send.send(i).unwrap();
            }));
        }
        drop(send);
        let mut seen = 0;
        while recv.recv().is_ok() {
            seen += 1;
        }
        drop(pool);
        assert_eq!(seen, 16);
        assert_eq!(*count.lock().unwrap(), 16);
    }
}
