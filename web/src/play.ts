/** The play page's core: the loaded session over the WASM game, the solver,
 * playback, progress and timer clients around it, the board and its zoom, the
 * paint/score/finish pipeline, the search laboratory's log and result panels,
 * the hint, the tap walks, the share links, and the engine-shared sound every
 * move's cue rides. The one module the pages read; it reads no page. */
import { WasmGame } from '../wasm/sokomind';
import { BoardView } from './board.ts';
import { daily, ENTRIES, rating, type Entry, type Rating } from './catalog.ts';
import * as storage from './storage.ts';
import * as theme from './theme.ts';
import { formatHash, navigate, type Route } from './router.ts';
import { playHash } from './shell.ts';
import { walkRoute } from './walk.ts';
import { PinchZoom } from './zoom.ts';
import { clock, Timer } from './timer.ts';
import { present } from './complete.ts';
import { celebrate } from './celebrate.ts';
import { openDialog } from './dialog.ts';
import { MAX_ROUTE, MAX_STATES, MODES, type SearchUpdate, type Snapshot, type SolveRequest } from './protocol.ts';
import { ENGINES, SolverClient } from './solver-client.ts';
import { decodeSnapshot } from './transport.ts';
import { Playback } from './playback.ts';
import { Progress } from './progress.ts';
import * as journal from './journal.ts';
import * as stats from './stats.ts';
import * as achievements from './achievements.ts';
import * as frames from './frames.ts';
import { idb } from './idb.ts';
import * as optimal from './optimal.ts';
import * as replays from './replays.ts';
import { statusText } from './verdict.ts';
import * as editor from './editor.ts';
import * as audio from './audio.ts';
import { $, button, choice, message, select, setStatus, setText } from './dom.ts';

/** The loaded puzzle. load() replaces it whole; render() refreshes `state`
 * from the game after every change. */
interface Session {
  game: WasmGame;
  puzzle: Entry & { text: string };
  tiles: Uint8Array;
  labels: Uint8Array;
  state: Snapshot;
}
const seconds = (ms: number) => `${(ms / 1000).toFixed(1)} s`;
const MOVE_HINT = 'Arrow keys or WASD to move. U to undo.';
/** The id of a puzzle loaded from pasted rows. Its best is stored with those rows, so it shows
 * only while the same rows are loaded again. */
const CUSTOM_PUZZLE_ID = 'custom';
const customPuzzle = (text: string): Entry => {
  const rows = text.split('\n');
  return {
    id: CUSTOM_PUZZLE_ID,
    title: 'Your puzzle',
    difficulty: 'custom',
    collection: 'Your puzzles',
    boxes: rows
      .join('')
      .split('')
      .filter(symbol => symbol >= 'A' && symbol <= 'Z' && symbol !== 'R' && symbol !== 'S').length,
    rows,
    width: Math.max(...rows.map(row => row.length)),
    height: rows.length,
  };
};
const board = new BoardView($('board'));
const zoom = new PinchZoom($('board'));
/** Undefined until start() loads the first puzzle. */
let session: Session | undefined;
/** The session, for code that only runs once a puzzle is loaded: event
 * handlers bound after the first load, and playback and search callbacks. */
function loaded(): Session {
  if (!session) throw new Error('No puzzle is loaded');
  return session;
}
/** The search laboratory's status log: what the solver said, and when —
 * newest first, at most thirty entries. */
interface LogEntry {
  elapsedMs: number;
  message: string;
}
const searchLog: LogEntry[] = [];
/** The search's live elapsed time, as the log stamps its entries. */
let searchElapsed = 0;

function logSearch(message: string) {
  // A running search repeats its status; the log records each change once.
  if (searchLog[0]?.message === message) return;
  searchLog.unshift({ elapsedMs: searchElapsed, message });
  if (searchLog.length > 30) searchLog.pop();
  $('solver-log').replaceChildren(
    ...searchLog.map(entry => {
      const item = document.createElement('li');
      const time = document.createElement('time');
      time.textContent = seconds(entry.elapsedMs);
      const text = document.createElement('span');
      text.textContent = entry.message;
      item.append(time, text);
      return item;
    }),
  );
}

