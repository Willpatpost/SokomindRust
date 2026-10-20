import './styles/tokens.css';
import './style.css';
import init, { WasmGame } from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import { BoardView, hue } from './board.ts';
import { bindInput, ignoresKey } from './input.ts';
import {
  DIFFICULTIES,
  ENTRIES,
  daily,
  favorites,
  filter as filterEntries,
  minimap,
  paginate,
  PER_PAGE,
  rate,
  rating,
  solved,
  toggleFavorite,
  type Entry,
  type Rating,
} from './catalog.ts';
import * as storage from './storage.ts';
import * as theme from './theme.ts';
import { formatHash, navigate, route, startRouter, subscribe, type Route } from './router.ts';
import { onPage, playHash, startShell } from './shell.ts';
import { walkRoute } from './walk.ts';
import { PinchZoom } from './zoom.ts';
import { clock, Timer } from './timer.ts';
import { present } from './complete.ts';
import { celebrate } from './celebrate.ts';
import { closeDialog, openDialog } from './dialog.ts';
import { ACTIONS, MAX_ROUTE, MAX_STATES, MODES, errorMessage, type SearchUpdate, type Snapshot, type SolveRequest } from './protocol.ts';
import { ENGINES, SolverClient } from './solver-client.ts';
import { decodeHealth, decodeSnapshot } from './transport.ts';
import { Playback } from './playback.ts';
import { Progress } from './progress.ts';
import * as journal from './journal.ts';
import * as stats from './stats.ts';
import * as achievements from './achievements.ts';
import * as frames from './frames.ts';
import * as journey from './journey.ts';
import { idb } from './idb.ts';
import * as lab from './lab.ts';
import * as optimal from './optimal.ts';
import * as replays from './replays.ts';
import * as traces from './traces.ts';
import { statusText } from './verdict.ts';
import * as editor from './editor.ts';

/** The loaded puzzle. load() replaces it whole; render() refreshes `state`
 * from the game after every change. */
interface Session {
  game: WasmGame;
  puzzle: Entry & { text: string };
  tiles: Uint8Array;
  labels: Uint8Array;
  state: Snapshot;
}
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const button = (id: string) => $<HTMLButtonElement>(id);
const select = (id: string) => $<HTMLSelectElement>(id);
/** The selected value of a select whose options are exactly `values`. */
function choice<T extends string>(id: string, values: readonly T[]): T {
  const value = select(id).value;
  const known = values.find(item => item === value);
  if (known === undefined) throw new Error(`Unknown #${id} option: ${value}`);
  return known;
}
// role=status re-announces every write, so identical text is left alone.
function setText(id: string, value: string) {
  const el = $(id);
  if (el.textContent !== value) el.textContent = value;
}
const message = (value: string) => setText('message', value);
const setStatus = (value: string) => setText('search-status', value);
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
const playback = new Playback(
  direction => loaded().game.step(direction),
  changed,
  blocked => {
    solver.dropRoute();
    updateButtons();
    if (blocked) message('Replay was blocked.');
  },
);
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
  button('undo').disabled = !state || state.moves === 0;
}
function animate(route: string) {
  playback.play(route);
  updateButtons();
}
function invalidate() {
  solver.reset();
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
  const shown = present({ moves: state.moves, pushes: state.pushes }, before, optimal.proven(puzzle.id, puzzle.text));
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
  openDialog('complete');
  celebrate(before === null ? 'first' : shown.eyebrow === 'PERSONAL MILESTONE' ? 'best' : 'clear');
}

/** Toggles zen mode: the board and its controls alone, the chrome folded away. */
function toggleZen() {
  zen = !zen;
  document.querySelector('[data-page="play"]')?.classList.toggle('zen', zen);
  $('zen-dock').hidden = !zen;
}

/** Pauses the attempt: input and the clock stop until P resumes them. */
function togglePause() {
  paused = !paused;
  $('pause').hidden = !paused;
  if (paused) timer.pause();
  else if (!session?.state.solved) timer.run();
}

