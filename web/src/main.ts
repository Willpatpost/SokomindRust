import './styles/tokens.css';
import './style.css';
import init, { WasmGame } from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import catalog from '../../data/puzzles.json';
import { BoardView } from './board.ts';
import { bindInput, ignoresKey } from './input.ts';
import * as storage from './storage.ts';
import * as theme from './theme.ts';
import { navigate, route, startRouter, subscribe, type Route } from './router.ts';
import { onPage, playHash, startShell } from './shell.ts';
import { walkRoute } from './walk.ts';
import { PinchZoom } from './zoom.ts';
import { clock, Timer } from './timer.ts';
import { present } from './complete.ts';
import { celebrate } from './celebrate.ts';
import { closeDialog, openDialog } from './dialog.ts';
import { MAX_ROUTE, MAX_STATES, MODES, errorMessage, type SearchUpdate, type Snapshot, type SolveRequest } from './protocol.ts';
import { ENGINES, SolverClient } from './solver-client.ts';
import { decodeHealth, decodeSnapshot } from './transport.ts';
import { Playback } from './playback.ts';
import { Progress } from './progress.ts';
import { statusText } from './verdict.ts';

interface Puzzle {
  id: string;
  title: string;
  difficulty: string;
  rows: string[];
  hint?: string;
}
/** The loaded puzzle. load() replaces it whole; render() refreshes `state`
 * from the game after every change. */