const solver = new SolverClient({
  worker: () => new Worker(new URL('./solver.worker.ts', import.meta.url), { type: 'module' }),
  changed: updateButtons,
  elapsed: ms => {
    searchElapsed = ms;
    setText('elapsed', seconds(ms));
  },
  update: applyUpdate,
  status: value => {
    setStatus(value);
    logSearch(value);
  },
  verify: (prefix, route) => {
    verify(loaded().puzzle.text, prefix + route);
  },
});
const playback = new Playback(stepWithCue, changed, blocked => {
  solver.dropRoute();
  updateButtons();
  if (blocked) message('Replay was blocked.');
});
// The page's pause holds a playing route's steps: the robot stops behind the
// overlay, and the route resumes where it stood when the pause lifts.
playback.holds = () => paused;
const progress = new Progress({ verify, show: showBest, status: value => setText('storage', value) });
const timer = new Timer({ show: ms => setText('timer', clock(ms)) });
/** The best on show, for the score card's pace deltas. */
let best: storage.Best | null = null;

function updateButtons() {
  const busy = solver.busy;
  const state = session?.state;
  button('solve').disabled = busy || !state || state.solved;
  button('cancel').disabled = !busy && !playback.active;
  button('play').disabled = busy || (solver.route === undefined && !playback.active);
  setText('play', !playback.active ? 'Play route' : playback.state.kind === 'paused' ? 'Resume' : 'Pause');
  button('copy').disabled = solver.route === undefined;
  button('hint').disabled = hint.busy || paused || !state || state.solved;
  button('undo').disabled = !state || state.moves === 0 || paused;
}
function animate(route: string) {
  playback.play(route);
  updateButtons();
}
function invalidate() {
  solver.reset();
  // The position is moving on, and a hint answers for the position it was
  // asked about: it goes with the search and the playback. The client
  // silences the worker before terminating it, so no late nudge lands.
  hint.reset();
  playback.stop();
  updateButtons();
  setStatus('Ready when you are.');
}
/** Reads the game's position into the session, then paints it. */
function render() {
  const current = loaded();
  current.state = decodeSnapshot(current.game.snapshot(), current.labels.length);
  paint(current);
}
/** The dead boxes the last paint saw, and the pushes it saw them at: a push
 * that strands a new box opens the deadlock dialog. */
let deadCount = 0;
let lastPushes = 0;

function paint({ game, tiles, labels, state }: Session) {
  const onGoal = Array.from(labels, (_, i) => game.on_goal(i));
  const stuck = game.deadlocked_boxes();
  board.draw(game.width(), game.height(), tiles, labels, state, onGoal);
  board.markDead(stuck);
  if (stuck.length > deadCount && state.pushes > lastPushes) {
    openDialog('deadlock');
    message('Deadlock: a box cannot reach its goal from here.');
    void sound.playCue('blocked', { variant: 'deadlock' });
  }
  deadCount = stuck.length;
  lastPushes = state.pushes;
  setText('moves', String(state.moves));
  setText('pushes', String(state.pushes));
  score(state, onGoal, game.actions());
  updateButtons();
}
function saveSession() {
  if (session) progress.session(session.game.actions());
}
/** The arrows the move notation shows for the route letters. */
const GLYPHS: Record<string, string> = { U: '↑', D: '↓', L: '←', R: '→' };

/** Paints the score card and the stage's move strips: the pace deltas against
 * the best on show, the efficiency and placed counts, the recent moves, the
 * walk/push split, and the undo depth. */
function score(state: Snapshot, onGoal: boolean[], actions: string) {
  const recent = actions.length > 24 ? `…${actions.slice(-24)}` : actions;
  setText(
    'notation',
    recent.replace(/[UDLR]/g, letter => GLYPHS[letter]!),
  );
  const share = (count: number) => (state.moves === 0 ? '0%' : `${Math.round((count / state.moves) * 100)}%`);
  $<HTMLElement>('walks').style.width = share(state.moves - state.pushes);
  $<HTMLElement>('pushes-bar').style.width = share(state.pushes);
  setText('push-share', state.moves === 0 ? '' : `${share(state.pushes)} pushes`);
  setText('efficiency', state.moves === 0 ? '—' : share(state.pushes));
  setText('placed', `${onGoal.filter(Boolean).length} / ${onGoal.length}`);
  delta('moves-delta', state.moves, best?.moves);
  delta('pushes-delta', state.pushes, best?.pushes);
  setText('undo-depth', state.moves === 0 ? '' : `(${state.moves})`);
  button('undo-5').hidden = state.moves < 5;
}