function changed() {
  render();
  const { game, state } = loaded();
  if (!state.solved) {
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
  paint(next);
  progress.select(next.puzzle.id, next.puzzle.text);
  timer.select(next.puzzle.id);
  // A restored solve holds its time; the clock only runs while playing.
  if (next.state.solved) timer.pause();
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
  if (solver.busy || solver.route !== undefined || playback.active) invalidate();
  if (game.step(direction)) changed();
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
 * star noting the optimum was saved. */
function showResult(update: SearchUpdate, route: string | undefined) {
  setText(
    'solver-result-title',
    update.metrics.status === 'solved' ? 'Route found' : update.metrics.status === 'cancelled' ? 'Search stopped' : 'No route returned',
  );
  setText('solver-result-summary', statusText(update.metrics, route));
  setText('solver-result-guarantee', guarantee(update.metrics));
  $('solver-optimal').hidden = !(update.metrics.proof.kind === 'optimal' && route !== undefined && searchedFromStart);
  $('solver-result').hidden = false;
  if (update.metrics.status !== 'solved' || route === undefined) {
    setText('solver-result-moves', '—');
    setText('solver-result-pushes', '—');
    return;
  }
  const snapshot = verify(loaded().puzzle.text, solver.prefix + route);
  setText('solver-result-moves', snapshot.moves.toLocaleString());
  setText('solver-result-pushes', snapshot.pushes.toLocaleString());
}

/** What a finished search guarantees about its route, in the engine's own
 * words. */
function guarantee(metrics: SearchUpdate['metrics']): string {
  if (metrics.proof.kind === 'optimal') return 'Proven optimal';
  if (metrics.proof.kind === 'unsolvable') return 'Proven unsolvable';
  if (metrics.proof.kind === 'bounded') return `Best found — within ${metrics.proof.upper - metrics.proof.lower} of optimal`;
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
    if (solver.busy || solver.route !== undefined || playback.active) invalidate();
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
 * the attempt is paused, or the cooldown from three misses stands. */
function askHint() {
  if (hint.busy || paused || !session) return;
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

/** The list's local filter state, kept across the family's re-renders. */
let listSearch = '';
let listStatus: 'all' | 'solved' | 'open' | 'favorites' = 'all';

/** Renders the puzzles route family into its body: the tier grid, a tier's
 * collections — or its single collection's list outright — and the list a
 * collection names, with its filters, rows and pagination. An unknown tier or
 * collection lands on the grid. */
function renderPuzzles(next: Route) {
  if (next.kind !== 'puzzles' && next.kind !== 'puzzles-difficulty' && next.kind !== 'puzzles-collection') return;
  const body = $('puzzles-body');
  if (next.kind === 'puzzles') {
    body.replaceChildren(tierGrid());
    return;
  }
  if (!DIFFICULTIES.includes(next.difficulty)) {
    navigate({ kind: 'puzzles' });
    return;
  }
  const tier = filterEntries(ENTRIES, { difficulty: next.difficulty });
  const collections = [...new Set(tier.map(entry => entry.collection))];
  if (next.kind === 'puzzles-difficulty') {
    // A tier of one collection is that collection's list already.
    if (collections.length === 1) {
      body.replaceChildren(puzzleList({ difficulty: next.difficulty, collection: collections[0]! }, next.page, '#/puzzles'));
      return;
    }
    body.replaceChildren(collectionGrid(next.difficulty, collections));
    return;
  }
  if (!collections.includes(next.collection)) {
    navigate({ kind: 'puzzles-difficulty', difficulty: next.difficulty, page: 1 });
    return;
  }
  body.replaceChildren(
    puzzleList({ difficulty: next.difficulty, collection: next.collection }, next.page, `#/puzzles/${encodeURIComponent(next.difficulty)}`),
  );
}

/** The difficulty grid: one card per tier with its progress ring, plus the
 * next unsolved puzzle as a way in. */
function tierGrid(): HTMLElement {
  const grid = document.createElement('div');
  grid.className = 'selector-grid';
  for (const difficulty of DIFFICULTIES) {
    const tier = filterEntries(ENTRIES, { difficulty });
    const cleared = tier.filter(solved).length;
    const card = document.createElement('a');
    card.className = 'grid-card';
    card.href = `#/puzzles/${encodeURIComponent(difficulty)}`;
    card.append(ring(cleared, tier.length));
    const detail = document.createElement('div');
    const name = document.createElement('strong');
    name.textContent = difficulty;
    const line = document.createElement('span');
    line.textContent = `${cleared} of ${tier.length} cleared`;
    const meta = document.createElement('span');
    meta.textContent = `${new Set(tier.map(entry => entry.collection)).size} collections`;
    detail.append(name, line, meta);
    card.append(detail);
    grid.append(card);
  }
  const unsolved = ENTRIES.find(entry => !solved(entry)) ?? ENTRIES[0]!;
  const play = document.createElement('a');
  play.className = 'button primary';
  play.href = playHash(unsolved.id);
  play.textContent = 'Play next unsolved';
  const wrap = document.createElement('div');
  wrap.className = 'grid-wrap';
  wrap.append(grid, play);
  return wrap;
}

/** A tier's collections, one card each, with the way back to the tiers. */
function collectionGrid(difficulty: string, collections: string[]): HTMLElement {
  const grid = document.createElement('div');
  grid.className = 'selector-grid';
  for (const collection of collections) {
    const set = filterEntries(ENTRIES, { difficulty, collection });
    const cleared = set.filter(solved).length;
    const card = document.createElement('a');
    card.className = 'grid-card';
    card.href = `#/puzzles/${encodeURIComponent(difficulty)}/${encodeURIComponent(collection)}`;
    card.append(ring(cleared, set.length));
    const detail = document.createElement('div');
    const name = document.createElement('strong');
    name.textContent = collection;
    const line = document.createElement('span');
    line.textContent = `${cleared} of ${set.length} cleared`;
    detail.append(name, line);
    card.append(detail);
    grid.append(card);
  }
  const wrap = document.createElement('div');
  wrap.className = 'grid-wrap';
  wrap.append(backLink('#/puzzles', 'All difficulties'), grid);
  return wrap;
}

/** The filterable, paginated list of a tier and collection's puzzles. */
function puzzleList(by: { difficulty: string; collection: string }, page: number, back: string): HTMLElement {
  const wrap = document.createElement('div');
  wrap.className = 'list-wrap';
  wrap.append(backLink(back, 'Back'));
  const search = document.createElement('input');
  search.type = 'search';
  search.placeholder = 'Search puzzles';
  search.value = listSearch;
  search.id = 'puzzle-search';
  const chips = document.createElement('div');
  chips.className = 'chips';
  for (const status of ['all', 'solved', 'open', 'favorites'] as const) {
    const chip = document.createElement('button');
    chip.textContent = status === 'all' ? 'All' : status === 'solved' ? 'Cleared' : status === 'open' ? 'Open' : 'Favorites';
    chip.classList.toggle('on', status === listStatus);
    chip.onclick = () => {
      listStatus = status;
      navigate({ kind: 'puzzles-collection', ...by, page: 1 });
      // The same-route navigate emits nothing, so the list re-renders here.
      renderPuzzles(route());
    };
    chips.append(chip);
  }
  const bar = document.createElement('div');
  bar.className = 'list-bar';
  bar.append(search, chips);
  wrap.append(bar);
  const kept = filterEntries(ENTRIES, { ...by, search: listSearch, status: listStatus });
  const at = paginate(kept, page);
  const summary = document.createElement('p');
  summary.className = 'muted list-summary';
  const first = kept.length === 0 ? 0 : (at.page - 1) * PER_PAGE + 1;
  summary.textContent =
    kept.length === 0 ? 'No puzzles match.' : `Showing ${first}–${first + at.slice.length - 1} of ${kept.length} puzzles`;
  wrap.append(summary);
  const list = document.createElement('div');
  list.className = 'puzzle-list';
  for (const entry of at.slice) list.append(puzzleRow(entry));
  wrap.append(list);
  if (at.pages > 1) {
    const pages = document.createElement('div');
    pages.className = 'chips pages';
    for (let number = 1; number <= at.pages; number++) {
      const link = document.createElement('a');
      link.className = `chip${number === at.page ? ' on' : ''}`;
      link.href = `#/puzzles/${encodeURIComponent(by.difficulty)}/${encodeURIComponent(by.collection)}${number === 1 ? '' : `?page=${number}`}`;
      link.textContent = String(number);
      pages.append(link);
    }
    wrap.append(pages);
  }
  // The search re-renders the list as it narrows; the focus returns to it.
  search.oninput = () => {
    listSearch = search.value;
    renderPuzzles(route());
    $('puzzle-search')?.focus();
  };
  return wrap;
}

/** One list row: the minimap, the names and numbers, the best, the heart. */
function puzzleRow(entry: Entry): HTMLElement {
  const row = document.createElement('a');
  row.className = 'puzzle-row';
  row.href = playHash(entry.id);
  row.append(minimap(entry));
  const detail = document.createElement('div');
  detail.className = 'row-detail';
  const name = document.createElement('strong');
  name.textContent = entry.title;
  const meta = document.createElement('span');
  const best = storage.best(entry.id, entry.rows.join('\n'));
  meta.textContent = `${entry.width}×${entry.height} · ${entry.boxes} boxes${best === null ? '' : ` · best ${best.moves} moves`}`;
  detail.append(name, meta);
  row.append(detail);
  // The felt-difficulty mark the rating store holds, colored by its verdict.
  const felt = rating(entry.id);
  if (felt !== undefined) {
    const dot = document.createElement('span');
    dot.className = 'rating-dot';
    dot.dataset.rating = felt;
    dot.title = felt === 'easy' ? 'Felt easy' : felt === 'right' ? 'Just right' : 'Felt hard';
    row.append(dot);
  }
  const heart = document.createElement('button');
  const favorite = favorites().has(entry.id);
  heart.className = `heart${favorite ? ' on' : ''}`;
  heart.textContent = favorite ? '♥' : '♡';
  heart.title = 'Favorite';
  heart.onclick = event => {
    event.preventDefault();
    event.stopPropagation();
    toggleFavorite(entry.id);
    renderPuzzles(route());
  };
  row.append(heart);
  return row;
}

/** The progress ring a grid card shows: the cleared share of a donut. */
function ring(cleared: number, total: number): SVGSVGElement {
  const radius = 19;
  const circ = 2 * Math.PI * radius;
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 44 44');
  svg.classList.add('ring');
  const arcs: [string, number][] = [
    ['ring-back', 0],
    ['ring-arc', circ * (1 - (total === 0 ? 0 : cleared / total))],
  ];
  for (const [kind, offset] of arcs) {
    const circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
    circle.setAttribute('cx', '22');
    circle.setAttribute('cy', '22');
    circle.setAttribute('r', String(radius));
    circle.setAttribute('class', kind);
    circle.style.strokeDasharray = String(circ);
    circle.style.strokeDashoffset = String(offset);
    svg.append(circle);
  }
  return svg;
}

/** A small way-back link. */
function backLink(href: string, text: string): HTMLElement {
  const link = document.createElement('a');
  link.className = 'back';
  link.href = href;
  link.textContent = `← ${text}`;
  return link;
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

/** Renders the home dashboard and aims its primary action: the cleared
 * progress with its bar, the tier row, the solving streak, today's daily
 * challenge with its run and history, the recently cleared, and the saved
 * session's puzzle as the way back in. */
function syncHome() {
  const saved = storage.session();
  const id = saved && ENTRIES.some(p => p.id === saved.id) ? saved.id : ENTRIES[0]!.id;
  const start = $<HTMLAnchorElement>('start');
  start.href = playHash(id);
  start.textContent = saved ? 'Resume playing' : 'Start playing';
  const cleared = ENTRIES.filter(solved).length;
  const summary = document.createElement('p');
  summary.className = 'muted';
  summary.textContent = `${cleared} of ${ENTRIES.length} rooms cleared`;
  const bar = document.createElement('div');
  bar.className = 'progress-track';
  const fill = document.createElement('div');
  fill.className = 'progress-fill';
  fill.style.width = `${Math.round((cleared / ENTRIES.length) * 100)}%`;
  bar.append(fill);
  const tiers = document.createElement('div');
  tiers.className = 'tier-row';
  for (const difficulty of DIFFICULTIES) {
    const tier = filterEntries(ENTRIES, { difficulty });
    const link = document.createElement('a');
    link.href = `#/puzzles/${encodeURIComponent(difficulty)}`;
    const name = document.createElement('strong');
    name.textContent = difficulty;
    const count = document.createElement('span');
    count.textContent = `${tier.filter(solved).length}/${tier.length}`;
    link.append(name, count);
    tiers.append(link);
  }
  const book = journal.load();
  const parts: HTMLElement[] = [summary, bar, tiers];
  const streak = journal.solvingStreak(book);
  if (streak.current > 0) {
    const banner = document.createElement('div');
    banner.className = 'streak-banner';
    const flame = document.createElement('span');
    flame.className = 'flame';
    flame.setAttribute('aria-hidden', 'true');
    flame.textContent = '🔥';
    const detail = document.createElement('div');
    const current = document.createElement('strong');
    current.textContent = `${streak.current}-day streak`;
    detail.append(current);
    if (streak.longest > streak.current) {
      const longest = document.createElement('span');
      longest.textContent = `Best: ${streak.longest} days`;
      detail.append(longest);
    }
    banner.append(flame, detail);
    parts.push(banner);
  }
  const challenge = daily();
  const view = journal.dailyView(book);
  const card = document.createElement('div');
  card.className = 'card daily';
  card.dataset.state = view.state;
  const topline = document.createElement('p');
  topline.className = 'daily-top';
  const label = document.createElement('span');
  label.textContent = 'Daily challenge';
  const run = document.createElement('span');
  run.textContent = view.streak > 0 ? `${view.streak}-day run` : 'New room daily';
  topline.append(label, run);
  const title = document.createElement('h2');
  title.textContent = challenge.title;
  const framing = document.createElement('p');
  framing.className = 'muted';
  framing.textContent =
    view.state === 'cleared'
      ? 'Today is recorded. Replay the room to refine your route, or return tomorrow for a new one.'
      : view.state === 'recovery'
        ? 'A missed day does not close the path. This room starts a fresh run today.'
        : 'One shared room each day. Solve it before the day rolls over to add today to your run.';
  const play = document.createElement('a');
  play.className = 'button';
  play.href = playHash(challenge.id);
  play.textContent = view.state === 'cleared' ? 'Replay today’s room' : 'Play today’s room';
  card.append(topline, title, framing, play);
  const history = document.createElement('ol');
  history.className = 'daily-history';
  history.setAttribute('aria-label', 'Seven-day daily history');
  for (const day of view.history) {
    const item = document.createElement('li');
    item.dataset.outcome = day.outcome;
    item.setAttribute('aria-label', `${day.label}: ${day.outcome === 'today' ? 'open today' : day.outcome}`);
    const dot = document.createElement('span');
    dot.className = 'dot';
    dot.setAttribute('aria-hidden', 'true');
    dot.textContent = day.outcome === 'cleared' ? '✓' : day.outcome === 'today' ? '•' : '';
    const name = document.createElement('span');
    name.textContent = day.label;
    item.append(dot, name);
    history.append(item);
  }
  card.append(history);
  parts.push(card);
  parts.push(journeyCard(book));
  const recent = journal
    .recentlyCleared(book)
    .map(clear => ({ clear, entry: ENTRIES.find(p => p.id === clear.id) }))
    .filter((row): row is { clear: journal.RecentClear; entry: Entry } => row.entry !== undefined);
  if (recent.length > 0) {
    const strip = document.createElement('div');
    strip.className = 'recent';
    const heading = document.createElement('p');
    heading.className = 'muted';
    heading.textContent = 'Recently cleared';
    const list = document.createElement('div');
    list.className = 'recent-list';
    for (const { clear, entry } of recent) {
      const link = document.createElement('a');
      link.href = playHash(entry.id);
      const name = document.createElement('strong');
      name.textContent = entry.title;
      const meta = document.createElement('span');
      meta.textContent = `${clear.moves}m / ${clear.pushes}p · ${journal.timeAgo(clear.clearedAt)}`;
      link.append(name, meta);
      list.append(link);
    }
    strip.append(heading, list);
    parts.push(strip);
  }
  $('home-dash').replaceChildren(...parts);
}

/** The home page's guided journey card: five concept chapters with their
 * rooms, the next suggestion, and the pause that folds it away. */
function journeyCard(book: journal.Journal): HTMLElement {
  const card = document.createElement('section');
  card.className = 'card journey';
  if (journey.dismissed()) {
    const eyebrow = document.createElement('p');
    eyebrow.className = 'eyebrow';
    eyebrow.textContent = 'GUIDED PATH PAUSED';
    const title = document.createElement('h2');
    title.textContent = 'Explore your own way';
    const note = document.createElement('p');
    note.className = 'muted';
    note.textContent = 'Your chapter progress is safe. Resume whenever you want a suggestion.';
    const resume = document.createElement('button');
    resume.textContent = 'Resume guided path';
    resume.onclick = () => {
      journey.setDismissed(false);
      syncHome();
    };
    card.append(eyebrow, title, note, resume);
    return card;
  }
  const chapters = journey.chapterProgress(book);
  const done = chapters.reduce((sum, chapter) => sum + chapter.solved, 0);
  const total = chapters.reduce((sum, chapter) => sum + chapter.total, 0);
  const header = document.createElement('div');
  header.className = 'journey-head';
  const lead = document.createElement('div');
  const eyebrow = document.createElement('p');
  eyebrow.className = 'eyebrow';
  eyebrow.textContent = `Optional journey · ${done}/${total}`;
  const title = document.createElement('h2');
  title.textContent = 'Build your Sokoban instincts';
  const note = document.createElement('p');
  note.className = 'muted';
  note.textContent = 'Five concept chapters offer direction, never gates.';
  lead.append(eyebrow, title, note);
  const pause = document.createElement('button');
  pause.className = 'journey-pause';
  pause.textContent = 'Pause';
  pause.setAttribute('aria-label', 'Pause guided path');
  pause.onclick = () => {
    journey.setDismissed(true);
    syncHome();
  };
  header.append(lead, pause);
  card.append(header);
  const suggestion = journey.recommendation(book);
  const row = document.createElement('div');
  row.className = 'journey-next';
  const copy = document.createElement('div');
  const where = document.createElement('span');
  const room = document.createElement('strong');
  const why = document.createElement('p');
  why.className = 'muted';
  const go = document.createElement('a');
  go.className = 'button';
  if (suggestion !== null) {
    where.textContent = `Suggested next · ${suggestion.chapterTitle}`;
    room.textContent = ENTRIES.find(entry => entry.id === suggestion.roomId)?.title ?? suggestion.roomId;
    why.textContent = suggestion.reason;
    go.href = playHash(suggestion.roomId);
    go.textContent = 'Play suggestion';
  } else {
    where.textContent = 'Journey complete';
    room.textContent = 'Every chapter cleared';
    why.textContent = 'Your next direction is yours: revisit a favorite route or explore the full catalog.';
    go.href = '#/puzzles';
    go.textContent = 'Browse catalog';
  }
  copy.append(where, room, why);
  row.append(copy, go);
  card.append(row);
  const map = document.createElement('ol');
  map.className = 'journey-map';
  map.setAttribute('aria-label', 'Guided journey chapters');
  for (const chapter of chapters) {
    const item = document.createElement('li');
    item.className = 'chapter';
    if (chapter.complete) item.dataset.complete = 'true';
    const heading = document.createElement('div');
    heading.className = 'chapter-head';
    const number = document.createElement('span');
    number.className = 'chapter-number';
    number.textContent = String(chapter.number);
    const named = document.createElement('div');
    const name = document.createElement('h3');
    name.textContent = chapter.title;
    const concept = document.createElement('p');
    concept.textContent = chapter.concept;
    named.append(name, concept);
    const count = document.createElement('span');
    count.className = 'chapter-count';
    count.textContent = `${chapter.solved}/${chapter.total}`;
    heading.append(number, named, count);
    const explanation = document.createElement('p');
    explanation.className = 'muted';
    explanation.textContent = chapter.explanation;
    item.append(heading, explanation);
    if (chapter.requires.length > 0 && !chapter.ready) {
      const wait = document.createElement('p');
      wait.className = 'chapter-wait';
      const required = chapter.requires[0]!;
      wait.textContent = `Recommended first: ${chapters.find(entry => entry.id === required)?.title ?? required}`;
      item.append(wait);
    }
    const nodes = document.createElement('div');
    nodes.className = 'chapter-nodes';
    nodes.setAttribute('aria-label', `${chapter.title} rooms`);
    for (const [index, id] of chapter.rooms.entries()) {
      const node = document.createElement('a');
      node.className = 'node';
      const entry = ENTRIES.find(candidate => candidate.id === id);
      const cleared = book.cleared[id] !== undefined || (entry !== undefined && solved(entry));
      if (cleared) node.dataset.complete = 'true';
      node.href = playHash(id);
      node.textContent = cleared ? '✓' : String(index + 1);
      node.title = entry?.title ?? id;
      node.setAttribute('aria-label', `${entry?.title ?? id}${cleared ? ', cleared' : ', not cleared'}`);
      nodes.append(node);
    }
    item.append(nodes);
    map.append(item);
  }
  card.append(map);
  const browse = document.createElement('a');
  browse.className = 'journey-browse';
  browse.href = '#/puzzles';
  browse.textContent = `Browse all ${ENTRIES.length} rooms`;
  card.append(browse);
  return card;
}

/** Renders the stats route: the aggregates over the journal and the stored
 * bests, the activity heat map, and the progress tools — copy, export,
 * import with its merge summary, and the reset behind three steps. */
function renderStats() {
  const body = $('stats-body');
  const book = journal.load();
  const a = stats.aggregates(book);
  // The tools below can change what the board wears; keep the frame current.
  frames.apply();
  const note = (text: string) => {
    const live = document.getElementById('stats-status');
    if (live) live.textContent = text;
  };

  const tools = document.createElement('div');
  tools.className = 'stats-tools';
  const copy = document.createElement('button');
  copy.id = 'stats-copy';
  copy.textContent = 'Copy stats';
  copy.onclick = async () => {
    try {
      await navigator.clipboard.writeText(stats.statsText(a));
      copy.textContent = 'Copied!';
      setTimeout(() => {
        copy.textContent = 'Copy stats';
      }, 2000);
    } catch {
      note('Clipboard is unavailable in this browser.');
    }
  };
  const exportProgress = document.createElement('button');
  exportProgress.id = 'stats-export';
  exportProgress.textContent = 'Export progress';
  exportProgress.onclick = () => {
    const blob = new Blob([JSON.stringify(book, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `sokomind-progress-${journal.dayKey(new Date())}.json`;
    link.click();
    URL.revokeObjectURL(url);
  };
  const importLabel = document.createElement('button');
  importLabel.id = 'stats-import';
  importLabel.textContent = 'Import progress';
  const file = document.createElement('input');
  file.id = 'stats-file';
  file.type = 'file';
  file.accept = '.json,application/json';
  file.hidden = true;
  importLabel.onclick = () => file.click();
  file.onchange = async () => {
    const chosen = file.files?.[0];
    file.value = '';
    if (!chosen) return;
    const result = journal.imported(await chosen.text());
    if (!result.ok) {
      note(result.error);
      return;
    }
    const current = journal.load();
    const summary = journal.summarizeMerge(current, result.journal);
    storage.write('journal', journal.merge(current, result.journal));
    renderStats();
    note(
      `Imported: ${summary.added} added, ${summary.improved} improved, ${summary.unchanged} unchanged, `
        + `${summary.rejected + result.rejected} rejected, ${result.invalid} invalid.`,
    );
  };
  const status = document.createElement('p');
  status.id = 'stats-status';
  status.setAttribute('role', 'status');
  tools.append(copy, exportProgress, importLabel, file, status);

  if (a.solved === 0) {
    const empty = document.createElement('p');
    empty.className = 'muted';
    empty.textContent = 'Solve some rooms to fill in your statistics. An import still works.';
    body.replaceChildren(tools, empty, showcase(a, book));
    return;
  }

  const card = (label: string, big: string, sub?: string, bigId?: string): HTMLElement => {
    const cardEl = document.createElement('div');
    cardEl.className = 'card stat-card';
    const eyebrow = document.createElement('p');
    eyebrow.className = 'eyebrow';
    eyebrow.textContent = label;
    const value = document.createElement('span');
    value.className = 'stat-big';
    if (bigId !== undefined) value.id = bigId;
    value.textContent = big;
    cardEl.append(eyebrow, value);
    if (sub !== undefined) {
      const line = document.createElement('span');
      line.className = 'stat-sub';
      line.textContent = sub;
      cardEl.append(line);
    }
    return cardEl;
  };
  const grid = document.createElement('div');
  grid.className = 'stats-grid';
  grid.append(
    card('Rooms solved', String(a.solved), `of ${a.total} (${Math.round(a.completion)}%)`, 'stats-solved'),
    card('Total moves', a.moves.toLocaleString(), `${a.pushes.toLocaleString()} pushes`),
    card('Avg pushes per room', a.averagePushes.toFixed(1)),
    card(
      'Streak',
      String(a.streak.current),
      `${a.streak.current === 1 ? 'day' : 'days'} current${a.streak.longest > 0 ? ` · ${a.streak.longest} best` : ''}`,
    ),
    a.elapsedMs > 0
      ? card('Time played', stats.formatDuration(a.elapsedMs))
      : card('Estimated time played', stats.formatDuration(a.recorded * 90_000), '~90s per room average'),
  );
  if (a.efficiency)
    grid.append(card('Best efficiency', `${a.efficiency.pushes}/${a.efficiency.boxes}`, `pushes/boxes on ${a.efficiency.title}`));

  // The donut: one arc per solved tier, the count at its center, a legend beside it.
  const donutCard = document.createElement('div');
  donutCard.className = 'card stat-card wide';
  const donutLabel = document.createElement('p');
  donutLabel.className = 'eyebrow';
  donutLabel.textContent = 'Solved by difficulty';
  const donutRow = document.createElement('div');
  donutRow.className = 'donut-row';
  const donut = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  donut.setAttribute('viewBox', '0 0 100 100');
  donut.classList.add('donut');
  const circumference = 2 * Math.PI * 40;
  const solvedTiers = a.byDifficulty.filter(tier => tier.solved > 0);
  let offset = 0;
  for (const tier of solvedTiers) {
    const dash = (tier.solved / a.solved) * circumference;
    const segment = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
    segment.setAttribute('cx', '50');
    segment.setAttribute('cy', '50');
    segment.setAttribute('r', '40');
    segment.setAttribute('fill', 'none');
    segment.setAttribute('stroke', stats.TIER_COLORS[tier.difficulty] ?? 'var(--ink-700)');
    segment.setAttribute('stroke-width', '16');
    segment.setAttribute('stroke-dasharray', `${dash} ${circumference - dash}`);
    segment.setAttribute('stroke-dashoffset', String(-offset));
    segment.setAttribute('transform', 'rotate(-90 50 50)');
    offset += dash;
    donut.append(segment);
  }
  const count = document.createElementNS('http://www.w3.org/2000/svg', 'text');
  count.setAttribute('x', '50');
  count.setAttribute('y', '47');
  count.setAttribute('text-anchor', 'middle');
  count.setAttribute('class', 'donut-center');
  count.textContent = String(a.solved);
  const word = document.createElementNS('http://www.w3.org/2000/svg', 'text');
  word.setAttribute('x', '50');
  word.setAttribute('y', '59');
  word.setAttribute('text-anchor', 'middle');
  word.setAttribute('class', 'donut-label');
  word.textContent = 'solved';
  donut.append(count, word);
  const legend = document.createElement('div');
  legend.className = 'donut-legend';
  for (const tier of solvedTiers) {
    const row = document.createElement('div');
    row.className = 'legend-item';
    const dot = document.createElement('span');
    dot.className = 'legend-dot';
    dot.style.background = stats.TIER_COLORS[tier.difficulty] ?? 'var(--ink-700)';
    const name = document.createElement('span');
    name.textContent = tier.difficulty;
    const value = document.createElement('span');
    value.className = 'legend-value';
    value.textContent = String(tier.solved);
    row.append(dot, name, value);
    legend.append(row);
  }
  donutRow.append(donut, legend);
  donutCard.append(donutLabel, donutRow);
  grid.append(donutCard);

  // The bars: each tier's completion, the track sized by the tier's room count.
  const barsCard = document.createElement('div');
  barsCard.className = 'card stat-card wide';
  const barsLabel = document.createElement('p');
  barsLabel.className = 'eyebrow';
  barsLabel.textContent = 'Completion by difficulty';
  const bars = document.createElement('div');
  bars.className = 'bar-chart';
  const maxTotal = Math.max(...a.byDifficulty.map(tier => tier.total), 1);
  for (const tier of a.byDifficulty) {
    const row = document.createElement('div');
    row.className = 'bar-row';
    const name = document.createElement('span');
    name.className = 'bar-label';
    name.textContent = tier.difficulty;
    const track = document.createElement('div');
    track.className = 'bar-track';
    track.style.width = `${Math.round((tier.total / maxTotal) * 100)}%`;
    const fill = document.createElement('span');
    fill.className = 'bar-fill';
    fill.style.width = `${tier.total === 0 ? 0 : Math.round((tier.solved / tier.total) * 100)}%`;
    fill.style.background = stats.TIER_COLORS[tier.difficulty] ?? 'var(--ink-700)';
    track.append(fill);
    const value = document.createElement('span');
    value.className = 'bar-value';
    value.textContent = `${tier.solved}/${tier.total}`;
    row.append(name, track, value);
    bars.append(row);
  }
  barsCard.append(barsLabel, bars);
  grid.append(barsCard);

  // The heat map: the last 90 days, one cell per day, five levels deep.
  const heatCard = document.createElement('div');
  heatCard.className = 'card stat-card wide';
  const heatLabel = document.createElement('p');
  heatLabel.className = 'eyebrow';
  heatLabel.textContent = 'Activity — last 90 days';
  const cells = journal.heat(book);
  const heatGrid = document.createElement('div');
  heatGrid.className = 'heat-grid';
  heatGrid.setAttribute('role', 'img');
  heatGrid.setAttribute('aria-label', `Activity over the last 90 days: ${cells.reduce((sum, cell) => sum + cell.count, 0)} rooms cleared`);
  for (const cell of cells) {
    const day = document.createElement('span');
    day.className = 'heat-cell';
    const level = stats.heatLevel(cell.count);
    if (level > 0) day.dataset.level = String(level);
    day.title = `${cell.key}: ${cell.count} cleared`;
    heatGrid.append(day);
  }
  const heatLegend = document.createElement('div');
  heatLegend.className = 'heat-legend';
  const less = document.createElement('span');
  less.className = 'heat-word';
  less.textContent = 'Less';
  heatLegend.append(less);
  for (let level = 0; level <= 4; level++) {
    const step = document.createElement('span');
    step.className = 'heat-cell';
    if (level > 0) step.dataset.level = String(level);
    heatLegend.append(step);
  }
  const more = document.createElement('span');
  more.className = 'heat-word';
  more.textContent = 'More';
  heatLegend.append(more);
  heatCard.append(heatLabel, heatGrid, heatLegend);
  grid.append(heatCard);

  // The improvement lab: recent personal-best routes, replay-verified, each
  // playable again from its fresh board.
  const lab = document.createElement('div');
  lab.className = 'card stat-card wide';
  const labEyebrow = document.createElement('p');
  labEyebrow.className = 'eyebrow';
  labEyebrow.textContent = 'IMPROVEMENT LAB';
  const labTitle = document.createElement('h3');
  labTitle.textContent = 'Personal-best replays';
  const labCount = document.createElement('span');
  labCount.className = 'showcase-tally';
  const labState = document.createElement('p');
  labState.className = 'muted';
  labState.textContent = 'Finding recent verified routes…';
  lab.append(labEyebrow, labTitle, labCount, labState);
  grid.append(lab);
  void (async () => {
    const found = await replays.index();
    const items: { entry: Entry; routes: replays.Route[] }[] = [];
    for (const id of found.ids) {
      if (items.length >= 6) break;
      const entry = ENTRIES.find(candidate => candidate.id === id);
      if (entry === undefined) continue;
      const read = await replays.read(idb, id, entry.rows.join('\n'), verifyRoute);
      if (read.status === 'ready' && read.routes.length > 0) items.push({ entry, routes: read.routes });
    }
    if (!found.available) {
      labState.textContent = 'Replay storage is unavailable. Your summary statistics still work normally.';
      return;
    }
    if (items.length === 0) {
      labState.textContent = 'No replay routes are available yet. Clear a room again to capture its route.';
      return;
    }
    labCount.textContent = `${items.length} ready`;
    labState.hidden = true;
    const labList = document.createElement('ul');
    labList.className = 'shelf-list';
    for (const { entry, routes } of items) {
      const best = routes[0];
      if (best === undefined) continue;
      const item = document.createElement('li');
      const detail = document.createElement('div');
      const name = document.createElement('strong');
      name.textContent = entry.title;
      const meta = document.createElement('span');
      meta.textContent = `${best.moves} moves · ${best.pushes} pushes · ${routes.length} ${routes.length === 1 ? 'route' : 'routes'}`;
      const saved = document.createElement('small');
      saved.textContent = `Saved ${achievements.milestoneLabel(best.completedAt)}`;
      detail.append(name, meta, saved);
      const play = document.createElement('button');
      play.textContent = 'Replay';
      play.onclick = () => {
        pendingReplay = { id: entry.id, route: best.actionLog };
        navigate({ kind: 'play', puzzleId: entry.id });
      };
      const studyButton = document.createElement('button');
      studyButton.textContent = 'Study';
      studyButton.onclick = () => openStudy(entry);
      item.append(detail, play, studyButton);
      labList.append(item);
    }
    lab.append(labList);
  })();

  // The danger zone: the reset asks three times, the last by name.
  const danger = document.createElement('div');
  danger.className = 'card stat-card wide danger';
  const dangerLabel = document.createElement('p');
  dangerLabel.className = 'eyebrow';
  dangerLabel.textContent = 'Danger zone';
  const zone = document.createElement('div');
  zone.className = 'danger-zone';
  const draw = (step: 'idle' | 'confirm' | 'typing') => {
    zone.replaceChildren();
    if (step === 'idle') {
      const start = document.createElement('button');
      start.id = 'stats-reset';
      start.textContent = 'Reset all progress';
      start.onclick = () => draw('confirm');
      zone.append(start);
      return;
    }
    if (step === 'confirm') {
      const warning = document.createElement('p');
      warning.className = 'muted';
      warning.textContent =
        'This permanently deletes your solved rooms, statistics and daily history from this browser. Your theme, favorites and ratings stay.';
      const go = document.createElement('button');
      go.textContent = 'I understand, continue';
      go.onclick = () => draw('typing');
      const cancel = document.createElement('button');
      cancel.textContent = 'Cancel';
      cancel.onclick = () => draw('idle');
      zone.append(warning, go, cancel);
      return;
    }
    const ask = document.createElement('p');
    ask.className = 'muted';
    const strong = document.createElement('strong');
    strong.textContent = 'RESET';
    ask.append('Type ', strong, ' to confirm:');
    const input = document.createElement('input');
    input.id = 'stats-reset-input';
    input.type = 'text';
    input.spellcheck = false;
    const go = document.createElement('button');
    go.id = 'stats-reset';
    go.textContent = 'Permanently reset progress';
    go.disabled = true;
    input.oninput = () => {
      go.disabled = input.value !== 'RESET';
    };
    go.onclick = () => {
      journal.reset();
      for (const name of storage.names()) if (name.startsWith('best.')) storage.remove(name);
      // The replay shelf follows the solved record down; the optimal cache
      // stays, being the engine's knowledge rather than progress.
      void replays.clear();
      // The reset can unearn the frame the board wears.
      frames.apply();
      navigate({ kind: 'home' });
    };
    const cancel = document.createElement('button');
    cancel.textContent = 'Cancel';
    cancel.onclick = () => draw('idle');
    zone.append(ask, input, go, cancel);
  };
  draw('idle');
  danger.append(dangerLabel, zone);
  grid.append(danger);

  body.replaceChildren(tools, grid, showcase(a, book));
}

/** The stats page's progression showcase: the achievements in their
 * collections with their progress, the milestones behind them, and the
 * board-frame keepsakes an achievement unlocks. */
function showcase(a: stats.Aggregates, book: journal.Journal): HTMLElement {
  const section = document.createElement('section');
  section.className = 'showcase';
  const intro = document.createElement('div');
  intro.className = 'showcase-intro';
  const lead = document.createElement('div');
  const eyebrow = document.createElement('p');
  eyebrow.className = 'eyebrow';
  eyebrow.textContent = 'LONG-TERM PROGRESS';
  const title = document.createElement('h2');
  title.textContent = 'Achievements and keepsakes';
  const note = document.createElement('p');
  note.className = 'muted';
  note.textContent = 'Every requirement is visible and evaluated from your saved solves. Rewards never lock rooms or change board rules.';
  lead.append(eyebrow, title, note);
  const earned = achievements.unlocked(a, book);
  const tally = document.createElement('strong');
  tally.textContent = `${earned.length}/${achievements.ACHIEVEMENTS.length} earned`;
  intro.append(lead, tally);
  section.append(intro);

  // Recent milestones: the path behind you, dated by the records.
  const marks = achievements.milestones(a, book);
  const recent = document.createElement('div');
  recent.className = 'showcase-block';
  const recentHead = document.createElement('div');
  const recentEyebrow = document.createElement('p');
  recentEyebrow.className = 'eyebrow';
  recentEyebrow.textContent = 'RECENT MILESTONES';
  const recentTitle = document.createElement('h3');
  recentTitle.textContent = 'The path behind you';
  recentHead.append(recentEyebrow, recentTitle);
  const recentNote = document.createElement('span');
  recentNote.className = 'muted';
  recentNote.textContent = 'Based on saved completion dates';
  recent.append(recentHead, recentNote);
  if (marks.length > 0) {
    const list = document.createElement('ol');
    list.className = 'milestone-list';
    for (const mark of marks) {
      const item = document.createElement('li');
      const tick = document.createElement('span');
      tick.className = 'milestone-mark';
      tick.setAttribute('aria-hidden', 'true');
      tick.textContent = '✓';
      const detail = document.createElement('div');
      const name = document.createElement('strong');
      name.textContent = mark.title;
      const when = document.createElement('span');
      when.textContent = `${mark.collectionTitle} · ${achievements.milestoneLabel(mark.earnedAt)}`;
      const what = document.createElement('small');
      what.textContent = mark.description;
      detail.append(name, when, what);
      item.append(tick, detail);
      list.append(item);
    }
    recent.append(list);
  } else {
    const none = document.createElement('p');
    none.className = 'muted';
    none.textContent = 'Your first solved room will place a milestone here.';
    recent.append(none);
  }
  section.append(recent);

  // The collections, one block per group of achievements.
  const earnedIds = new Set(earned.map(def => def.id));
  const collections = document.createElement('div');
  collections.className = 'showcase-collections';
  for (const collection of achievements.COLLECTIONS) {
    const defs = achievements.ACHIEVEMENTS.filter(def => def.collection === collection.id);
    const block = document.createElement('section');
    const head = document.createElement('div');
    const name = document.createElement('h3');
    name.textContent = collection.title;
    const blurb = document.createElement('p');
    blurb.className = 'muted';
    blurb.textContent = collection.description;
    head.append(name, blurb);
    const count = document.createElement('span');
    count.className = 'showcase-tally';
    count.textContent = `${defs.filter(def => earnedIds.has(def.id)).length}/${defs.length}`;
    block.append(head, count);
    const list = document.createElement('ul');
    list.className = 'achievement-list';
    for (const def of defs) {
      const progress = def.progress(a, book);
      const item = document.createElement('li');
      if (progress.complete) item.dataset.earned = 'true';
      const icon = document.createElement('span');
      icon.className = 'achievement-icon';
      icon.setAttribute('aria-hidden', 'true');
      icon.textContent = progress.complete ? '✓' : def.icon;
      const detail = document.createElement('div');
      const top = document.createElement('div');
      const label = document.createElement('strong');
      label.textContent = def.title;
      const standing = document.createElement('span');
      standing.textContent = progress.complete ? 'Earned' : 'In progress';
      top.append(label, standing);
      const what = document.createElement('p');
      what.textContent = def.description;
      const track = document.createElement('div');
      track.className = 'progress-track';
      track.setAttribute('role', 'progressbar');
      track.setAttribute('aria-label', `${def.title}: ${progress.label}`);
      track.setAttribute('aria-valuemin', '0');
      track.setAttribute('aria-valuemax', String(progress.target));
      track.setAttribute('aria-valuenow', String(progress.current));
      const fill = document.createElement('span');
      fill.className = 'progress-fill';
      fill.style.width = `${Math.round((progress.current / progress.target) * 100)}%`;
      track.append(fill);
      const label2 = document.createElement('small');
      label2.textContent = progress.label;
      detail.append(top, what, track, label2);
      item.append(icon, detail);
      list.append(item);
    }
    block.append(list);
    collections.append(block);
  }
  section.append(collections);

  // The board-frame keepsakes: choose a quiet finishing touch.
  const keepsakes = document.createElement('div');
  keepsakes.className = 'showcase-block';
  const keepsakeHead = document.createElement('div');
  const keepsakeEyebrow = document.createElement('p');
  keepsakeEyebrow.className = 'eyebrow';
  keepsakeEyebrow.textContent = 'BOARD-FRAME KEEPSAKES';
  const keepsakeTitle = document.createElement('h3');
  keepsakeTitle.textContent = 'Choose a quiet finishing touch';
  keepsakeHead.append(keepsakeEyebrow, keepsakeTitle);
  const keepsakeNote = document.createElement('span');
  keepsakeNote.className = 'muted';
  keepsakeNote.textContent = 'Frame and shadow only';
  keepsakes.append(keepsakeHead, keepsakeNote);
  const never = document.createElement('p');
  never.className = 'muted';
  never.textContent = 'These never recolor tiles, goals, crates, the robot, warnings, or focus indicators.';
  keepsakes.append(never);
  const picks = document.createElement('div');
  picks.className = 'keepsake-grid';
  picks.setAttribute('role', 'group');
  picks.setAttribute('aria-label', 'Board frame cosmetic');
  for (const state of frames.states(frames.preference(), theme.preferences().themeFamily, a, book)) {
    const usable = state.unlocked && state.compatible;
    const pick = document.createElement('button');
    pick.className = 'keepsake';
    pick.dataset.frame = state.id;
    if (state.active) pick.dataset.active = 'true';
    if (!usable) pick.dataset.locked = 'true';
    pick.setAttribute('aria-pressed', String(state.active));
    pick.disabled = !usable;
    pick.onclick = () => {
      frames.save(state.id);
      renderStats();
    };
    const preview = document.createElement('span');
    preview.className = 'keepsake-preview';
    preview.setAttribute('aria-hidden', 'true');
    const chip = document.createElement('span');
    preview.append(chip);
    const copy = document.createElement('span');
    copy.className = 'keepsake-copy';
    const name = document.createElement('strong');
    name.textContent = state.title;
    const blurb = document.createElement('small');
    blurb.textContent = state.description;
    const standing = document.createElement('em');
    standing.textContent = !state.unlocked
      ? state.requirement
      : !state.compatible
        ? `Use with ${state.families.map(family => family.replaceAll('-', ' ')).join(' or ')}`
        : state.active
          ? 'Active'
          : 'Available';
    copy.append(name, blurb, standing);
    pick.append(preview, copy);
    picks.append(pick);
  }
  keepsakes.append(picks);
  section.append(keepsakes);
  return section;
}

/** The replay study: the dialog over a room's verified routes, watched step
 * by step on the board, with the comparison of two attempts. */
interface StudyRoute {
  label: string;
  route: replays.Route;
  trace: traces.Trace;
}

const studyBoard = new BoardView($('replay-board'));
let study:
  | { room: Entry; routes: StudyRoute[]; watch: number; compare: number; step: number; playing: boolean; speed: number; ghost: boolean }
  | undefined;
let studyTimer: number | undefined;

/** Opens the replay study for a room: its verified routes rebuilt on the
 * engine frame by frame, best first. */
function openStudy(entry: Entry) {
  void (async () => {
    const rows = entry.rows.join('\n');
    const read = await replays.read(idb, entry.id, rows, verifyRoute);
    if (read.status !== 'ready' || read.routes.length === 0) {
      const note = document.getElementById('stats-status');
      if (note) note.textContent = 'No verified replay is available for this room yet.';
      return;
    }
    const routes: StudyRoute[] = [];
    for (const [index, route] of read.routes.entries()) {
      if (routes.some(existing => existing.route.actionLog === route.actionLog)) continue;
      const game = new WasmGame(rows);
      let trace: traces.Trace | null;
      try {
        trace = traces.build(game, route.actionLog);
      } finally {
        game.free();
      }
      if (trace !== null) routes.push({ label: index === 0 ? 'Personal best' : `Earlier best ${index}`, route, trace });
    }
    if (routes.length === 0) {
      const note = document.getElementById('stats-status');
      if (note) note.textContent = 'This room’s replays could not be rebuilt on the engine.';
      return;
    }
    study = { room: entry, routes, watch: 0, compare: routes.length > 1 ? 1 : -1, step: 0, playing: false, speed: 1, ghost: false };
    setText('replay-title', entry.title);
    select('replay-watch').replaceChildren(
      ...routes.map((route, index) => new Option(`${route.label} — ${route.route.moves} moves`, String(index))),
    );
    fillCompareChoices();
    drawStudy();
    openDialog('replay');
  })();
}

/** The compare select's choices: every route but the watched one, or the
 * note that none stands. */
function fillCompareChoices() {
  const current = study;
  if (current === undefined) return;
  const choices = select('replay-compare');
  const others = current.routes.filter((_, index) => index !== current.watch);
  choices.replaceChildren(
    ...others.map(route => new Option(`${route.label} — ${route.route.moves} moves`, String(current.routes.indexOf(route)))),
  );
  if (others.length === 0) {
    choices.add(new Option('No earlier best', '-1'));
    choices.disabled = true;
  } else choices.disabled = false;
  if (![...choices.options].some(option => option.value === String(current.compare)))
    current.compare = others.length > 0 ? current.routes.indexOf(others[0]!) : -1;
  choices.value = String(current.compare);
}

/** The study's whole face: the watched frame on the board — the reference's
 * ghost when asked for — the timeline with its markers, and the comparison. */
function drawStudy() {
  const current = study;
  if (current === undefined) return;
  const watched = current.routes[current.watch]!;
  const total = watched.trace.actionLog.length;
  const step = Math.min(current.step, total);
  const frame = traces.atStep(watched.trace, step);
  studyBoard.draw(watched.trace.width, watched.trace.height, watched.trace.tiles, watched.trace.labels, frame.state, frame.onGoal, false);
  const reference = current.compare >= 0 ? current.routes[current.compare] : undefined;
  let referenceState: Snapshot | undefined;
  if (reference !== undefined) referenceState = traces.atStep(reference.trace, Math.min(step, reference.trace.actionLog.length)).state;
  studyBoard.ghost(current.ghost && referenceState !== undefined ? referenceState.player : undefined, watched.trace.width);
  const seek = $<HTMLInputElement>('replay-seek');
  seek.max = String(total);
  seek.value = String(step);
  setText('replay-position', `Move ${step} / ${total}`);
  setText(
    'replay-meta',
    `${watched.route.moves} moves · ${watched.route.pushes} pushes${watched.route.elapsedMs ? ` · ${clock(watched.route.elapsedMs)}` : ''}`,
  );
  const comparison = reference !== undefined ? traces.compare(watched.trace, reference.trace) : undefined;
  $('replay-markers').replaceChildren(
    ...(comparison?.markers ?? []).map(marker => {
      const mark = document.createElement('button');
      mark.className = 'replay-marker';
      mark.dataset.kind = marker.kind;
      mark.textContent = marker.symbol;
      mark.title = marker.label;
      mark.setAttribute('aria-label', marker.label);
      mark.style.left = `${total === 0 ? 0 : (marker.step / total) * 100}%`;
      mark.onclick = () => {
        current.playing = false;
        current.step = Math.min(marker.step, total);
        drawStudy();
      };
      return mark;
    }),
  );
  setText('replay-summary', comparison?.summary ?? 'Save another improved personal best to compare two routes.');
  setText(
    'replay-reference',
    reference !== undefined ? `${reference.label}: ${reference.route.moves} moves · ${reference.route.pushes} pushes` : '',
  );
  $('replay-legend').replaceChildren(
    ...(comparison?.markers ?? []).map(marker => {
      const item = document.createElement('li');
      const mark = document.createElement('span');
      mark.dataset.kind = marker.kind;
      mark.textContent = marker.symbol;
      item.append(mark, marker.label);
      return item;
    }),
  );
  let description = traces.describe(watched.trace, step, watched.label);
  if (referenceState !== undefined)
    description += ` Comparison keeper at row ${Math.floor(referenceState.player / watched.trace.width) + 1}, column ${(referenceState.player % watched.trace.width) + 1}.`;
  setText('replay-description', description);
  button('replay-play').textContent = current.playing ? 'Pause' : 'Play';
}

/** Steps the study to `value`, bounded to its route, stopping the playback. */
function studySeek(value: number) {
  const current = study;
  if (current === undefined) return;
  current.playing = false;
  const total = current.routes[current.watch]!.trace.actionLog.length;
  current.step = Math.max(0, Math.min(total, Math.round(value)));
  drawStudy();
}

/** Advances the playback one step at its speed, rescheduling itself. */
function studyTick() {
  const current = study;
  if (current === undefined || !current.playing) return;
  const total = current.routes[current.watch]!.trace.actionLog.length;
  current.step = Math.min(total, current.step + 1);
  if (current.step >= total) current.playing = false;
  drawStudy();
  scheduleStudy();
}

function scheduleStudy() {
  if (studyTimer !== undefined) clearTimeout(studyTimer);
  studyTimer = study?.playing ? setTimeout(studyTick, Math.round(520 / study.speed)) : undefined;
}

/** The Solver Lab: a workspace over one room and position, where searches
 * run, their results are watched step by step, and two runs compare. */
const labBoard = new BoardView($('lab-board'));
let labPuzzle: (Entry & { text: string }) | undefined;
let labPrefix = '';
let labRuns: lab.RunRecord[] = [];
let labSequence = 0;
let labConfig: { mode: (typeof MODES)[number]; timeMs: number; memoryMiB: number; engine: string } | undefined;
let labWatched = -1;
let labLeft = -1;
let labRight = -1;
let labTrace: traces.Trace | undefined;
let labStep = 0;
let labPlaying = false;
let labTimer: number | undefined;

const labSolver = new SolverClient({
  worker: () => new Worker(new URL('./solver.worker.ts', import.meta.url), { type: 'module' }),
  changed: updateLabButtons,
  elapsed: ms => setText('lab-elapsed', seconds(ms)),
  update: applyLabUpdate,
  status: value => setText('lab-status', value),
  verify: (prefix, route) => {
    const current = labPuzzle;
    if (current === undefined) throw new Error('No puzzle is loaded');
    return verify(current.text, prefix + route);
  },
});

/** Renders the lab route: the room it names — a bare route renders the
 * default — with the transferred position replayed on a scratch game, or
 * the room's start when the log does not replay. */
function renderLab(next: Route) {
  if (next.kind !== 'solver-lab') return;
  const puzzle = ENTRIES.find(entry => entry.id === next.puzzleId) ?? ENTRIES[0]!;
  const text = puzzle.rows.join('\n');
  setText('lab-status', 'Run a search to begin.');
  let prefix = '';
  if (next.actionLog !== undefined) {
    try {
      verify(text, next.actionLog);
      prefix = next.actionLog;
    } catch {
      setText('lab-status', 'The transferred position could not be replayed; the lab starts from the room’s start.');
    }
  }
  labPuzzle = { ...puzzle, text };
  labPrefix = prefix;
  labRuns = [];
  labWatched = -1;
  labLeft = -1;
  labRight = -1;
  labTrace = undefined;
  labStep = 0;
  labPlaying = false;
  scheduleLab();
  select('lab-puzzle').value = puzzle.id;
  $<HTMLAnchorElement>('lab-play').href = `${playHash(puzzle.id)}${prefix === '' ? '' : `?play=${prefix}`}`;
  setText('lab-note', prefix === '' ? 'Initial position' : `Transferred after ${prefix.length} moves`);
  fillWatchChoices();
  drawLab();
  drawLabCompare();
}

/** Populates the lab's controls once: the rooms, the setup selects cloned
 * from the play page's (their source of truth), and the playback controls. */
function bindLab() {
  const rooms = select('lab-puzzle');
  for (const puzzle of ENTRIES) rooms.add(new Option(`${puzzle.title} · ${puzzle.difficulty}`, puzzle.id));
  rooms.onchange = () => {
    labSolver.cancel();
    navigate({ kind: 'solver-lab', puzzleId: rooms.value });
    rooms.blur();
  };
  const mode = select('lab-mode');
  for (const option of Array.from(select('mode').options)) {
    const clone = new Option(option.text, option.value, option.defaultSelected, option.selected);
    clone.dataset.help = option.dataset.help ?? '';
    mode.add(clone);
  }
  const syncModeHelp = () => setText('lab-mode-help', mode.selectedOptions[0]?.dataset.help ?? '');
  mode.onchange = syncModeHelp;
  syncModeHelp();
  for (const option of Array.from(select('seconds').options))
    select('lab-seconds').add(new Option(option.text, option.value, option.defaultSelected, option.selected));
  for (const option of Array.from(select('memory').options))
    select('lab-memory').add(new Option(option.text, option.value, option.defaultSelected, option.selected));
  const engine = select('lab-engine');
  for (const value of ENGINES) engine.add(new Option(value === 'browser' ? 'This browser · WebAssembly' : 'Server · native Rust', value));
  const speed = select('lab-speed');
  for (const value of [0.5, 1, 2, 4]) speed.add(new Option(`${value}×`, String(value)));
  speed.value = '1';
  button('lab-run').onclick = labRun;
  button('lab-cancel').onclick = () => labSolver.cancel();
  const seek = $<HTMLInputElement>('lab-seek');
  seek.oninput = () => labSeek(Number(seek.value));
  button('lab-first').onclick = () => labSeek(0);
  button('lab-back').onclick = () => labSeek(labStep - 1);
  button('lab-forward').onclick = () => labSeek(labStep + 1);
  button('lab-last').onclick = () => labSeek(Number(seek.max));
  button('lab-play-route').onclick = () => {
    if (labRouteTotal() === 0) return;
    if (labStep >= labRouteTotal()) labStep = 0;
    labPlaying = !labPlaying;
    scheduleLab();
    drawLab();
  };
  select('lab-watch').onchange = () => {
    labWatched = Number(select('lab-watch').value);
    watchLabTrace();
    labStep = 0;
    labPlaying = false;
    drawLab();
    drawLabCompare();
  };
}

/** Runs one experiment: the current setup, from the lab's position. */
function labRun() {
  if (labSolver.busy || labPuzzle === undefined) return;
  const mode = choice('lab-mode', MODES);
  const engine = choice('lab-engine', ENGINES);
  labConfig = {
    mode,
    engine,
    timeMs: Number(select('lab-seconds').value) * 1000,
    memoryMiB: Number(select('lab-memory').value),
  };
  for (const metric of ['lab-elapsed', 'lab-rate', 'lab-reserved', 'lab-lower', 'lab-best']) setText(metric, '—');
  $('lab-population').replaceChildren();
  setText('lab-status', 'Preparing search…');
  void labSolver.solve(engine, {
    rows: labPuzzle.text,
    actions: labPrefix,
    mode,
    timeMs: labConfig.timeMs,
    memoryMiB: labConfig.memoryMiB,
    maxStates: MAX_STATES,
  });
}

function updateLabButtons() {
  button('lab-run').disabled = labSolver.busy;
  button('lab-cancel').disabled = !labSolver.busy;
}

/** The lab's live telemetry and, on completion, the run's record: the
 * populations as bars, the rate and bounds, and the retained run the
 * playback then watches. */
function applyLabUpdate(update: SearchUpdate, route: string | undefined) {
  const { expanded, generated, reservedBytes, lowerBound, best } = update.metrics;
  $('lab-population').replaceChildren(
    ...lab.population(expanded, generated).map(row => {
      const container = document.createElement('div');
      container.className = 'lab-population-row';
      const head = document.createElement('div');
      const name = document.createElement('strong');
      name.textContent = row.label;
      const value = document.createElement('span');
      value.textContent = row.value.toLocaleString();
      head.append(name, value);
      const track = document.createElement('div');
      track.className = 'bar-track';
      const fill = document.createElement('span');
      fill.className = 'bar-fill';
      fill.style.width = `${Math.round(row.share * 100)}%`;
      track.append(fill);
      const note = document.createElement('small');
      note.textContent = row.description;
      container.append(head, track, note);
      return container;
    }),
  );
  setText('lab-reserved', `${(reservedBytes / 1048576).toFixed(1)} MiB`);
  setText('lab-lower', lowerBound !== undefined ? String(lowerBound) : '—');
  setText('lab-best', best !== undefined ? `${best} moves` : '—');
  setText('lab-rate', update.elapsedMs > 0 ? `${Math.round(expanded / (update.elapsedMs / 1000)).toLocaleString()} states/s` : '—');
  setText('lab-status', statusText(update.metrics, route));
  if (update.type !== 'done') return;
  // A lab search from the room's start with an optimal proof certifies the
  // room's optimum, exactly as the play page's searches do.
  if (update.metrics.proof.kind === 'optimal' && route !== undefined && labPrefix === '' && labPuzzle !== undefined)
    optimal.record(labPuzzle.id, labPuzzle.text, update.metrics.proof.moves);
  if (labConfig === undefined || labPuzzle === undefined) return;
  const record: lab.RunRecord = {
    id: `lab-run-${++labSequence}`,
    puzzleId: labPuzzle.id,
    actions: labPrefix,
    mode: labConfig.mode,
    timeMs: labConfig.timeMs,
    memoryMiB: labConfig.memoryMiB,
    engine: labConfig.engine,
    status: update.metrics.status,
    expanded,
    generated,
    reservedBytes,
    elapsedMs: update.elapsedMs,
    capturedAt: new Date().toISOString(),
  };
  if (route !== undefined) {
    // The run's full log replays through the engine before it can be watched.
    try {
      const snapshot = verify(labPuzzle.text, labPrefix + route);
      record.moves = snapshot.moves;
      record.pushes = snapshot.pushes;
      record.verifiedLog = labPrefix + route;
    } catch {
      /* The client verified this route; the lab fails closed anyway. */
    }
  }
  labRuns = [record, ...labRuns].slice(0, 6);
  labWatched = 0;
  labLeft = 0;
  labRight = labRuns.length > 1 ? 1 : -1;
  fillWatchChoices();
  watchLabTrace();
  labStep = 0;
  labPlaying = false;
  drawLab();
  drawLabCompare();
}

/** The watch select's choices: every retained run, newest first. */
function fillWatchChoices() {
  select('lab-watch').replaceChildren(...labRuns.map((record, index) => new Option(lab.label(record), String(index))));
  select('lab-watch').disabled = labRuns.length === 0;
}

/** Rebuilds the watched run's trace on a scratch game. */
function watchLabTrace() {
  labTrace = undefined;
  const record = labWatched >= 0 ? labRuns[labWatched] : undefined;
  if (record === undefined || record.verifiedLog === undefined || labPuzzle === undefined) return;
  const game = new WasmGame(labPuzzle.text);
  try {
    labTrace = traces.build(game, record.verifiedLog) ?? undefined;
  } finally {
    game.free();
  }
}

/** The watched route's own length: the trace less the transferred prefix. */
function labRouteTotal(): number {
  return Math.max(0, (labTrace?.actionLog.length ?? 0) - labPrefix.length);
}

/** The lab's watched face: the run's name, its board at the step, the path
 * strip and the playback position. The comparison draws separately, so the
 * playback's steps do not rebuild it. */
function drawLab() {
  updateLabButtons();
  const record = labWatched >= 0 ? labRuns[labWatched] : undefined;
  setText('lab-watched-label', record !== undefined ? lab.label(record) : 'No run selected');
  const trace = labTrace;
  if (record === undefined || trace === undefined) {
    button('lab-play-route').disabled = true;
    setText('lab-position', 'Run a search to inspect a verified solution path.');
    return;
  }
  const total = labRouteTotal();
  const step = Math.min(labStep, total);
  const frame = traces.atStep(trace, labPrefix.length + step);
  labBoard.draw(trace.width, trace.height, trace.tiles, trace.labels, frame.state, frame.onGoal, false);
  $('lab-path').replaceChildren(
    ...trace.actionLog
      .slice(labPrefix.length)
      .split('')
      .map((letter, index) => {
        const cell = document.createElement('span');
        if (step === index + 1) cell.dataset.current = 'true';
        if (index < step) cell.dataset.past = 'true';
        cell.textContent = GLYPHS[letter] ?? letter;
        return cell;
      }),
  );
  const seek = $<HTMLInputElement>('lab-seek');
  seek.max = String(total);
  seek.value = String(step);
  const routePushes = frame.state.pushes - traces.atStep(trace, labPrefix.length).state.pushes;
  setText('lab-position', `Move ${step} of ${total} · ${routePushes} ${routePushes === 1 ? 'push' : 'pushes'} in this route`);
  button('lab-play-route').disabled = total === 0;
  button('lab-play-route').textContent = labPlaying ? 'Pause playback' : 'Play route';
}

/** The comparison: two chosen runs' cards and their signed deltas. */
function drawLabCompare() {
  setText('lab-runs', `${labRuns.length} of 6 runs`);
  const body = $('lab-compare');
  if (labRuns.length < 2) {
    const empty = document.createElement('p');
    empty.className = 'muted';
    empty.textContent =
      'Run two experiments to compare them. Keep the limits fixed and change the mode to see how strategy affects work and route quality.';
    body.replaceChildren(empty);
    return;
  }
  const leftIndex = labLeft >= 0 ? labLeft : 0;
  const rightIndex = labRight >= 0 && labRight !== leftIndex ? labRight : leftIndex === 0 ? 1 : 0;
  const left = labRuns[leftIndex]!;
  const right = labRuns[rightIndex]!;
  const comparison = lab.compareRuns(left, right);
  const delta = (value: number | undefined, suffix = '') =>
    value === undefined ? '—' : value === 0 ? `same${suffix}` : `${value > 0 ? '+' : '−'}${Math.abs(value).toLocaleString()}${suffix}`;
  const selectors = document.createElement('div');
  selectors.className = 'lab-compare-selectors';
  for (const [name, chosen, apply] of [
    [
      'Left result',
      leftIndex,
      (value: number) => {
        labLeft = value;
      },
    ],
    [
      'Right result',
      rightIndex,
      (value: number) => {
        labRight = value;
      },
    ],
  ] as const) {
    const field = document.createElement('label');
    const label = document.createElement('span');
    label.textContent = name;
    const choices = document.createElement('select');
    for (const [index, record] of labRuns.entries()) choices.add(new Option(lab.label(record), String(index)));
    choices.value = String(chosen);
    choices.onchange = () => {
      apply(Number(choices.value));
      drawLabCompare();
    };
    field.append(label, choices);
    selectors.append(field);
  }
  const grid = document.createElement('div');
  grid.className = 'lab-compare-grid';
  for (const record of [left, right]) {
    const card = document.createElement('article');
    const name = document.createElement('p');
    name.textContent = `${record.mode} · ${record.engine}`;
    const outcome = document.createElement('h3');
    outcome.textContent = record.moves !== undefined ? 'Route found' : record.status.replace('_', ' ');
    const metrics = document.createElement('dl');
    for (const [term, value] of [
      ['Elapsed', `${(record.elapsedMs / 1000).toFixed(1)} s`],
      ['Expanded', record.expanded.toLocaleString()],
      ['Generated', record.generated.toLocaleString()],
      ['Moves', record.moves !== undefined ? record.moves.toLocaleString() : '—'],
      ['Pushes', record.pushes !== undefined ? record.pushes.toLocaleString() : '—'],
      ['Mode', record.mode],
    ] as const) {
      const row = document.createElement('div');
      const dt = document.createElement('dt');
      dt.textContent = term;
      const dd = document.createElement('dd');
      dd.textContent = value;
      row.append(dt, dd);
      metrics.append(row);
    }
    card.append(name, outcome, metrics);
    grid.append(card);
  }
  const summary = document.createElement('div');
  summary.className = 'lab-delta';
  const inputState = document.createElement('strong');
  inputState.textContent = comparison.sameInput ? 'Same puzzle and starting state' : 'Inputs differ';
  const limitState = document.createElement('span');
  limitState.textContent = comparison.sameLimits ? 'Limits match' : 'Limits or mode differ';
  const deltas = document.createElement('dl');
  for (const [term, value] of [
    ['Elapsed Δ', delta(comparison.elapsedDeltaMs, ' ms')],
    ['Expanded Δ', delta(comparison.expandedDelta)],
    ['Generated Δ', delta(comparison.generatedDelta)],
    ['Moves Δ', delta(comparison.moveDelta)],
    ['Pushes Δ', delta(comparison.pushDelta)],
  ] as const) {
    const row = document.createElement('div');
    const dt = document.createElement('dt');
    dt.textContent = term;
    const dd = document.createElement('dd');
    dd.textContent = value;
    row.append(dt, dd);
    deltas.append(row);
  }
  summary.append(inputState, limitState, deltas);
  body.replaceChildren(selectors, grid, summary);
}

/** Steps the playback to `value`, bounded to the watched route. */
function labSeek(value: number) {
  labPlaying = false;
  labStep = Math.max(0, Math.min(labRouteTotal(), Math.round(value)));
  drawLab();
}

/** Advances the playback one step at its speed, rescheduling itself. */
function labTick() {
  if (!labPlaying) return;
  labStep = Math.min(labRouteTotal(), labStep + 1);
  if (labStep >= labRouteTotal()) labPlaying = false;
  drawLab();
  scheduleLab();
}

function scheduleLab() {
  if (labTimer !== undefined) clearTimeout(labTimer);
  labTimer = labPlaying ? setTimeout(labTick, Math.round(520 / Number(select('lab-speed').value))) : undefined;
}

/** The theme menu's labels; the values are theme.ts's lists. */
const FAMILY_LABELS: Record<theme.ThemeFamily, string> = {
  'cozy-study': 'Cozy study',
  'midnight-neon': 'Midnight neon',
  'minimal-ink': 'Minimal ink',
};
const APPEARANCE_LABELS: Record<theme.Appearance, string> = { system: 'System', light: 'Light', dark: 'Dark' };
const MOTION_LABELS: Record<theme.Motion, string> = { system: 'System', full: 'Full', reduced: 'Reduced' };

/** Shows the stored preferences in the theme menu's selects. */
function syncThemeMenu() {
  const current = theme.preferences();
  select('theme-family').value = current.themeFamily;
  select('appearance').value = current.appearance;
  select('motion').value = current.motion;
}

/** Populates the theme menu from theme.ts's lists and applies every change; a
 * preference another tab stores arrives as a storage event, which re-syncs. */
function bindThemeMenu() {
  const family = select('theme-family');
  for (const value of theme.THEME_FAMILIES) family.add(new Option(FAMILY_LABELS[value], value));
  const appearance = select('appearance');
  for (const value of theme.APPEARANCES) appearance.add(new Option(APPEARANCE_LABELS[value], value));
  const motion = select('motion');
  for (const value of theme.MOTIONS) motion.add(new Option(MOTION_LABELS[value], value));
  syncThemeMenu();
  family.onchange = () => {
    theme.set({ themeFamily: choice('theme-family', theme.THEME_FAMILIES) });
    // The family decides which frames fit.
    frames.apply();
  };
  appearance.onchange = () => theme.set({ appearance: choice('appearance', theme.APPEARANCES) });
  motion.onchange = () => theme.set({ motion: choice('motion', theme.MOTIONS) });
  addEventListener('storage', event => {
    if (event.key === storage.key(theme.EXPERIENCE_KEY)) syncThemeMenu();
  });
}

/** ────────────────────────────────────────────────────────────────────────────
 * The puzzle editor: local drafts, the paintable grid, the shared-link preview
 * and the private playtest. The engine stays the source of truth — a grid only
 * tests and shares once the Rust parse of its rows succeeds.
 */

/** Whether the WebAssembly engine has started: the play route and the editor's
 * engine verdict both wait for it. */
let engineReady = false;

/** The editor's own board, for the playtest. */
const editorBoard = new BoardView($('editor-board'));

/** The drafts this browser keeps, loaded on the editor's first mount. */
let drafts: editor.DraftStore | undefined;
/** The grid on show: the active draft's, or a shared preview's read-only view. */
let editorState: editor.EditorState | undefined;
/** The undo history around it; a tool change never enters. */
let editorPast: editor.EditorState[] = [];
let editorFuture: editor.EditorState[] = [];
/** Whether the grid changed since its draft last autosaved. */
let editorDirty = false;
let editorSaveTimer: number | undefined;
/** The puzzle a #/editor?custom= link carries, decoded, when the route has one. */
let shared: editor.SharedPuzzle | undefined;
/** The custom parameter the editor last rendered, to catch the route dropping it. */
let editorCustom: string | undefined;
/** A live playtest, over the grid's rows or a shared puzzle's. */
let playtest: { game: WasmGame; tiles: Uint8Array; labels: Uint8Array; state: Snapshot; title: string } | undefined;
/** The grid's keyboard focus, the one cell the roving tabindex makes tabbable. */
let focusCell = { row: 0, column: 0 };
/** Whether painting floods connected cells instead of replacing one. */
let fillMode = false;
/** The grid's cell buttons, indexed [row][column]. */
let cellButtons: HTMLButtonElement[][] = [];
/** The rows the engine last parsed, and what it said: parsing runs only when
 * the rows change, so typing in the details never re-parses the grid. */
let verdictRows: string | undefined;
let verdictError: string | undefined;

/** The rooms a never-painted draft can start from. */
const STARTERS: editor.LoadablePuzzle[] = [
  {
    title: 'Single push starter',
    difficulty: 'beginner',
    hint: 'Push the box directly onto the goal.',
    rows: ['OOOOOOO', 'O     O', 'O RXS O', 'O     O', 'OOOOOOO'],
  },
  {
    title: 'Turn the corner starter',
    difficulty: 'beginner',
    hint: 'Try shaping a route that approaches the box from two sides.',
    rows: ['OOOOOOOOO', 'O       O', 'O R     O', 'O   X   O', 'O     S O', 'O       O', 'OOOOOOOOO'],
  },
];

/** The keys that select a paint tool, one per tool in toolbar order. */
const TOOL_KEYS: Record<string, string> = Object.fromEntries(editor.TOOLS.map((tool, index) => [String(index + 1), tool.symbol]));

const editorNotice = (value: string) => setText('editor-notice', value);

/** The glyph a cell shows: walls read as W, the floor as a dot. */
function cellGlyph(symbol: string): string {
  if (symbol === 'O') return 'W';
  if (symbol === ' ') return '·';
  if (symbol === 'S') return 'G';
  return symbol;
}

/** The glyph a toolbar tool shows; the goal tool paints a ring, not a letter. */
function toolGlyph(symbol: string): string {
  if (symbol === 'O') return 'W';
  if (symbol === ' ') return '·';
  if (symbol === 'S') return '◎';
  return symbol;
}

/** A cell's name, for the labels that read the grid. */
function cellName(symbol: string): string {
  if (symbol === 'O') return 'wall';
  if (symbol === ' ') return 'floor';
  if (symbol === 'R') return 'robot';
  if (symbol === 'X') return 'generic box';
  if (symbol === 'S') return 'generic goal';
  if (editor.isTypedBox(symbol)) return `typed box ${symbol}`;
  if (editor.isTypedGoal(symbol)) return `typed goal ${symbol}`;
  return 'unknown cell';
}

/** Writes one cell's symbol: its data attribute, glyph, label, and — for a
 * typed pair — the hue its box and goal share with the playing board. */
function updateCell(cell: HTMLButtonElement, symbol: string, tool: string) {
  cell.dataset.symbol = symbol;
  cell.textContent = cellGlyph(symbol);
  cell.setAttribute(
    'aria-label',
    `Row ${Number(cell.dataset.row) + 1}, column ${Number(cell.dataset.column) + 1}: ${cellName(symbol)}. Paint ${cellName(tool)}.`,
  );
  if (editor.isTypedBox(symbol)) {
    const tone = hue(symbol.charCodeAt(0));
    cell.style.background = `hsl(${tone} 45% 55%)`;
    cell.style.borderColor = `hsl(${tone} 45% 45%)`;
    cell.style.borderStyle = 'solid';
    cell.style.color = 'var(--paper-50)';
  } else if (editor.isTypedGoal(symbol)) {
    const tone = hue(symbol.toUpperCase().charCodeAt(0));
    cell.style.background = '';
    cell.style.borderColor = `hsl(${tone} 45% 45%)`;
    cell.style.borderStyle = 'dashed';
    cell.style.color = `hsl(${tone} 45% 32%)`;
  } else {
    cell.style.background = '';
    cell.style.borderColor = '';
    cell.style.borderStyle = '';
    cell.style.color = '';
  }
}

/** Builds the grid whole; its shape changed. */
function buildGrid(state: editor.EditorState) {
  const grid = $('editor-grid');
  focusCell = { row: Math.min(focusCell.row, state.height - 1), column: Math.min(focusCell.column, state.width - 1) };
  const rows: HTMLElement[] = [];
  cellButtons = state.cells.map((row, rowIndex) => {
    const rowElement = document.createElement('div');
    rowElement.className = 'editor-row';
    rowElement.setAttribute('role', 'row');
    const buttons = row.map((symbol, columnIndex) => {
      const cell = document.createElement('button');
      cell.type = 'button';
      cell.className = 'editor-cell';
      cell.dataset.row = String(rowIndex);
      cell.dataset.column = String(columnIndex);
      cell.setAttribute('role', 'gridcell');
      cell.tabIndex = rowIndex === focusCell.row && columnIndex === focusCell.column ? 0 : -1;
      updateCell(cell, symbol, state.tool);
      rowElement.append(cell);
      return cell;
    });
    rows.push(rowElement);
    return buttons;
  });
  grid.replaceChildren(...rows);
}

/** Applies a state change to the grid: a rebuild when the shape changed, else
 * only the cells that changed. */
function syncGrid(previous: editor.EditorState | undefined, next: editor.EditorState) {
  const grid = $('editor-grid');
  if (previous === undefined || previous.width !== next.width || previous.height !== next.height) buildGrid(next);
  else
    for (let row = 0; row < next.height; row++)
      for (let column = 0; column < next.width; column++)
        if (previous.cells[row]![column] !== next.cells[row]![column]!)
          updateCell(cellButtons[row]![column]!, next.cells[row]![column]!, next.tool);
  grid.style.setProperty('--editor-columns', String(next.width));
  grid.setAttribute('aria-rowcount', String(next.height));
  grid.setAttribute('aria-colcount', String(next.width));
  const badge = $('editor-size-badge');
  const size = `${next.width} × ${next.height}`;
  if (badge.textContent !== size) badge.textContent = size;
  badge.setAttribute('aria-label', `Grid size: ${next.width} by ${next.height}`);
}

/** Applies an editor action: undoable changes dirty the draft, schedule its
 * autosave, and clear the standing notice and share link; a tool or fill-mode
 * change is the painter's live hand, never history. */
function editorAct(next: editor.EditorState) {
  if (editorState === undefined || next === editorState) return;
  const previous = editorState;
  editorPast.push(previous);
  if (editorPast.length > 50) editorPast.shift();
  editorFuture = [];
  editorDirty = true;
  scheduleEditorSave();
  setText('editor-notice', '');
  $('editor-share-field').hidden = true;
  editorState = next;
  syncGrid(previous, next);
  refreshReadiness();
  syncUndoRedo();
  syncStarters();
}

/** Selects a paint tool. */
function setTool(symbol: string) {
  if (editorState === undefined) return;
  editorState = { ...editorState, tool: symbol };
  syncTools();
}

/** Steps the editor back and forward through its history; the tool in hand
 * survives the trip. */
function editorUndo() {
  if (editorState === undefined || editorPast.length === 0) return;
  const from = editorState;
  // The redo stack reads from its front, so the newest undo returns first.
  editorFuture.unshift(from);
  const target = editorPast.pop()!;
  editorState = { ...target, tool: from.tool };
  editorDirty = true;
  scheduleEditorSave();
  $('editor-share-field').hidden = true;
  syncGrid(from, editorState);
  refreshReadiness();
  syncUndoRedo();
  syncStarters();
}

function editorRedo() {
  if (editorState === undefined || editorFuture.length === 0) return;
  const from = editorState;
  editorPast.push(from);
  const target = editorFuture.shift()!;
  editorState = { ...target, tool: from.tool };
  editorDirty = true;
  scheduleEditorSave();
  $('editor-share-field').hidden = true;
  syncGrid(from, editorState);
  refreshReadiness();
  syncUndoRedo();
  syncStarters();
}

function syncUndoRedo() {
  button('editor-undo').disabled = editorPast.length === 0;
  button('editor-redo').disabled = editorFuture.length === 0;
}

/** What the readiness column says, and whether Test and Share may run: the
 * painter's checks first, then the engine's own parse of the rows — the
 * verdict that matters, straight from Rust. */
function editorVerdict(): { errors: string[]; playable: boolean } {
  if (editorState === undefined) return { errors: [], playable: false };
  const structural = editor.validate(editorState);
  if (!structural.valid) return { errors: structural.errors, playable: false };
  if (!engineReady) return { errors: ['The engine is still starting…'], playable: false };
  const rows = editor.rowsOf(editorState).join('\n');
  if (rows !== verdictRows) {
    verdictRows = rows;
    verdictError = undefined;
    try {
      const game = new WasmGame(rows);
      game.free();
    } catch (error) {
      verdictError = errorMessage(error);
    }
  }
  return verdictError === undefined ? { errors: [], playable: true } : { errors: [verdictError], playable: false };
}

function refreshReadiness() {
  const panel = $('editor-validation');
  if (editorState === undefined) {
    panel.replaceChildren();
    return;
  }
  const verdict = editorVerdict();
  panel.replaceChildren(
    ...verdict.errors.map(error => {
      const item = document.createElement('p');
      item.textContent = error;
      return item;
    }),
  );
  if (verdict.playable) {
    const ok = document.createElement('p');
    ok.className = 'editor-valid';
    ok.textContent = 'Puzzle is valid';
    panel.append(ok);
  }
  button('editor-test').disabled = !verdict.playable;
  button('editor-share').disabled = !verdict.playable;
}

/** The toolbar's tool buttons, and the typed pair's label. */
const toolButtons: HTMLButtonElement[] = [];
let typedBoxButton: HTMLButtonElement;
let typedGoalButton: HTMLButtonElement;
let typedLabel: editor.TypedLabel = 'A';

function syncTools() {
  const tool = editorState?.tool;
  for (const control of toolButtons) control.setAttribute('aria-pressed', String(control.dataset.tool === tool));
  syncTypedButtons();
  // The cells' labels name what a click paints, and the tool just changed.
  if (editorState !== undefined)
    for (const row of cellButtons) for (const control of row) updateCell(control, control.dataset.symbol ?? 'O', editorState.tool);
}

function syncTypedButtons() {
  const pair = (control: HTMLButtonElement, glyph: string, name: string) => {
    const letter = document.createElement('strong');
    letter.textContent = glyph;
    const kind = document.createElement('span');
    kind.textContent = name;
    control.replaceChildren(letter, kind);
  };
  pair(typedBoxButton, typedLabel, 'Box');
  pair(typedGoalButton, typedLabel.toLowerCase(), 'Goal');
  typedBoxButton.setAttribute('aria-label', `Typed box ${typedLabel}`);
  typedGoalButton.setAttribute('aria-label', `Typed goal ${typedLabel.toLowerCase()}`);
  const tool = editorState?.tool;
  typedBoxButton.setAttribute('aria-pressed', String(tool === typedLabel));
  typedGoalButton.setAttribute('aria-pressed', String(tool === typedLabel.toLowerCase()));
}

/** The typed kind the pair paints: selecting a label while a goal tool is
 * active keeps painting goals. */
function typedKind(): 'box' | 'goal' {
  return editorState !== undefined && editor.isTypedGoal(editorState.tool) ? 'goal' : 'box';
}

/** The details fields follow the state; typing in them is the state. */
function syncDetails() {
  if (editorState === undefined) return;
  const title = $<HTMLInputElement>('editor-title');
  if (title.value !== editorState.title) title.value = editorState.title;
  select('editor-difficulty').value = editorState.difficulty;
  const hint = $<HTMLTextAreaElement>('editor-hint');
  if (hint.value !== editorState.hint) hint.value = editorState.hint;
}

function syncSizeInputs() {
  if (editorState === undefined) return;
  $<HTMLInputElement>('editor-width').value = String(editorState.width);
  $<HTMLInputElement>('editor-height').value = String(editorState.height);
}

/** The drafts the operations that replace the store start from: the live grid
 * folded into the active draft. */
function savedDrafts(): editor.Draft[] {
  const store = drafts;
  if (store === undefined || !editorDirty || editorState === undefined) return store === undefined ? [] : store.drafts;
  const updatedAt = new Date().toISOString();
  return store.drafts.map(draft => (draft.id === store.activeId ? { ...draft, state: editorState!, updatedAt } : draft));
}

function activeDraft(): editor.Draft {
  const store = drafts!;
  return store.drafts.find(draft => draft.id === store.activeId) ?? store.drafts[0]!;
}

function syncDraftsUi() {
  if (drafts === undefined) return;
  const selector = select('editor-drafts');
  const active = activeDraft();
  selector.replaceChildren(...drafts.drafts.map(draft => new Option(draft.name, draft.id)));
  selector.value = active.id;
  const name = $<HTMLInputElement>('editor-draft-name');
  if (name.value !== active.name) name.value = active.name;
  button('editor-delete').disabled = drafts.drafts.length <= 1;
  setText('editor-draft-saved', active.updatedAt === null ? 'Not saved yet' : `Last saved ${journal.timeAgo(active.updatedAt)}`);
}

/** Loads the draft store on the editor's first mount: a malformed payload is
 * quarantined for download, never silently dropped. */
function loadDrafts() {
  const raw = storage.read(editor.DRAFTS_KEY);
  const parsed = editor.parseDraftStore(raw);
  if (parsed) {
    drafts = parsed;
    return;
  }
  if (raw !== null) {
    storage.write(editor.RECOVERY_KEY, raw);
    storage.remove(editor.DRAFTS_KEY);
  }
  drafts = editor.createDraftStore();
  storage.write(editor.DRAFTS_KEY, drafts);
}

function syncRecovery() {
  const raw = storage.read(editor.RECOVERY_KEY);
  $('editor-recovery').hidden = raw === null;
}

/** Folds the live grid into its draft and writes the store. */
function saveEditorDrafts() {
  if (drafts === undefined || editorState === undefined || !editorDirty) return;
  const updatedAt = new Date().toISOString();
  drafts = {
    activeId: drafts.activeId,
    drafts: drafts.drafts.map(draft => (draft.id === drafts!.activeId ? { ...draft, state: editorState!, updatedAt } : draft)),
  };
  editorDirty = false;
  storage.write(editor.DRAFTS_KEY, drafts);
  syncDraftsUi();
}

/** The autosave's debounce: a stroke paints many cells, the store writes once. */
function scheduleEditorSave() {
  clearTimeout(editorSaveTimer);
  editorSaveTimer = setTimeout(() => saveEditorDrafts(), 1000);
}

/** Flushes a pending autosave: leaving the editor, hiding the page, or entering
 * a shared preview all save first. */
function flushEditorSave() {
  clearTimeout(editorSaveTimer);
  editorSaveTimer = undefined;
  if (editorDirty) saveEditorDrafts();
}

/** Writes the next store and opens `state` as the grid, if one is named: the
 * operation already persisted everything, so no save stays pending. */
function commitDrafts(next: editor.Draft[], activeId: string, state?: editor.EditorState) {
  drafts = { activeId, drafts: next };
  editorDirty = false;
  clearTimeout(editorSaveTimer);
  editorSaveTimer = undefined;
  storage.write(editor.DRAFTS_KEY, drafts);
  if (state !== undefined) openEditorState(state);
  syncDraftsUi();
}

/** Shows a state as the editing grid, with a fresh history around it. */
function openEditorState(state: editor.EditorState) {
  const previous = editorState;
  editorState = state;
  editorPast = [];
  editorFuture = [];
  syncGrid(previous, state);
  syncDetails();
  syncSizeInputs();
  refreshReadiness();
  syncUndoRedo();
  syncStarters();
}

function syncStarters() {
  $('editor-starters').hidden = !(
    editorState !== undefined
    && editor.isBlank(editorState)
    && shared === undefined
    && playtest === undefined
  );
}

/** The editor page's mount. A route with a custom payload shows that puzzle
 * read-only — the painter's draft waits on storage — and leaving the preview
 * restores it, so looking at a link never costs a stroke. */
function renderEditor(next: Route) {
  if (next.kind !== 'editor') return;
  if (drafts === undefined) loadDrafts();
  exitPlaytest();
  const payload = next.custom;
  if (payload !== undefined && payload !== editorCustom) {
    flushEditorSave();
    shared = editor.decodeShare(payload) ?? undefined;
    $('editor-link-error').hidden = shared !== undefined;
    if (shared !== undefined) {
      openEditorState(editor.loadPuzzle(editorState ?? activeDraft().state, shared));
      setText('editor-shared-note', `Your saved draft "${activeDraft().name}" is unchanged.`);
    } else if (editorState === undefined) openEditorState(activeDraft().state);
  }
  if (payload === undefined && editorCustom !== undefined) {
    const restored = editor.parseDraftStore(storage.read(editor.DRAFTS_KEY));
    drafts = restored ?? editor.createDraftStore();
    shared = undefined;
    $('editor-link-error').hidden = true;
  }
  if (editorState === undefined) openEditorState(activeDraft().state);
  editorCustom = payload;
  const preview = payload !== undefined && shared !== undefined;
  $('editor-shared').hidden = !preview;
  button('editor-shared-play').disabled = shared === undefined || !engineReady;
  $<HTMLFieldSetElement>('editor-fields').disabled = preview;
  syncRecovery();
  syncDraftsUi();
  syncDetails();
  syncSizeInputs();
  syncTools();
  syncStarters();
  refreshReadiness();
}

/** Starts a private playtest of a puzzle's rows: a fresh Rust game over the
 * editor's board, its solves never journaling. */
function startPlaytest(puzzle: editor.LoadablePuzzle) {
  let game: WasmGame;
  try {
    game = new WasmGame(puzzle.rows.join('\n'));
  } catch (error) {
    editorNotice(errorMessage(error));
    return;
  }
  const tiles = game.tiles();
  const labels = game.labels();
  playtest = { game, tiles, labels, state: decodeSnapshot(game.snapshot(), labels.length), title: puzzle.title };
  setText('editor-playtest-title', puzzle.title);
  $('editor-playtest').hidden = false;
  $<HTMLFieldSetElement>('editor-fields').hidden = true;
  $<HTMLElement>('editor-playtest-title').focus();
  drawPlaytest();
}

function drawPlaytest() {
  const active = playtest;
  if (active === undefined) return;
  const onGoal = Array.from(active.labels, (_, index) => active.game.on_goal(index));
  editorBoard.draw(active.game.width(), active.game.height(), active.tiles, active.labels, active.state, onGoal);
  setText('editor-moves', String(active.state.moves));
  setText('editor-pushes', String(active.state.pushes));
  const status = $('editor-playtest-status');
  status.dataset.solved = String(active.state.solved);
  const text = active.state.solved
    ? `Solved in ${active.state.moves} ${active.state.moves === 1 ? 'move' : 'moves'}!`
    : 'Playtest in progress';
  if (status.textContent !== text) status.textContent = text;
}

function playtestMove(direction: number) {
  const active = playtest;
  if (active === undefined || active.state.solved) return;
  if (active.game.step(direction)) {
    active.state = decodeSnapshot(active.game.snapshot(), active.labels.length);
    drawPlaytest();
  } else editorBoard.blocked(direction);
}

/** A still touch or click on the playtest board walks the robot to the cell,
 * through floor only, as the play page's taps do. */
function playtestTap(x: number, y: number) {
  const active = playtest;
  if (active === undefined || active.state.solved) return;
  const cell = editorBoard.cellAt(x, y);
  if (cell === undefined) return;
  const route = walkRoute(active.game.width(), active.tiles, active.state.boxes, active.state.player, cell);
  if (route === undefined) return;
  for (const letter of route) {
    const direction = ACTIONS.indexOf(letter);
    if (direction === -1 || !active.game.step(direction)) break;
  }
  active.state = decodeSnapshot(active.game.snapshot(), active.labels.length);
  drawPlaytest();
}

function exitPlaytest() {
  if (playtest === undefined) return;
  playtest.game.free();
  playtest = undefined;
  $('editor-playtest').hidden = true;
  $<HTMLFieldSetElement>('editor-fields').hidden = false;
}

/** Leaving the editor: a pending autosave writes, and a live playtest ends. */
function leaveEditor() {
  flushEditorSave();
  exitPlaytest();
}

/** Shares the grid as a #/editor?custom= link: the field always carries it,
 * the clipboard or share sheet copies it. */
async function shareEditor() {
  if (editorState === undefined) return;
  const puzzle = editor.puzzleOf(editorState);
  const url = `${location.origin}${location.pathname}${formatHash({ kind: 'editor', custom: editor.encodeShare(puzzle) })}`;
  $<HTMLInputElement>('editor-share-url').value = url;
  $('editor-share-field').hidden = false;
  try {
    if (navigator.share !== undefined) {
      await navigator.share({ title: puzzle.title, url });
      return;
    }
    await navigator.clipboard.writeText(url);
    editorNotice('Share link copied.');
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') return;
    editorNotice('The share link is ready below. Select it to copy manually.');
  }
}

/** Paints one cell, or floods it in fill mode. */
function paintCell(row: number, column: number) {
  if (editorState === undefined) return;
  editorAct(fillMode ? editor.floodFill(editorState, row, column) : editor.paint(editorState, row, column));
}

/** Paints at a point: the cell button under it, when there is one. */
function paintAt(x: number, y: number) {
  const element = document.elementFromPoint(x, y)?.closest('button.editor-cell');
  if (element instanceof HTMLButtonElement) paintCell(Number(element.dataset.row), Number(element.dataset.column));
}

/** Moves the grid's roving focus, clamped to the grid. */
function focusGridCell(row: number, column: number) {
  if (editorState === undefined) return;
  const next = {
    row: Math.max(0, Math.min(editorState.height - 1, row)),
    column: Math.max(0, Math.min(editorState.width - 1, column)),
  };
  focusCell = next;
  cellButtons[next.row]?.[next.column]?.focus();
}

/** The pointer that is painting, if one is: a mouse or pen drags its paint
 * along; a touch waits for proof it never panned the canvas. */
let activePointer:
  { id: number; pointerType: string; startX: number; startY: number; row: number; column: number; moved: boolean } | undefined;

/** Binds the editor page's controls once; renderEditor repaints per route. */
function bindEditor() {
  // The toolbar: grouped tool buttons, then the typed pair.
  const tools = $('editor-tools');
  for (const group of ['terrain', 'pieces'] as const) {
    const groupElement = document.createElement('div');
    groupElement.className = 'editor-tool-group';
    const label = document.createElement('p');
    label.textContent = group === 'terrain' ? 'Terrain' : 'Pieces';
    const row = document.createElement('div');
    row.className = 'editor-tool-row';
    for (const tool of editor.TOOLS.filter(item => item.group === group)) {
      const control = document.createElement('button');
      control.type = 'button';
      control.className = 'editor-tool';
      control.dataset.tool = tool.symbol;
      control.textContent = toolGlyph(tool.symbol);
      const shortcut = editor.TOOLS.indexOf(tool) + 1;
      control.setAttribute('aria-label', `${tool.label} (${shortcut})`);
      control.title = `${tool.label} (${shortcut})`;
      control.onclick = () => setTool(tool.symbol);
      toolButtons.push(control);
      row.append(control);
    }
    groupElement.append(label, row);
    tools.append(groupElement);
  }
  const labels = select('editor-typed-label');
  for (const value of editor.TYPED_LABELS) labels.add(new Option(value, value));
  labels.value = typedLabel;
  labels.onchange = () => {
    typedLabel = choice('editor-typed-label', editor.TYPED_LABELS);
    setTool(typedKind() === 'goal' ? typedLabel.toLowerCase() : typedLabel);
  };
  typedBoxButton = button('editor-typed-box');
  typedGoalButton = button('editor-typed-goal');
  typedBoxButton.onclick = () => setTool(typedLabel);
  typedGoalButton.onclick = () => setTool(typedLabel.toLowerCase());
  syncTypedButtons();

  // The details and size fields: typing in them is the state.
  const difficulty = select('editor-difficulty');
  for (const value of DIFFICULTIES) difficulty.add(new Option(value.charAt(0).toUpperCase() + value.slice(1), value));
  const title = $<HTMLInputElement>('editor-title');
  title.addEventListener('input', () => {
    if (editorState !== undefined) editorAct({ ...editorState, title: title.value });
  });
  difficulty.onchange = () => {
    if (editorState !== undefined) editorAct({ ...editorState, difficulty: choice('editor-difficulty', DIFFICULTIES) });
  };
  const hint = $<HTMLTextAreaElement>('editor-hint');
  hint.addEventListener('input', () => {
    if (editorState !== undefined) editorAct({ ...editorState, hint: hint.value });
  });
  const width = $<HTMLInputElement>('editor-width');
  width.addEventListener('change', () => {
    if (editorState !== undefined) editorAct(editor.resize(editorState, Number(width.value), editorState.height));
    syncSizeInputs();
  });
  const height = $<HTMLInputElement>('editor-height');
  height.addEventListener('change', () => {
    if (editorState !== undefined) editorAct(editor.resize(editorState, editorState.width, Number(height.value)));
    syncSizeInputs();
  });

  // The grid: pointer painting, roving keyboard focus, keyboard activation.
  const grid = $('editor-grid');
  grid.addEventListener('pointerdown', event => {
    if (event.pointerType === 'mouse' && event.button !== 0) return;
    const cell = event.target instanceof Element ? event.target.closest('button.editor-cell') : null;
    if (!(cell instanceof HTMLButtonElement)) return;
    const row = Number(cell.dataset.row);
    const column = Number(cell.dataset.column);
    if (!Number.isInteger(row) || !Number.isInteger(column)) return;
    activePointer = {
      id: event.pointerId,
      pointerType: event.pointerType,
      startX: event.clientX,
      startY: event.clientY,
      row,
      column,
      moved: false,
    };
    // A touch may be a tap or the canvas's scroll; painting waits for proof it never panned.
    if (event.pointerType === 'touch') return;
    grid.setPointerCapture(event.pointerId);
    paintCell(row, column);
  });
  grid.addEventListener('pointermove', event => {
    const active = activePointer;
    if (active === undefined || active.id !== event.pointerId) return;
    if (active.pointerType === 'touch') {
      if (Math.hypot(event.clientX - active.startX, event.clientY - active.startY) > 8) active.moved = true;
      return;
    }
    paintAt(event.clientX, event.clientY);
  });
  const finishPainting = (event: PointerEvent, commitTouch: boolean) => {
    const active = activePointer;
    if (active === undefined || active.id !== event.pointerId) return;
    activePointer = undefined;
    if (commitTouch && active.pointerType === 'touch' && !active.moved) paintCell(active.row, active.column);
    if (grid.hasPointerCapture(event.pointerId)) grid.releasePointerCapture(event.pointerId);
  };
  grid.addEventListener('pointerup', event => finishPainting(event, true));
  grid.addEventListener('pointercancel', event => finishPainting(event, false));
  grid.addEventListener('lostpointercapture', event => {
    if (activePointer?.id === event.pointerId) activePointer = undefined;
  });
  grid.addEventListener('click', event => {
    // Pointer input painted already; a detail of 0 names keyboard activation.
    if (event.detail !== 0) return;
    const cell = event.target instanceof Element ? event.target.closest('button.editor-cell') : null;
    if (cell instanceof HTMLButtonElement) paintCell(Number(cell.dataset.row), Number(cell.dataset.column));
  });
  grid.addEventListener('keydown', event => {
    const cell = event.target instanceof Element ? event.target.closest('button.editor-cell') : null;
    if (!(cell instanceof HTMLButtonElement)) return;
    const row = Number(cell.dataset.row);
    const column = Number(cell.dataset.column);
    let nextRow = row;
    let nextColumn = column;
    switch (event.key) {
      case 'ArrowUp':
        nextRow -= 1;
        break;
      case 'ArrowDown':
        nextRow += 1;
        break;
      case 'ArrowLeft':
        nextColumn -= 1;
        break;
      case 'ArrowRight':
        nextColumn += 1;
        break;
      case 'Home':
        nextRow = event.ctrlKey ? 0 : row;
        nextColumn = 0;
        break;
      case 'End':
        nextRow = event.ctrlKey ? (editorState?.height ?? 1) - 1 : row;
        nextColumn = (editorState?.width ?? 1) - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    focusGridCell(nextRow, nextColumn);
  });
  grid.addEventListener('focusin', event => {
    const cell = event.target instanceof Element ? event.target.closest('button.editor-cell') : null;
    if (!(cell instanceof HTMLButtonElement)) return;
    focusCell = { row: Number(cell.dataset.row), column: Number(cell.dataset.column) };
    for (const row of cellButtons) for (const control of row) control.tabIndex = control === cell ? 0 : -1;
  });

  // The history, fill mode and the actions.
  button('editor-undo').onclick = () => editorUndo();
  button('editor-redo').onclick = () => editorRedo();
  button('editor-fill').onclick = () => {
    fillMode = !fillMode;
    const control = button('editor-fill');
    control.setAttribute('aria-pressed', String(fillMode));
    control.textContent = fillMode ? '◆ Fill on' : '◇ Fill off';
  };
  button('editor-test').onclick = () => {
    if (editorState !== undefined) startPlaytest(editor.puzzleOf(editorState));
  };
  button('editor-share').onclick = () => void shareEditor();
  button('editor-clear').onclick = () => openDialog('editor-clear-dialog');
  button('editor-clear-cancel').onclick = () => closeDialog('editor-clear-dialog');
  button('editor-clear-confirm').onclick = () => {
    closeDialog('editor-clear-dialog');
    if (editorState !== undefined) editorAct(editor.clearBoard(editorState));
  };
  button('editor-import-open').onclick = () => {
    $<HTMLTextAreaElement>('editor-import-text').value = '';
    $('editor-import-error').hidden = true;
    openDialog('editor-import');
  };
  button('editor-import-cancel').onclick = () => closeDialog('editor-import');
  button('editor-import-confirm').onclick = () => {
    const lines = $<HTMLTextAreaElement>('editor-import-text')
      .value.split('\n')
      .filter(line => line.length > 0);
    const error = (message: string) => {
      setText('editor-import-error', message);
      $('editor-import-error').hidden = false;
    };
    if (lines.length < 2) return error('Paste at least 2 rows of puzzle text.');
    const width = Math.max(...lines.map(line => [...line].length));
    if (width < editor.MIN_SIZE || lines.length < editor.MIN_SIZE)
      return error(`Puzzle must be at least ${editor.MIN_SIZE}x${editor.MIN_SIZE}.`);
    if (width > 20 || lines.length > 20) return error('Puzzle must be at most 20x20.');
    if (editorState === undefined) return;
    closeDialog('editor-import');
    editorAct(editor.loadPuzzle(editorState, { title: 'Imported puzzle', difficulty: 'beginner', rows: lines }));
    editorNotice('Puzzle rows imported. Paint the board, then choose Test puzzle.');
  };
  for (const [id, starter] of [
    ['editor-starter-single', STARTERS[0]!],
    ['editor-starter-turn', STARTERS[1]!],
  ] as const) {
    button(id).onclick = () => {
      if (editorState === undefined) return;
      editorAct(editor.loadPuzzle(editorState, starter));
      editorNotice(`${starter.title} loaded. Paint the board, then choose Test puzzle.`);
    };
  }

  // The drafts.
  select('editor-drafts').onchange = () => {
    if (drafts === undefined) return;
    const target = drafts.drafts.find(draft => draft.id === select('editor-drafts').value);
    if (target === undefined || target.id === drafts.activeId) return;
    commitDrafts(savedDrafts(), target.id, target.state);
    editorNotice('Local draft opened.');
  };
  button('editor-rename').onclick = () => {
    if (drafts === undefined) return;
    const name = $<HTMLInputElement>('editor-draft-name').value.trim();
    if (!name) return editorNotice('Give the draft a name.');
    const activeId = drafts.activeId;
    commitDrafts(
      savedDrafts().map(draft => (draft.id === activeId ? { ...draft, name } : draft)),
      activeId,
    );
    editorNotice('Draft renamed.');
  };
  button('editor-new').onclick = () => {
    if (drafts === undefined) return;
    if (drafts.drafts.length >= editor.MAX_DRAFTS) return editorNotice(`You can keep up to ${editor.MAX_DRAFTS} local drafts.`);
    const state = editor.initialState();
    const draft: editor.Draft = {
      id: editor.newDraftId(new Set(drafts.drafts.map(item => item.id))),
      name: 'New draft',
      updatedAt: new Date().toISOString(),
      state,
    };
    commitDrafts([...savedDrafts(), draft], draft.id, state);
    editorNotice('New local draft created.');
  };
  button('editor-duplicate').onclick = () => {
    if (drafts === undefined || editorState === undefined) return;
    if (drafts.drafts.length >= editor.MAX_DRAFTS) return editorNotice(`You can keep up to ${editor.MAX_DRAFTS} local drafts.`);
    const base = savedDrafts();
    const active = base.find(draft => draft.id === drafts!.activeId)!;
    const copy: editor.Draft = {
      ...active,
      id: editor.newDraftId(new Set(base.map(draft => draft.id))),
      name: `${active.name} copy`.slice(0, 60),
      updatedAt: new Date().toISOString(),
      state: editorState,
    };
    commitDrafts([...base, copy], copy.id, copy.state);
    editorNotice('Draft duplicated.');
  };
  button('editor-delete').onclick = () => {
    if (drafts === undefined || drafts.drafts.length <= 1) return;
    setText('editor-delete-message', `Delete "${activeDraft().name}" from this device? This cannot be undone.`);
    openDialog('editor-delete-dialog');
  };
  button('editor-delete-cancel').onclick = () => closeDialog('editor-delete-dialog');
  button('editor-delete-confirm').onclick = () => {
    if (drafts === undefined || drafts.drafts.length <= 1) return;
    const next = drafts.drafts.filter(draft => draft.id !== drafts!.activeId);
    commitDrafts(next, next[0]!.id, next[0]!.state);
    closeDialog('editor-delete-dialog');
    editorNotice('Draft deleted.');
  };

  // The shared preview's banner, and a quarantined draft's recovery.
  button('editor-shared-play').onclick = () => {
    if (shared !== undefined) startPlaytest(shared);
  };
  button('editor-shared-import').onclick = () => {
    if (shared === undefined || editorState === undefined || drafts === undefined) return;
    if (drafts.drafts.length >= editor.MAX_DRAFTS) return editorNotice(`You can keep up to ${editor.MAX_DRAFTS} local drafts.`);
    const state = editorState;
    const draft: editor.Draft = {
      id: editor.newDraftId(new Set(drafts.drafts.map(item => item.id))),
      name: shared.title.slice(0, 60),
      updatedAt: new Date().toISOString(),
      state,
    };
    // The preview never dirtied the draft it interrupted, so the store keeps it as it was.
    commitDrafts([...drafts.drafts, draft], draft.id, state);
    shared = undefined;
    navigate({ kind: 'editor' });
    editorNotice('Shared puzzle imported as a new local draft.');
  };
  button('editor-recovery-download').onclick = () => {
    const raw = storage.read(editor.RECOVERY_KEY);
    if (raw === null) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify(raw, null, 2)], { type: 'application/json' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'sokomind-invalid-editor-draft.json';
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  };
  button('editor-recovery-clear').onclick = () => {
    storage.remove(editor.RECOVERY_KEY);
    $('editor-recovery').hidden = true;
  };

  // The playtest's controls; its pad keeps its own ids, for the play page's
  // data-direction buttons belong to that page alone.
  button('editor-up').onclick = () => playtestMove(0);
  button('editor-down').onclick = () => playtestMove(1);
  button('editor-left').onclick = () => playtestMove(2);
  button('editor-right').onclick = () => playtestMove(3);
  button('editor-playtest-undo').onclick = () => {
    const active = playtest;
    if (active !== undefined && active.game.undo()) {
      active.state = decodeSnapshot(active.game.snapshot(), active.labels.length);
      drawPlaytest();
    }
  };
  button('editor-playtest-reset').onclick = () => {
    const active = playtest;
    if (active === undefined) return;
    active.game.reset();
    active.state = decodeSnapshot(active.game.snapshot(), active.labels.length);
    drawPlaytest();
  };
  button('editor-playtest-exit').onclick = () => {
    exitPlaytest();
    button('editor-test').focus();
  };
  bindInput($('editor-board'), {
    move: playtestMove,
    undo: () => button('editor-playtest-undo').click(),
    swipeable: () => editorBoard.fits,
    tap: playtestTap,
  });

  // The editor's own keys: 1-5 pick a tool, the chords work the history, and
  // Ctrl+Enter tests. They belong to the editing grid — a playtest or a
  // preview hands the keyboard back.
  document.addEventListener('keydown', event => {
    if (route().kind !== 'editor' || playtest !== undefined || shared !== undefined || document.querySelector('dialog[open]')) return;
    if (event.altKey) return;
    const editable = event.target instanceof Element && event.target.matches('input,textarea,select');
    if (event.ctrlKey || event.metaKey) {
      if (editable) return;
      const key = event.key.toLowerCase();
      if (key === 'z' && !event.shiftKey) {
        event.preventDefault();
        editorUndo();
      } else if (key === 'z' || key === 'y') {
        event.preventDefault();
        editorRedo();
      } else if (key === 'enter' && editorState !== undefined && !button('editor-test').disabled) {
        event.preventDefault();
        startPlaytest(editor.puzzleOf(editorState));
      }
      return;
    }
    if (editable) return;
    const tool = TOOL_KEYS[event.key];
    if (tool !== undefined) setTool(tool);
  });
}

async function start() {
  theme.start();
  bindThemeMenu();
  // The board wears whatever frame the journal has earned.
  frames.apply();
  const mode = select('mode');
  const syncModeHelp = () => setText('mode-help', mode.selectedOptions[0]?.dataset.help ?? '');
  mode.onchange = syncModeHelp;
  // The browser can restore the select's value on reload; match the help text.
  syncModeHelp();
  // The shell shows the first route before the engine is ready: a play route loads
  // once WebAssembly is, and every later one through the router.
  onPage('play', next => {
    if (engineReady) enterPlay(next);
  });
  onPage('home', syncHome);
  onPage('puzzles', renderPuzzles);
  onPage('stats', renderStats);
  onPage('solver-lab', renderLab);
  onPage('editor', renderEditor);
  // The lab's and the editor's controls bind before the router reports its
  // first route, so a page that opens cold is interactive at once.
  bindLab();
  bindEditor();
  // Leaving the play page stops its search and playback; the session keeps
  // saving. Leaving the editor flushes its autosave and ends any playtest.
  subscribe(next => {
    if (next.kind !== 'play') invalidate();
    if (next.kind !== 'editor') leaveEditor();
  });
  startShell();
  startRouter();
  await init({ module_or_path: wasmUrl });
  engineReady = true;
  if (route().kind === 'play') enterPlay(route());
  // The editor already renders without the engine; this pass brings its
  // readiness verdict, which only the engine can give.
  if (route().kind === 'editor') renderEditor(route());
  const selector = select('puzzles');
  for (const puzzle of ENTRIES) selector.add(new Option(`${puzzle.title} · ${puzzle.difficulty}`, puzzle.id));
  selector.add(new Option('Custom puzzle', CUSTOM_PUZZLE_ID));
  selector.onchange = () => {
    if (selector.value === CUSTOM_PUZZLE_ID) {
      selector.value = session?.puzzle.id ?? ENTRIES[0].id;
      $<HTMLTextAreaElement>('rows').closest('details')!.open = true;
      return;
    }
    const puzzle = ENTRIES.find(p => p.id === selector.value);
    if (puzzle) navigate({ kind: 'play', puzzleId: puzzle.id });
    // Arrow keys must move the robot, not walk the dropdown after a change.
    selector.blur();
  };
  const step = (delta: number) => {
    const id = session?.puzzle.id ?? ENTRIES[0].id;
    const index = ENTRIES.findIndex(p => p.id === id);
    navigate({ kind: 'play', puzzleId: ENTRIES[(index + delta + ENTRIES.length) % ENTRIES.length].id });
  };
  button('prev').onclick = () => step(-1);
  button('next').onclick = () => step(1);
  button('undo').onclick = () => {
    invalidate();
    if (loaded().game.undo()) changed();
  };
  button('undo-5').onclick = () => {
    invalidate();
    const { game } = loaded();
    for (let i = 0; i < 5 && game.moves() > 0; i++) game.undo();
    changed();
  };
  button('copy-moves').onclick = async () => {
    try {
      await navigator.clipboard.writeText(loaded().game.actions());
      message('Move sequence copied as U/D/L/R.');
    } catch {
      message('Clipboard is unavailable in this browser.');
    }
  };
  button('reset').onclick = () => {
    invalidate();
    loaded().game.reset();
    // A restart is a fresh attempt: its clock starts over.
    timer.reset(loaded().puzzle.id);
    changed();
  };
  button('best').onclick = () => {
    const best = progress.best();
    if (!best) return;
    invalidate();
    loaded().game.reset();
    render();
    animate(best.route);
  };
  button('load-custom').onclick = () => {
    try {
      load(
        customPuzzle(
          $<HTMLTextAreaElement>('rows')
            .value.replace(/\r/g, '')
            .replace(/^\n|\n$/g, ''),
        ),
      );
    } catch (error) {
      message(errorMessage(error));
    }
  };
  button('load-route').onclick = () => {
    const actions = $<HTMLTextAreaElement>('route-input').value.toUpperCase().replace(/\s/g, '');
    if (!actions) {
      message('Route is empty.');
      return;
    }
    try {
      // Rust validates the whole replay atomically before replacing the current state.
      loaded().game.replay(actions);
      invalidate();
      changed();
    } catch (error) {
      message(errorMessage(error));
    }
  };
  bindInput($('board'), {
    // The keys are document-wide, so they drive only the page they belong to:
    // on the editor route its grid and playtest own them.
    move: direction => {
      if (route().kind === 'play') move(direction);
    },
    // A click does nothing while the Undo button is disabled.
    undo: () => {
      if (route().kind === 'play') button('undo').click();
    },
    swipeable: () => board.fits && !zoom.zoomed,
    tap,
  });
  button('exit-zen').onclick = () => toggleZen();
  button('complete-replay').onclick = () => {
    closeDialog('complete');
    button('best').click();
  };
  button('complete-next').onclick = () => {
    closeDialog('complete');
    step(1);
  };
  button('complete-close').onclick = () => closeDialog('complete');
  button('deadlock-undo').onclick = () => {
    closeDialog('deadlock');
    button('undo').click();
  };
  button('deadlock-restart').onclick = () => {
    closeDialog('deadlock');
    button('reset').click();
  };
  button('deadlock-keep').onclick = () => closeDialog('deadlock');
  // The replay study's controls: watch and compare, seek and step, play.
  button('replay-close').onclick = () => closeDialog('replay');
  $('replay').addEventListener('close', () => {
    // Native Esc closes the dialog without our button; the playback stops
    // either way.
    if (study !== undefined) study.playing = false;
    scheduleStudy();
  });
  select('replay-watch').onchange = () => {
    const current = study;
    if (current === undefined) return;
    current.watch = Number(select('replay-watch').value);
    if (current.compare === current.watch) current.compare = current.routes.findIndex((_, index) => index !== current.watch);
    current.step = 0;
    current.playing = false;
    fillCompareChoices();
    drawStudy();
  };
  select('replay-compare').onchange = () => {
    if (study === undefined) return;
    study.compare = Number(select('replay-compare').value);
    study.step = 0;
    study.playing = false;
    drawStudy();
  };
  const seek = $<HTMLInputElement>('replay-seek');
  seek.oninput = () => studySeek(Number(seek.value));
  button('replay-first').onclick = () => studySeek(0);
  button('replay-back').onclick = () => studySeek((study?.step ?? 0) - 1);
  button('replay-forward').onclick = () => studySeek((study?.step ?? 0) + 1);
  button('replay-last').onclick = () => studySeek(Number(seek.max));
  button('replay-play').onclick = () => {
    if (study === undefined) return;
    const total = study.routes[study.watch]!.trace.actionLog.length;
    if (study.step >= total) study.step = 0;
    study.playing = !study.playing;
    scheduleStudy();
    drawStudy();
  };
  const speed = select('replay-speed');
  for (const value of [0.5, 1, 2, 4]) speed.add(new Option(`${value}×`, String(value)));
  speed.value = '1';
  speed.onchange = () => {
    if (study !== undefined) study.speed = Number(speed.value);
    scheduleStudy();
  };
  const ghostBox = $<HTMLInputElement>('replay-ghost');
  ghostBox.onchange = () => {
    if (study !== undefined) study.ghost = ghostBox.checked;
    drawStudy();
  };
  $('replay').addEventListener('keydown', event => {
    if (event.target instanceof Element && event.target.matches('input,textarea,select,button')) return;
    if (event.key === ' ') {
      event.preventDefault();
      button('replay-play').click();
    } else if (event.key === 'ArrowLeft') {
      event.preventDefault();
      button('replay-back').click();
    } else if (event.key === 'ArrowRight') {
      event.preventDefault();
      button('replay-forward').click();
    }
  });
  // The search laboratory: the play page's solver dialog, and its door to the lab.
  button('open-solver').onclick = () => openDialog('solver');
  button('solver-close').onclick = () => closeDialog('solver');
  button('solver-lab-link').onclick = () => {
    closeDialog('solver');
    const actions = session?.game.actions() ?? '';
    navigate(
      actions === ''
        ? { kind: 'solver-lab', puzzleId: loaded().puzzle.id }
        : { kind: 'solver-lab', puzzleId: loaded().puzzle.id, actionLog: actions },
    );
  };
  button('hint').onclick = askHint;
  button('share').onclick = () => void share();
  button('random').onclick = () => {
    const pool = filterEntries(ENTRIES, { status: 'open' });
    const from = pool.length === 0 ? ENTRIES : pool;
    navigate({ kind: 'play', puzzleId: from[Math.floor(Math.random() * from.length)]!.id });
  };
  for (const [id, value] of RATE_BUTTONS) {
    button(id).onclick = () => {
      rate(loaded().puzzle.id, value);
      markRating();
      message('Rating saved.');
    };
  }
  /** Walks to the next puzzle with no stored best, wrapping the ENTRIES. */
  const stepToUnsolved = () => {
    const from = session?.puzzle.id ?? ENTRIES[0].id;
    const index = ENTRIES.findIndex(p => p.id === from);
    for (let ahead = 1; ahead <= ENTRIES.length; ahead++) {
      const candidate = ENTRIES[(index + ahead) % ENTRIES.length]!;
      if (storage.best(candidate.id, candidate.rows.join('\n')) === null) {
        navigate({ kind: 'play', puzzleId: candidate.id });
        return;
      }
    }
    navigate({ kind: 'play', puzzleId: ENTRIES[(index + 1) % ENTRIES.length]!.id });
  };
  // The page-level keys the movement layer does not own. While a dialog is up,
  // its keys are the dialog's.
  document.addEventListener('keydown', event => {
    if (document.querySelector('dialog[open]')) return;
    if (route().kind !== 'play') return;
    if (ignoresKey(event, event.target instanceof Element && event.target.matches('input,textarea,select'))) return;
    switch (event.key.toLowerCase()) {
      // The plain Z is zen; its chord undoes in the movement layer.
      case 'z':
        if (!event.ctrlKey && !event.metaKey) toggleZen();
        break;
      case 'h':
        askHint();
        break;
      case 'r':
        if (!paused) button('reset').click();
        break;
      case 'p':
        togglePause();
        break;
      case '[':
        step(-1);
        break;
      case ']':
        step(1);
        break;
      case 'n':
        stepToUnsolved();
        break;
      case '?':
        openDialog('shortcuts');
        break;
    }
  });
  button('solve').onclick = solve;
  button('cancel').onclick = () => {
    if (playback.active) {
      playback.end();
      message('Playback stopped.');
    } else solver.cancel();
  };
  button('play').onclick = () => {
    playback.toggle(solver.route);
    updateButtons();
  };
  button('copy').onclick = async () => {
    if (solver.route === undefined) return;
    try {
      await navigator.clipboard.writeText(solver.prefix + solver.route);
      message('Full route copied as U/D/L/R.');
    } catch {
      message('Clipboard is unavailable in this browser.');
    }
  };
  // The skip button bypasses the header's controls; it must not touch the hash,
  // which the router owns.
  button('skip').onclick = () => {
    $<HTMLElement>('page').focus();
    scrollTo({ top: 0 });
  };
  addEventListener('resize', () => render());
  addEventListener('pagehide', () => {
    saveSession();
    flushEditorSave();
  });
  // Mobile browsers often skip pagehide, so hiding the page flushes the pending save too.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') {
      saveSession();
      timer.pause();
    } else if (!session?.state.solved) timer.run();
  });
  void probeServer();
}
/** Labels the page when /api/health answers, so the native solver can run. Static hosting has no
 * API: a 404, an app page instead of JSON, or no answer leaves Browser mode on show. */
async function probeServer() {
  try {
    const response = await fetch('/api/health');
    if (response.ok && decodeHealth(await response.json())) setText('connection', 'Native solver connected');
  } catch {
    /* No API here; the browser solver still works. */
  }
}
start().catch(error => message(`Could not start WebAssembly: ${errorMessage(error)}. Run npm run wasm and reload.`));