interface Session {
  game: WasmGame;
  puzzle: Puzzle & { text: string };
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
const customPuzzle = (text: string): Puzzle => ({
  id: CUSTOM_PUZZLE_ID,
  title: 'Your puzzle',
  difficulty: 'custom',
  rows: text.split('\n'),
});
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
const solver = new SolverClient({
  worker: () => new Worker(new URL('./solver.worker.ts', import.meta.url), { type: 'module' }),
  changed: updateButtons,
  elapsed: ms => setText('elapsed', seconds(ms)),
  update: applyUpdate,
  status: setStatus,
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
function paint({ game, tiles, labels, state }: Session) {
  const onGoal = Array.from(labels, (_, i) => game.on_goal(i));
  board.draw(game.width(), game.height(), tiles, labels, state, onGoal);
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
/** Opens the completion dialog for a solve, against the best that stood before
 * it, and bursts the celebration sized by the tier the solve earned. */
function finish(state: Snapshot, before: storage.Best | null) {
  const shown = present({ moves: state.moves, pushes: state.pushes }, before);
  setText('complete-eyebrow', shown.eyebrow);
  setText('complete-title', shown.title);
  setText('complete-summary', shown.summary);
  document.getElementById('complete-milestones')!.replaceChildren(
    ...shown.milestones.map(text => {
      const item = document.createElement('li');
      item.textContent = text;
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
  finish(state, before);
}
function load(puzzle: Puzzle, actions = '') {
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
  const index = catalog.findIndex(p => p.id === puzzle.id);
  setText('position', index === -1 ? 'custom' : `${index + 1} / ${catalog.length}`);
  setText('title', puzzle.title);
  setText('difficulty', puzzle.difficulty);
  setText('hint', puzzle.hint || 'Match every box to its goal.');
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
// Every transport route is replayed on a scratch Rust game before display.
function applyUpdate(update: SearchUpdate, route: string | undefined) {
  const { expanded, generated, reservedBytes } = update.metrics;
  setText('expanded', expanded.toLocaleString());
  setText('generated', generated.toLocaleString());
  setText('reserved', `${(reservedBytes / 1048576).toFixed(1)} MiB`);
  setStatus(statusText(update.metrics, route));
}
function solve() {
  if (solver.busy) return;
  invalidate();
  const { game, puzzle } = loaded();
  const request: SolveRequest = {
    rows: puzzle.text,
    actions: game.actions(),
    mode: choice('mode', MODES),
    timeMs: Number(select('seconds').value) * 1000,
    memoryMiB: Number(select('memory').value),
    maxStates: MAX_STATES,
  };
  for (const metric of ['expanded', 'generated', 'reserved', 'elapsed']) setText(metric, '—');
  setStatus('Preparing search…');
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

/** Loads the play route's puzzle: a shared log replays as given, and without one the
 * saved session continues when it was saved for this puzzle. A route that names no
 * catalog puzzle, or a log that does not replay, goes home. */
function enterPlay(next: Route) {
  if (next.kind !== 'play') return;
  const puzzle = catalog.find(p => p.id === next.puzzleId);
  if (!puzzle) {
    navigate({ kind: 'home' });
    return;
  }
  const saved = storage.session();
  const actions = next.actionLog ?? (saved?.id === puzzle.id && saved.rows === puzzle.rows.join('\n') ? saved.actions : '');
  try {
    load(puzzle, actions);
  } catch {
    navigate({ kind: 'home' });
  }
}

/** Aims the home page's primary action at the saved session's puzzle when one waits
 * there, or at the catalog's first. */
function syncHome() {
  const saved = storage.session();
  const id = saved && catalog.some(p => p.id === saved.id) ? saved.id : catalog[0].id;
  const start = $<HTMLAnchorElement>('start');
  start.href = playHash(id);
  start.textContent = saved ? 'Resume playing' : 'Start playing';
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
  family.onchange = () => theme.set({ themeFamily: choice('theme-family', theme.THEME_FAMILIES) });
  appearance.onchange = () => theme.set({ appearance: choice('appearance', theme.APPEARANCES) });
  motion.onchange = () => theme.set({ motion: choice('motion', theme.MOTIONS) });
  addEventListener('storage', event => {
    if (event.key === storage.key(theme.EXPERIENCE_KEY)) syncThemeMenu();
  });
}

async function start() {
  theme.start();
  bindThemeMenu();
  const mode = select('mode');
  const syncModeHelp = () => setText('mode-help', mode.selectedOptions[0]?.dataset.help ?? '');
  mode.onchange = syncModeHelp;
  // The browser can restore the select's value on reload; match the help text.
  syncModeHelp();
  // The shell shows the first route before the engine is ready: a play route loads
  // once WebAssembly is, and every later one through the router.
  let engineReady = false;
  onPage('play', next => {
    if (engineReady) enterPlay(next);
  });
  onPage('home', syncHome);
  // Leaving the play page stops its search and playback; the session keeps saving.
  subscribe(next => {
    if (next.kind !== 'play') invalidate();
  });
  startShell();
  startRouter();
  await init({ module_or_path: wasmUrl });
  engineReady = true;
  if (route().kind === 'play') enterPlay(route());
  const selector = select('puzzles');
  for (const puzzle of catalog) selector.add(new Option(`${puzzle.title} · ${puzzle.difficulty}`, puzzle.id));
  selector.add(new Option('Custom puzzle', CUSTOM_PUZZLE_ID));
  selector.onchange = () => {
    if (selector.value === CUSTOM_PUZZLE_ID) {
      selector.value = session?.puzzle.id ?? catalog[0].id;
      $<HTMLTextAreaElement>('rows').closest('details')!.open = true;
      return;
    }
    const puzzle = catalog.find(p => p.id === selector.value);
    if (puzzle) navigate({ kind: 'play', puzzleId: puzzle.id });
    // Arrow keys must move the robot, not walk the dropdown after a change.
    selector.blur();
  };
  const step = (delta: number) => {
    const id = session?.puzzle.id ?? catalog[0].id;
    const index = catalog.findIndex(p => p.id === id);
    navigate({ kind: 'play', puzzleId: catalog[(index + delta + catalog.length) % catalog.length].id });
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
    move,
    // A click does nothing while the Undo button is disabled.
    undo: () => button('undo').click(),
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
  /** Walks to the next puzzle with no stored best, wrapping the catalog. */
  const stepToUnsolved = () => {
    const from = session?.puzzle.id ?? catalog[0].id;
    const index = catalog.findIndex(p => p.id === from);
    for (let ahead = 1; ahead <= catalog.length; ahead++) {
      const candidate = catalog[(index + ahead) % catalog.length]!;
      if (storage.best(candidate.id, candidate.rows.join('\n')) === null) {
        navigate({ kind: 'play', puzzleId: candidate.id });
        return;
      }
    }
    navigate({ kind: 'play', puzzleId: catalog[(index + 1) % catalog.length]!.id });
  };
  // The page-level keys the movement layer does not own. While a dialog is up,
  // its keys are the dialog's.
  document.addEventListener('keydown', event => {
    if (document.querySelector('dialog[open]')) return;
    if (ignoresKey(event, event.target instanceof Element && event.target.matches('input,textarea,select'))) return;
    switch (event.key.toLowerCase()) {
      // The plain Z is zen; its chord undoes in the movement layer.
      case 'z':
        if (!event.ctrlKey && !event.metaKey) toggleZen();
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
  addEventListener('pagehide', () => saveSession());
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
