import './style.css';
import init, { WasmGame } from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import catalog from '../../data/puzzles.json';
import { BoardView, type Snapshot } from './board';
import * as storage from './storage';
import {
  MAX_ROUTE,
  MAX_STATES,
  errorMessage,
  type SearchStatus,
  type SearchUpdate,
  type SolveRequest,
} from './protocol';
import { SolverClient } from './solver-client';
import { Playback } from './playback';
import { ProgressClient } from './progress';

interface Puzzle { id: string; title: string; difficulty: string; rows: string[]; hint?: string }
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const button = (id: string) => $<HTMLButtonElement>(id);
const select = (id: string) => $<HTMLSelectElement>(id);
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
  id: 'custom',
  title: 'Your puzzle',
  difficulty: 'custom',
  rows: text.split('\n'),
});
const board = new BoardView($<HTMLCanvasElement>('board'));
let game: WasmGame;
let current: Puzzle & { text: string };
let tiles: Uint8Array;
let labels: Uint8Array;
let state: Snapshot;
const solver = new SolverClient({
  worker: () => new Worker(new URL('./solver.worker.ts', import.meta.url), { type: 'module' }),
  changed: updateButtons,
  elapsed: ms => setText('elapsed', seconds(ms)),
  update: applyUpdate,
  status: setStatus,
  verify: (prefix, route) => { verify(current.text, prefix + route); },
});
const playback = new Playback(
  direction => game.step(direction), changed,
  blocked => {
    solver.dropRoute();
    updateButtons();
    if (blocked) message('Replay was blocked.');
  },
);
const progress = new ProgressClient({
  verify, show: showBest, status: value => setText('storage', value),
});