/** The signed difference from the best on show, colored by which way it reads. */
function delta(id: string, value: number, against: number | undefined) {
  if (against === undefined) {
    setText(id, '');
    return;
  }
  const sign = value === against ? '' : value > against ? '+' : '−';
  setText(id, `${sign}${Math.abs(value - against)}`);
  $(id).className = `delta ${value <= against ? 'good' : 'bad'}`;
}

function showBest(record: storage.Best | null) {
  best = record;
  button('best').disabled = !record;
  setText('best-score', record ? `Best: ${record.moves} moves · ${record.pushes} pushes` : '');
  // A local const narrows into the callback; the module's let cannot.
  const current = session;
  if (current)
    score(
      current.state,
      Array.from(current.labels, (_, i) => current.game.on_goal(i)),
      current.game.actions(),
    );
}
// Stored routes are untrusted: replay one from the start on a scratch game.
function verify(rows: string, route: string): Snapshot {
  const check = new WasmGame(rows);
  try {
    check.replay(route);
    const result = decodeSnapshot(check.snapshot(), check.labels().length);
    if (!result.solved) throw new Error('Route does not solve this puzzle');
    return result;
  } finally {
    check.free();
  }
}
/** The replay repository's verifier: the same scratch-game replay, answering
 * the counts a stored route must confirm. */
const verifyRoute: replays.Verify = (rows, actionLog) => {
  try {
    const snapshot = verify(rows, actionLog);
    return { moves: snapshot.moves, pushes: snapshot.pushes };
  } catch {
    return null;
  }
};
/** The completion dialog's rating buttons, in order. */
const RATE_BUTTONS: [string, Rating][] = [
  ['rate-easy', 'easy'],
  ['rate-right', 'right'],
  ['rate-hard', 'hard'],
];

/** Marks the completion dialog's rating buttons for the puzzle on show. */
function markRating() {
  if (!session) return;
  const current = rating(session.puzzle.id);
  for (const [id, value] of RATE_BUTTONS) button(id).classList.toggle('on', current === value);
}

/** Opens the completion dialog for a solve, against the best that stood before
 * it, bursts the celebration sized by the tier the solve earned, lists any
 * achievements the solve unlocked, and stars a route the engine has certified
 * as move-optimal. */
function finish(state: Snapshot, before: storage.Best | null, earned: readonly achievements.Def[] = []) {
  const { puzzle } = loaded();
  const certified = optimal.proven(puzzle.id, puzzle.text);
  const shown = present({ moves: state.moves, pushes: state.pushes }, before, certified);
  setText('complete-eyebrow', shown.eyebrow);
  setText('complete-title', shown.title);
  setText('complete-summary', shown.summary);
  document.getElementById('complete-milestones')!.replaceChildren(
    ...shown.milestones.map(text => {
      const item = document.createElement('li');
      item.textContent = text;
      return item;
    }),
    ...earned.map(def => {
      const item = document.createElement('li');
      item.textContent = `Achievement earned · ${def.title}`;
      return item;
    }),
  );
  const signed = (value: number, against: number | undefined) =>
    against === undefined
      ? String(value)
      : `${value} (${value === against ? '=' : value < against ? '−' : '+'}${Math.abs(value - against)})`;
  setText('complete-moves', signed(state.moves, before?.moves));
  setText('complete-pushes', signed(state.pushes, before?.pushes));
  setText('complete-time', clock(timer.elapsed()));
  markRating();
  // The celebration's chord: the engine's certification outranks the record,
  // and a first clear outranks a personal best.
  void sound.playCue('solve', { variant: audio.solveVariant(before, state.moves, certified !== undefined) });
  openDialog('complete');
  celebrate(before === null ? 'first' : shown.eyebrow === 'PERSONAL MILESTONE' ? 'best' : 'clear');
}

/** Toggles zen mode: the board and its controls alone, the chrome folded away. */
function toggleZen() {
  zen = !zen;
  document.querySelector('[data-page="play"]')?.classList.toggle('zen', zen);
  $('zen-dock').hidden = !zen;
}

