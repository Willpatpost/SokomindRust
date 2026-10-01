import './style.css';
import init, { WasmGame } from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import catalog from '../../data/puzzles.json';
import { BoardView } from './board.ts';
import { bindInput } from './input.ts';
import * as storage from './storage.ts';
import { MAX_ROUTE, MAX_STATES, MODES, errorMessage, type SearchUpdate, type Snapshot, type SolveRequest } from './protocol.ts';
import { ENGINES, SolverClient } from './solver-client.ts';
import { decodeSnapshot } from './transport.ts';
import { Playback } from './playback.ts';
import { CUSTOM_PUZZLE_ID, ProgressClient } from './progress.ts';
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
const MOVE_HINT = 'Arrow keys or WASD to move. Z to undo.';
const customPuzzle = (text: string): Puzzle => ({
  id: CUSTOM_PUZZLE_ID,
  title: 'Your puzzle',
  difficulty: 'custom',
  rows: text.split('\n'),
});
const board = new BoardView($<HTMLCanvasElement>('board'));
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
const progress = new ProgressClient({
  verify,
  show: showBest,
  status: value => setText('storage', value),
  connected: persistence => setText('connection', persistence ? 'PostgreSQL connected' : 'Native solver connected'),
});

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
  board.draw(
    game.width(),
    game.height(),
    tiles,
    labels,
    state,
    Array.from(labels, (_, i) => game.on_goal(i)),
  );
  setText('moves', String(state.moves));
  setText('pushes', String(state.pushes));
  updateButtons();
}
function saveSession() {
  if (session) progress.session(session.game.actions());
}
function showBest(best: storage.Best | null) {
  button('best').disabled = !best;
  setText('best-score', best ? `Best: ${best.moves} moves · ${best.pushes} pushes` : '');
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
function changed() {
  render();
  const { game, state } = loaded();
  if (!state.solved) {
    message(MOVE_HINT);
    progress.session(game.actions(), true);
    return;
  }
  message(`Solved in ${state.moves} moves and ${state.pushes} pushes.`);
  const actions = game.actions();
  progress.session(actions);
  progress.keep(actions, state.moves, state.pushes);
  void progress.sync(actions);
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
  setText('title', puzzle.title);
  setText('difficulty', puzzle.difficulty);
  setText('hint', puzzle.hint || 'Match every box to its goal.');
  $<HTMLTextAreaElement>('rows').value = text;
  paint(next);
  progress.select(next.puzzle.id, next.puzzle.text);
  message(actions ? 'Session restored by replaying your moves.' : MOVE_HINT);
  saveSession();
}
function move(direction: number) {
  if (!session) return;
  const { game } = session;
  if (solver.busy || solver.route !== undefined || playback.active) invalidate();
  if (game.step(direction)) changed();
  else if (game.moves() >= MAX_ROUTE) message(`Session move limit reached (${MAX_ROUTE}). Undo or restart to continue.`);
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

async function start() {
  const mode = select('mode');
  const syncModeHelp = () => setText('mode-help', mode.selectedOptions[0]?.dataset.help ?? '');
  mode.onchange = syncModeHelp;
  // The browser can restore the select's value on reload; match the help text.
  syncModeHelp();
  await init({ module_or_path: wasmUrl });
  const selector = select('puzzles');
  for (const puzzle of catalog) selector.add(new Option(`${puzzle.title} · ${puzzle.difficulty}`, puzzle.id));
  selector.add(new Option('Custom puzzle', CUSTOM_PUZZLE_ID));
  const saved = storage.session();
  const savedPuzzle = saved?.id === CUSTOM_PUZZLE_ID ? customPuzzle(saved.rows) : catalog.find(p => p.id === saved?.id);
  const canRestore = savedPuzzle && savedPuzzle.rows.join('\n') === saved?.rows;
  // A stale saved layout falls back to the saved puzzle itself, not puzzle one.
  const target = savedPuzzle ?? catalog[0];
  try {
    load(target, canRestore ? saved.actions : '');
  } catch {
    try {
      load(target);
    } catch {
      load(catalog[0]);
      message('Saved session was invalid. Started a fresh puzzle.');
    }
  }
  selector.onchange = () => {
    if (selector.value === CUSTOM_PUZZLE_ID) {
      selector.value = loaded().puzzle.id;
      $<HTMLTextAreaElement>('rows').closest('details')!.open = true;
      return;
    }
    const puzzle = catalog.find(p => p.id === selector.value);
    if (puzzle) load(puzzle);
    // Arrow keys must move the robot, not walk the dropdown after a change.
    selector.blur();
  };
  button('next').onclick = () => {
    const { id } = loaded().puzzle;
    const index = catalog.findIndex(p => p.id === id);
    load(catalog[(index + 1) % catalog.length]);
  };
  button('undo').onclick = () => {
    invalidate();
    if (loaded().game.undo()) changed();
  };
  button('reset').onclick = () => {
    invalidate();
    loaded().game.reset();
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
      load(customPuzzle($<HTMLTextAreaElement>('rows').value.replace(/\r/g, '').replace(/^\n|\n$/g, '')));
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
  bindInput($<HTMLCanvasElement>('board'), {
    move,
    // A click does nothing while the Undo button is disabled.
    undo: () => button('undo').click(),
    swipeable: () => board.fits,
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
  addEventListener('resize', () => render());
  addEventListener('pagehide', () => saveSession());
  // Mobile browsers often skip pagehide, so hiding the page flushes the pending save too.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') saveSession();
  });
  // Static hosting needs no backend. A server that is busy, restarting or not
  // yet started is asked again with backoff until it reports persistence.
  await progress.probe();
}
start().catch(error => message(`Could not start WebAssembly: ${errorMessage(error)}. Run npm run wasm and reload.`));