function updateButtons() {
  const busy = solver.busy;
  button('solve').disabled = busy || !game || state.solved;
  button('cancel').disabled = !busy && !playback.active;
  button('play').disabled = busy || (solver.route === undefined && !playback.active);
  setText('play', !playback.active ? 'Play route' : playback.state.kind === 'paused' ? 'Resume' : 'Pause');
  button('copy').disabled = solver.route === undefined;
  button('undo').disabled = !game || state.moves === 0;
}
function animate(route: string) { playback.play(route); updateButtons(); }
function invalidate() {
  solver.reset();
  playback.stop();
  updateButtons();
  setStatus('Ready when you are.');
}
// Snapshot ABI: player, moves, pushes, solved, then box cells.
function snapshot(from: WasmGame): Snapshot {
  const s = from.snapshot();
  return { player: s[0], moves: s[1], pushes: s[2], solved: s[3] === 1, boxes: s.subarray(4) };
}
function render() {
  state = snapshot(game);
  board.draw(game.width(), game.height(), tiles, labels, state, Array.from(labels, (_, i) => game.on_goal(i)));
  setText('moves', String(state.moves));
  setText('pushes', String(state.pushes));
  updateButtons();
}
function saveSession() {
  if (game) progress.session(game.actions());
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
    const result = snapshot(check);
    if (!result.solved) throw new Error('Route does not solve this puzzle');
    return result;
  } finally {
    check.free();
  }
}
function changed() {
  render();
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
  const next = new WasmGame(text);
  try {
    if (actions) next.replay(actions);
  } catch (error) {
    next.free();
    throw error;
  }
  if (game) invalidate();
  game?.free();
  game = next;
  current = { ...puzzle, text };
  tiles = game.tiles();
  labels = game.labels();
  select('puzzles').value = puzzle.id;
  setText('title', puzzle.title);
  setText('difficulty', puzzle.difficulty);
  setText('hint', puzzle.hint || 'Match every box to its goal.');
  $<HTMLTextAreaElement>('rows').value = text;
  render();
  progress.select(current.id, current.text);
  message(actions ? 'Session restored by replaying your moves.' : MOVE_HINT);
  saveSession();
}
function move(direction: number) {
  if (!game) return;
  if (solver.busy || solver.route !== undefined || playback.active) invalidate();
  if (game.step(direction)) changed();
  else if (game.moves() >= MAX_ROUTE)
    message(`Session move limit reached (${MAX_ROUTE}). Undo or restart to continue.`);
}
// Every transport route is replayed on a scratch Rust game before display.
function applyUpdate(update: SearchUpdate, route: string | undefined) {
  const { expanded, generated, reservedBytes, lowerBound, proof, status } = update.metrics;
  setText('expanded', expanded.toLocaleString());
  setText('generated', generated.toLocaleString());
  setText('reserved', `${(reservedBytes / 1048576).toFixed(1)} MiB`);
  const note = proof.kind === 'optimal' ? ' · proven move-optimal from this position'
    : proof.kind === 'unsolvable' ? ' · proven unsolvable'
    // Worker routes arrive throttled, so the gap is measured on the route shown.
    : route !== undefined && lowerBound !== undefined ? ` · within ${route.length - lowerBound} of optimal`
    : ' · optimality unproven';
  const result = route !== undefined ? `${route.length} remaining moves${note}. ` : '';
  const verdict: Record<SearchStatus, string> = {
    running: 'Searching…',
    solved: 'Search complete.',
    exhausted: proof.kind === 'unsolvable'
      ? 'No solution exists from this position.'
      : 'Search ended without finding a route (not a proof — use Optimal to prove unsolvability).',
    state_limit: 'State limit reached.',
    memory_limit: 'Memory limit reached.',
    time_limit: 'Time budget reached.',
    cancelled: 'Stopped.',
  };
  setStatus(result + (verdict[status] ?? status));
}
function solve() {
  if (solver.busy) return;
  invalidate();
  const request: SolveRequest = {
    rows: current.text,
    actions: game.actions(),
    mode: select('mode').value,
    timeMs: Number(select('seconds').value) * 1000,
    memoryMiB: Number(select('memory').value),
    maxStates: MAX_STATES,
  };
  for (const metric of ['expanded', 'generated', 'reserved', 'elapsed']) setText(metric, '—');
  setStatus('Preparing search…');
  void solver.solve(select('engine').value, request);
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
  selector.add(new Option('Custom puzzle', 'custom'));
  const saved = storage.session();
  const savedPuzzle = saved?.id === 'custom' ? customPuzzle(saved.rows) : catalog.find(p => p.id === saved?.id);
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
    if (selector.value === 'custom') {
      selector.value = current.id;
      $<HTMLTextAreaElement>('rows').closest('details')!.open = true;
      return;
    }
    const puzzle = catalog.find(p => p.id === selector.value);
    if (puzzle) load(puzzle);
    // Arrow keys must move the robot, not walk the dropdown after a change.
    selector.blur();
  };
  button('next').onclick = () => {
    const index = catalog.findIndex(p => p.id === current.id);
    load(catalog[(index + 1) % catalog.length]);
  };
  button('undo').onclick = () => {
    invalidate();
    if (game.undo()) changed();
  };
  button('reset').onclick = () => {
    invalidate();
    game.reset();
    changed();
  };
  button('best').onclick = () => {
    const best = progress.best();
    if (!best) return;
    invalidate();
    game.reset();
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
      game.replay(actions);
      invalidate();
      changed();
    } catch (error) {
      message(errorMessage(error));
    }
  };
  for (const control of document.querySelectorAll<HTMLButtonElement>('[data-direction]'))
    control.onclick = () => move(Number(control.dataset.direction));
  // Swipe on the board moves the robot; taps stay with the on-screen buttons.
  // Only a clean one-finger swipe counts: a second finger (pinch), a cancelled
  // touch, or any scroll during the gesture discards it, and an oversized
  // board that must pan takes no swipes at all.
  const canvas = $<HTMLCanvasElement>('board');
  const scrollOffsets = () =>
    `${scrollX},${scrollY},${canvas.parentElement!.scrollLeft},${canvas.parentElement!.scrollTop}`;
  let swipe: { id: number; x: number; y: number; scroll: string } | undefined;
  canvas.addEventListener('touchstart', event => {
    const touch = event.changedTouches[0];
    swipe = event.touches.length === 1 && board.fits
      ? { id: touch.identifier, x: touch.clientX, y: touch.clientY, scroll: scrollOffsets() }
      : undefined;
  }, { passive: true });
  // A second finger that lands off the board still makes this a pinch.
  document.addEventListener('touchstart', event => {
    if (event.touches.length > 1) swipe = undefined;
  }, { passive: true });
  canvas.addEventListener('touchcancel', () => {
    swipe = undefined;
  }, { passive: true });
  canvas.addEventListener('touchend', event => {
    const gesture = swipe;
    swipe = undefined;
    const touch = event.changedTouches[0];
    if (
      !gesture
      || event.touches.length !== 0
      || touch.identifier !== gesture.id
      || scrollOffsets() !== gesture.scroll
    ) return;
    const dx = touch.clientX - gesture.x, dy = touch.clientY - gesture.y;
    const ax = Math.abs(dx), ay = Math.abs(dy);
    // A swipe travels 24 px and at least twice as far along one axis as the other.
    if (Math.max(ax, ay) < 24 || Math.max(ax, ay) < 2 * Math.min(ax, ay)) return;
    move(ax > ay ? (dx > 0 ? 3 : 2) : (dy > 0 ? 1 : 0));
  }, { passive: true });
  document.addEventListener('keydown', event => {
    if (
      event.ctrlKey
      || event.metaKey
      || event.altKey
      || (event.target as HTMLElement).matches('input,textarea,select')
    ) return;
    const key = event.key.toLowerCase();
    const directions: Record<string, number> = {
      arrowup: 0,
      w: 0,
      arrowdown: 1,
      s: 1,
      arrowleft: 2,
      a: 2,
      arrowright: 3,
      d: 3,
    };
    if (key in directions) {
      event.preventDefault();
      move(directions[key]);
    } else if (key === 'z') {
      event.preventDefault();
      button('undo').click();
    }
  });
  button('solve').onclick = () => { void solve(); };
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
  try {
    const response = await fetch('/api/health', { signal: AbortSignal.timeout(1500) });
    if (response.ok) {
      const health = await response.json();
      progress.persistence = health.persistence === true;
      setText('connection', progress.persistence ? 'PostgreSQL connected' : 'Native solver connected');
      if (progress.persistence) void progress.pull();
    }
  } catch {
    /* Static hosting needs no backend. */
  }
}
start().catch(error => message(`Could not start WebAssembly: ${errorMessage(error)}. Run npm run wasm and reload.`));