/** Pauses the attempt: input, searches and the clock stop until P resumes them. */
function togglePause() {
  paused = !paused;
  $('pause').hidden = !paused;
  if (paused) {
    timer.pause();
    // A hint running under a pause would land its nudge on resume, unasked for.
    hint.reset();
  } else if (session !== undefined && !session.state.solved && session.state.moves > 0) timer.run();
  updateButtons();
}

function changed() {
  render();
  const { game, state } = loaded();
  if (!state.solved) {
    // The clock runs only while attempting: the first move starts it, a
    // replayed route or a return to the start holds it, and a solve ends it.
    if (playback.active || state.moves === 0) timer.pause();
    else timer.run();
    message(MOVE_HINT);
    progress.session(game.actions(), true);
    return;
  }
  timer.pause();
  const before = best;
  message(`Solved in ${state.moves} moves and ${state.pushes} pushes.`);
  const actions = game.actions();
  progress.session(actions);
  progress.keep(actions, state.moves, state.pushes);
  // Every solve joins the journal — its dates feed the streaks, the daily run
  // and the recently cleared — and today's daily pick records the run itself.
  // A custom puzzle keeps only its rows-scoped best, never catalog progress.
  const { puzzle } = loaded();
  let earned: readonly achievements.Def[] = [];
  if (puzzle.id !== CUSTOM_PUZZLE_ID) {
    // The unlock moment: the achievements standing before the solve, against
    // the journal it joins.
    const priorBook = journal.load();
    const priorIds = achievements.unlockedIds(stats.aggregates(priorBook), priorBook);
    journal.keep(puzzle.id, state.moves, state.pushes, timer.elapsed());
    if (puzzle.id === daily().id) journal.keepDaily(puzzle.id);
    const book = journal.load();
    earned = achievements.newlyUnlocked(priorIds, achievements.unlockedIds(stats.aggregates(book), book));
    // A solve can earn the frame the board wears.
    frames.apply();
    // The route joins the replay repository, replay-verified before it
    // lands; only an improvement over the best before it stays.
    void replays.promote(
      idb,
      puzzle.id,
      puzzle.text,
      {
        actionLog: actions,
        moves: state.moves,
        pushes: state.pushes,
        completedAt: new Date().toISOString(),
        elapsedMs: timer.elapsed(),
      },
      verifyRoute,
    );
  }
  finish(state, before, earned);
}
function load(puzzle: Entry, actions = '') {
  const text = puzzle.rows.join('\n');
  const game = new WasmGame(text);
  let next: Session;
  try {
    if (actions) game.replay(actions);
    const tiles = game.tiles(),
      labels = game.labels();
    next = { game, puzzle: { ...puzzle, text }, tiles, labels, state: decodeSnapshot(game.snapshot(), labels.length) };
  } catch (error) {
    game.free();
    throw error;
  }
  if (session) {
    invalidate();
    session.game.free();
  }
  session = next;
  select('puzzles').value = puzzle.id;
  const index = ENTRIES.findIndex(p => p.id === puzzle.id);
  setText('position', index === -1 ? 'custom' : `${index + 1} / ${ENTRIES.length}`);
  setText('title', puzzle.title);
  setText('difficulty', puzzle.difficulty);
  setText('note', puzzle.hint || 'Match every box to its goal.');
  $<HTMLTextAreaElement>('rows').value = text;
  // A load never opens the deadlock dialog: the counters start from the
  // position as loaded, so only a push within the session that strands a new
  // box opens it, never the position a restore lands on.
  deadCount = next.game.deadlocked_boxes().length;
  lastPushes = next.state.pushes;
  paint(next);
  progress.select(next.puzzle.id, next.puzzle.text);
  timer.select(next.puzzle.id);
  // The clock runs only while attempting: a fresh board holds it until its
  // first move, a restored session runs from its restored moves, and a
  // restored solve holds its time.
  if (next.state.solved || next.state.moves === 0) timer.pause();
  message(actions ? 'Session restored by replaying your moves.' : MOVE_HINT);
  saveSession();
}
/** Pause and zen are page-level modes: pause locks input and the clock, zen
 * folds the chrome away around the board. */
let paused = false;
let zen = false;

function move(direction: number) {
  if (!session || paused) return;
  const { game } = session;
  if (solver.busy || hint.busy || solver.route !== undefined || playback.active) invalidate();
  if (stepWithCue(direction)) changed();
  else {
    board.blocked(direction);
    if (game.moves() >= MAX_ROUTE) message(`Session move limit reached (${MAX_ROUTE}). Undo or restart to continue.`);
  }
}
/** Whether the standing search started from the room's start: only such a
 * search can certify the room's optimum — one from a mid-game position
 * proves only the rest of its route. */
let searchedFromStart = false;

// Every transport route is replayed on a scratch Rust game before display.
function applyUpdate(update: SearchUpdate, route: string | undefined) {
  const { expanded, generated, reservedBytes, best, proof } = update.metrics;
  setText('expanded', expanded.toLocaleString());
  setText('generated', generated.toLocaleString());
  setText('reserved', `${(reservedBytes / 1048576).toFixed(1)} MiB`);
  setText('solver-best', best !== undefined ? `${best} moves` : '—');
  const claim = statusText(update.metrics, route);
  setStatus(claim);
  logSearch(claim);
  // A start-to-finish search that finished with an optimal proof certifies
  // the room's optimum: the cache the completion dialog's star reads.
  if (update.type === 'done' && proof.kind === 'optimal' && route !== undefined && searchedFromStart) {
    const { puzzle } = loaded();
    optimal.record(puzzle.id, puzzle.text, proof.moves);
  }
  if (update.type === 'done') showResult(update, route);
}

/** The laboratory's result panel: what the finished search returned, its
 * guarantee, and — when the engine certified the room from its start — the
 * star noting the optimum was saved. A search that stopped on a limit
 * keeps the route it stands on — Quality's normal way of ending — so the
 * panel keys on the route, not the status: only a finish without one
 * reports no route. */
function showResult(update: SearchUpdate, route: string | undefined) {
  setText(
    'solver-result-title',
    route === undefined
      ? update.metrics.status === 'cancelled'
        ? 'Search stopped'
        : 'No route returned'
      : update.metrics.status === 'solved'
        ? 'Route found'
        : 'Best route found',
  );
  setText('solver-result-summary', statusText(update.metrics, route));
  setText('solver-result-guarantee', guarantee(update.metrics, route));
  $('solver-optimal').hidden = !(update.metrics.proof.kind === 'optimal' && route !== undefined && searchedFromStart);
  $('solver-result').hidden = false;
  if (route === undefined) {
    setText('solver-result-moves', '—');
    setText('solver-result-pushes', '—');
    return;
  }
  const snapshot = verify(loaded().puzzle.text, solver.prefix + route);
  setText('solver-result-moves', snapshot.moves.toLocaleString());
  setText('solver-result-pushes', snapshot.pushes.toLocaleString());
}

/** What a finished search guarantees about its route, in the engine's own
 * words. Without a route there is nothing to guarantee: the summary line
 * beside it names the limit that stopped the search — but the measured
 * boundary steers: the boards the default budgets cannot route yield, each
 * way its own, to more memory, more time, or the other mode
 * (docs/stage-ladder.md, The app's budgets). */
function guarantee(metrics: SearchUpdate['metrics'], route: string | undefined): string {
  if (metrics.proof.kind === 'optimal') return 'Proven optimal';
  if (metrics.proof.kind === 'unsolvable') return 'Proven unsolvable';
  if (metrics.proof.kind === 'bounded') return `Best found — within ${metrics.proof.upper - metrics.proof.lower} of optimal`;
  if (route === undefined) return 'No route found — a bigger budget or a different mode may find one';
  return metrics.status === 'solved' ? 'First route found' : 'Best found before stopping';
}
function solve() {
  if (solver.busy) return;
  invalidate();
  const { game, puzzle } = loaded();
  // Only a search from the room's start can certify its optimum.
  searchedFromStart = game.moves() === 0;
  const request: SolveRequest = {
    rows: puzzle.text,
    actions: game.actions(),
    mode: choice('mode', MODES),
    timeMs: Number(select('seconds').value) * 1000,
    memoryMiB: Number(select('memory').value),
    maxStates: MAX_STATES,
  };
  for (const metric of ['expanded', 'generated', 'reserved', 'elapsed', 'solver-best']) setText(metric, '—');
  searchLog.length = 0;
  $('solver-log').replaceChildren();
  $('solver-result').hidden = true;
  setStatus('Preparing search…');
  logSearch('Preparing search…');
  void solver.solve(choice('engine', ENGINES), request);
}

/** A tap walks the robot to the tapped cell through a walk path, replayed as a
 * playback a new input replaces. A second quick touch resets the zoom instead,
 * and a touch while zoomed waits out that window first, so a double tap never
 * walks; a mouse click, which cannot zoom, always walks at once. The last tap
 * starts unset, not at zero: performance.now() also starts near zero, and a
 * first tap early in the page's life would otherwise read as a double. */
const DOUBLE_TAP_MS = 300;
let lastTap: number | undefined;
let pendingWalk: number | undefined;
function tap(x: number, y: number, touch: boolean) {
  clearTimeout(pendingWalk);
  const now = performance.now();
  if (touch && lastTap !== undefined && now - lastTap < DOUBLE_TAP_MS) {
    lastTap = undefined;
    zoom.reset();
    return;
  }
  if (touch) lastTap = now;
  const walk = () => {
    pendingWalk = undefined;
    const current = session;
    const cell = current === undefined ? undefined : board.cellAt(x, y);
    if (current === undefined || cell === undefined || paused) return;
    if (solver.busy || hint.busy || solver.route !== undefined || playback.active) invalidate();
    const route = walkRoute(current.game.width(), current.tiles, current.state.boxes, current.state.player, cell);
    if (route !== undefined) animate(route);
  };
  if (touch && zoom.zoomed) pendingWalk = setTimeout(walk, DOUBLE_TAP_MS);
  else walk();
}

/** The hint's misses in a row, and when three of them cooled it down. The
 * cooldown starts unset: performance.now() also starts near zero, and a
 * zero sentinel would read as a fresh miss. */
let hintStreak = 0;
let hintCooldown: number | undefined;

/** The hint: a fast search from the current position whose route's first
 * steps play as a nudge. Three fruitless hints cool it down for half a
 * minute. */
const hint = new SolverClient({
  worker: () => new Worker(new URL('./solver.worker.ts', import.meta.url), { type: 'module' }),
  changed: updateButtons,
  elapsed: () => {},
  update: (update, route) => {
    if (update.type !== 'done') return;
    if (route !== undefined) {
      hintStreak = 0;
      animate(route.slice(0, 3));
    } else {
      hintStreak += 1;
      if (hintStreak >= 3) {
        hintCooldown = performance.now();
        hintStreak = 0;
      }
      message('No hint found from here.');
    }
  },
  status: () => {},
  verify: (prefix, route) => {
    verify(loaded().puzzle.text, prefix + route);
  },
});

/** Asks for a hint: a fast search at the original's limits, unless one runs,
 * a route is playing, the attempt is paused, or the cooldown from three
 * misses stands. */
function askHint() {
  if (hint.busy || playback.active || paused || !session) return;
  if (hintCooldown !== undefined && performance.now() - hintCooldown < 30_000) {
    message('The hint is resting after three misses. It will return shortly.');
    return;
  }
  const { game, puzzle } = loaded();
  void hint.solve('browser', {
    rows: puzzle.text,
    actions: game.actions(),
    mode: 'fast',
    timeMs: 5000,
    memoryMiB: 128,
    maxStates: MAX_STATES,
  });
}

/** The most moves a share link carries; longer attempts share their start. */
const SHARED_ACTIONS_CAP = 2000;

/** Shares the attempt as its #/play/ link — the moves so far ride the URL —
 * through the browser's share sheet when it has one, the clipboard otherwise.
 * A pasted puzzle has no catalog id, so it shares through the editor's link:
 * the receiver gets a preview they can play or import. */
async function share() {
  const { game, puzzle } = loaded();
  let url: string;
  if (puzzle.id === CUSTOM_PUZZLE_ID) {
    url = `${location.origin}${location.pathname}${formatHash({
      kind: 'editor',
      custom: editor.encodeShare({
        title: 'Your puzzle',
        difficulty: 'beginner',
        rows: puzzle.rows,
        boxes: editor.countBoxes(puzzle.rows),
      }),
    })}`;
  } else {
    const played = game.actions().slice(0, SHARED_ACTIONS_CAP);
    url = `${location.origin}${location.pathname}${playHash(puzzle.id)}${played === '' ? '' : `?play=${played}`}`;
  }
  try {
    if (navigator.share !== undefined) {
      await navigator.share({ title: puzzle.title, url });
      return;
    }
    await navigator.clipboard.writeText(url);
    message('Share link copied.');
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') return;
    message('Sharing is unavailable in this browser.');
  }
}

/** A route the replay shelf asked to play: its room loads fresh, and the
 * route plays as a playback. */
let pendingReplay: { id: string; route: string } | undefined;

/** Loads the play route's puzzle: a shared log replays as given, and without one the
 * saved session continues when it was saved for this puzzle. A route that names no
 * catalog puzzle, or a log that does not replay, goes home. The shelf's replay
 * takes precedence: a fresh board, the route as a playback. */
function enterPlay(next: Route) {
  if (next.kind !== 'play') return;
  const puzzle = ENTRIES.find(p => p.id === next.puzzleId);
  if (!puzzle) {
    navigate({ kind: 'home' });
    return;
  }
  const replay = pendingReplay !== undefined && pendingReplay.id === puzzle.id ? pendingReplay : undefined;
  pendingReplay = undefined;
  const saved = storage.session();
  const actions =
    replay === undefined ? (next.actionLog ?? (saved?.id === puzzle.id && saved.rows === puzzle.rows.join('\n') ? saved.actions : '')) : '';
  try {
    load(puzzle, actions);
  } catch {
    navigate({ kind: 'home' });
    return;
  }
  if (replay !== undefined) {
    invalidate();
    animate(replay.route);
  }
}

/** The page's synthesizer, quiet until a gesture unlocks it. */
const sound = new audio.AudioController(theme.preferences());

/** How many boxes stand on their goals, straight off the engine. */
function goalsOn(game: WasmGame, labels: Uint8Array): number {
  let count = 0;
  for (let index = 0; index < labels.length; index++) if (game.on_goal(index)) count++;
  return count;
}

/** Steps the game and plays the move's cue off the engine's own verdict —
 * blocked, a step, a push, or a push that changed how many boxes stand on
 * their goals. The play page's moves and every playback ride the same step. */
function stepWithCue(direction: number): boolean {
  const { game, labels } = loaded();
  const beforeGoals = goalsOn(game, labels);
  const beforePushes = decodeSnapshot(game.snapshot(), labels.length).pushes;
  if (!game.step(direction)) {
    const blocked = audio.moveCue('blocked', direction, 0, 0);
    void sound.playCue(blocked.cue, { pitchOffset: blocked.pitchOffset });
    return false;
  }
  const afterGoals = goalsOn(game, labels);
  const afterPushes = decodeSnapshot(game.snapshot(), labels.length).pushes;
  const kind: audio.MoveKind =
    afterGoals > beforeGoals ? 'goal-enter' : afterGoals < beforeGoals ? 'goal-leave' : afterPushes > beforePushes ? 'push' : 'step';
  const presentation = audio.moveCue(kind, direction, afterGoals, labels.length);
  void sound.playCue(presentation.cue, { pitchOffset: presentation.pitchOffset, variant: presentation.variant });
  return true;
}

/** Whether the WebAssembly engine has started: the play route and the editor's
 * engine verdict both wait for it. */
let engineReady = false;

/** Marks the engine started: start() calls this once the wasm module has
 * initialized. */
function engineArrived() {
  engineReady = true;
}

/** Queues a route the replay shelf asked to play; enterPlay takes it. */
function queueReplay(id: string, route: string) {
  pendingReplay = { id, route };
}

export {
  CUSTOM_PUZZLE_ID,
  GLYPHS,
  RATE_BUTTONS,
  animate,
  askHint,
  board,
  changed,
  customPuzzle,
  engineArrived,
  engineReady,
  enterPlay,
  goalsOn,
  hint,
  invalidate,
  load,
  loaded,
  markRating,
  move,
  paused,
  playback,
  progress,
  queueReplay,
  render,
  saveSession,
  seconds,
  session,
  share,
  solve,
  solver,
  sound,
  tap,
  timer,
  togglePause,
  toggleZen,
  updateButtons,
  verify,
  verifyRoute,
  zoom,
};
