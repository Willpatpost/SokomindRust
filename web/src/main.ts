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
  type Proof,
  type SearchStatus,
  type SearchUpdate,
  type SolveRequest,
  type WorkerReply,
} from './protocol';

interface Puzzle { id: string; title: string; difficulty: string; rows: string[]; hint?: string }
// One search. While busy it holds its worker or request and timers; afterwards
// it keeps the route found from `prefix` for Play and Copy until the position changes.
interface Run {
  prefix: string;
  busy: boolean;
  route?: string;
  worker?: Worker;
  abort?: AbortController;
  timer?: number;
  watchdog?: number;
}
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
let run: Run | undefined;
// A route playing on the board; paused, it keeps its place for Resume.
let playback: { route: string; index: number; timer: number; paused: boolean } | undefined;
let saveTimer: number | undefined;
let persistence = false;
const profile = storage.profile();

function updateButtons() {
  const busy = run?.busy === true;
  button('solve').disabled = busy || !game || state.solved;
  button('cancel').disabled = !busy && !playback;
  button('play').disabled = busy || (!run?.route && !playback);
  setText('play', !playback ? 'Play route' : playback.paused ? 'Resume' : 'Pause');
  button('copy').disabled = !run?.route;
  button('undo').disabled = !game || state.moves === 0;
}
function endRun() {
  if (run) {
    run.busy = false;
    run.worker?.terminate();
    run.abort?.abort();
    run.worker = undefined;
    run.abort = undefined;
    clearInterval(run.timer);
    clearTimeout(run.watchdog);
  }
  updateButtons();
}
function stopPlayback() {
  if (playback) clearInterval(playback.timer);
  playback = undefined;
}
// Playback leaves the robot away from where the route starts, so a finished,
// blocked, or stopped playback drops the route instead of offering a replay.
function endPlayback() {
  stopPlayback();
  if (run) run.route = undefined;
  updateButtons();
}
function animate(route: string, index = 0) {
  stopPlayback();
  playback = {
    route,
    index,
    paused: false,
    timer: setInterval(() => {
      if (!playback) return;
      if (playback.index >= playback.route.length) {
        endPlayback();
        return;
      }
      if (!game.step('UDLR'.indexOf(playback.route[playback.index++]))) {
        endPlayback();
        message('Replay was blocked.');
        return;
      }
      changed();
    }, 70),
  };
  updateButtons();
}
function invalidate() {
  endRun();
  run = undefined;
  stopPlayback();
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
function saveSession(actions?: string) {
  if (!game) return;
  if (!storage.write('session', { id: current.id, rows: current.text, actions: actions ?? game.actions() })) {
    setText('storage', 'Browser storage is unavailable. This session has not been saved.');
  }
}
function showBest(best: storage.Best | null) {
  button('best').disabled = !best;
  setText('best-score', best ? `Best: ${best.moves} moves · ${best.pushes} pushes` : '');
}
// Stored routes are untrusted: replay one from the start on a scratch game.
function verify(route: string): Snapshot {
  const check = new WasmGame(current.text);
  try {
    check.replay(route);
    const result = snapshot(check);
    if (!result.solved) throw new Error('Route does not solve this puzzle');
    return result;
  } finally {
    check.free();
  }
}
function readBest(): storage.Best | null {
  const best = storage.best(current.id, current.text);
  if (!best) return null;
  try {
    const result = verify(best.route);
    return result.moves === best.moves && result.pushes === best.pushes ? best : null;
  } catch {
    return null;
  }
}
// `route` solves from the start and its counters come from a Rust replay.
function keepBest(route: string, moves: number, pushes: number) {
  const old = readBest();
  let best = old;
  if (!old || moves < old.moves || (moves === old.moves && pushes < old.pushes)) {
    const next = { rows: current.text, route, moves, pushes };
    if (storage.write('best.' + current.id, next)) best = next;
    else setText('storage', 'Could not save the best route in this browser.');
  }
  showBest(best);
}
// API errors carry {error}; a proxy's own error page has none.
async function errorText(response: Response): Promise<string | undefined> {
  const body: { error?: unknown } | null = await response.json().catch(() => null);
  return typeof body?.error === 'string' ? body.error : undefined;
}
async function syncBest(fullRoute: string) {
  if (!persistence || !profile || current.id === 'custom') return;
  const failed = 'Server save failed. Local progress is still available if browser storage is enabled.';
  try {
    const response = await fetch(`/api/progress/${encodeURIComponent(current.id)}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-profile-id': profile },
      body: JSON.stringify({ route: fullRoute }),
      signal: AbortSignal.timeout(5000),
    });
    if (!response.ok) {
      // A rejected save (bad input, rate limit) is final; 5xx means the server could not save.
      setText(
        'storage',
        response.status === 429 ? 'Server save rate-limited; try again shortly. Local progress is kept.'
          : response.status < 500 ? `Server save rejected: ${(await errorText(response)) ?? `HTTP ${response.status}`}`
          : failed,
      );
      return;
    }
    const result = await response.json();
    setText('storage', result.improved
      ? 'Verified best route saved in PostgreSQL for this browser profile.'
      : 'Server already stored an equal or better route for this puzzle.');
  } catch {
    setText('storage', failed);
  }
}
async function pullBest() {
  if (!persistence || !profile || current.id === 'custom') return;
  const id = current.id;
  try {
    const response = await fetch(`/api/progress/${encodeURIComponent(id)}`, {
      headers: { 'x-profile-id': profile },
      signal: AbortSignal.timeout(5000),
    });
    if (!response.ok) return;
    const result = await response.json();
    if (current.id !== id || typeof result.route !== 'string') return;
    const checked = verify(result.route);
    keepBest(result.route, checked.moves, checked.pushes);
  } catch {
    /* An optional read failure does not interrupt play. */
  }
}
function changed() {
  render();
  clearTimeout(saveTimer);
  if (!state.solved) {
    message(MOVE_HINT);
    saveTimer = setTimeout(() => saveSession(), 350);
    return;
  }
  message(`Solved in ${state.moves} moves and ${state.pushes} pushes.`);
  const actions = game.actions();
  saveSession(actions);
  keepBest(actions, state.moves, state.pushes);
  void syncBest(actions);
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
  showBest(readBest());
  message(actions ? 'Session restored by replaying your moves.' : MOVE_HINT);
  saveSession();
  void pullBest();
}
function move(direction: number) {
  if (!game) return;
  if (run?.busy || run?.route || playback) invalidate();
  if (game.step(direction)) changed();
  else if (game.moves() >= MAX_ROUTE)
    message(`Session move limit reached (${MAX_ROUTE}). Undo or restart to continue.`);
}
// Routes arrive already replayed by Rust: the worker's solution() or the server.
function applyUpdate(thisRun: Run, update: SearchUpdate) {
  if (update.route !== undefined) {
    // A route valid on its own can still overflow the replay limit after the prefix.
    if (thisRun.prefix.length + update.route.length > MAX_ROUTE) {
      throw new Error(`Position and route together exceed the ${MAX_ROUTE}-move replay limit`);
    }
    thisRun.route = update.route;
  }
  const { expanded, generated, reservedBytes, lowerBound, proof, status } = update.metrics;
  setText('expanded', expanded.toLocaleString());
  setText('generated', generated.toLocaleString());
  setText('reserved', `${(reservedBytes / 1048576).toFixed(1)} MiB`);
  const route = thisRun.route;
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
  // The run timer owns the elapsed display; the final value lands once, here.
  if (update.type === 'done') {
    endRun();
    setText('elapsed', seconds(update.elapsedMs));
  } else updateButtons();
}
function fail(error: unknown) {
  setStatus(errorMessage(error));
  endRun();
}
async function solve() {
  if (run?.busy) return;
  invalidate();
  const thisRun: Run = { prefix: game.actions(), busy: true };
  run = thisRun;
  updateButtons();
  const request: SolveRequest = {
    rows: current.text,
    actions: thisRun.prefix,
    mode: select('mode').value,
    timeMs: Number(select('seconds').value) * 1000,
    memoryMiB: Number(select('memory').value),
    maxStates: MAX_STATES,
  };
  setText('expanded', '—');
  setText('generated', '—');
  setText('reserved', '—');
  setText('elapsed', '—');
  setStatus('Preparing search…');
  const started = performance.now();
  thisRun.timer = setInterval(() => setText('elapsed', seconds(performance.now() - started)), 150);
  if (select('engine').value === 'browser') {
    const worker = new Worker(new URL('./solver.worker.ts', import.meta.url), { type: 'module' });
    thisRun.worker = worker;
    worker.onmessage = ({ data }: MessageEvent<WorkerReply>) => {
      if (thisRun !== run) return;
      try {
        if (data.type === 'error') fail(data.message);
        else applyUpdate(thisRun, data);
      } catch (error) {
        fail(error);
      }
    };
    worker.onerror = (event) => {
      if (thisRun === run) fail(event.message || 'Worker failed to start');
    };
    worker.postMessage({ type: 'solve', request });
    // Includes startup allowance; termination releases the whole WASM arena.
    thisRun.watchdog = setTimeout(() => {
      if (thisRun === run && thisRun.busy) {
        setStatus(thisRun.route ? 'Stopped at deadline. Verified route retained.' : 'Worker deadline reached.');
        endRun();
      }
    }, request.timeMs + 2000);
  } else {
    const abort = new AbortController();
    thisRun.abort = abort;
    thisRun.watchdog = setTimeout(() => abort.abort(), request.timeMs + 5000);
    try {
      const response = await fetch('/api/solve', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        signal: abort.signal,
        body: JSON.stringify({
          rows: current.rows,
          actions: request.actions,
          mode: request.mode,
          time_ms: request.timeMs,
          max_states: request.maxStates,
          memory_mib: request.memoryMiB,
        }),
      });
      if (!response.ok) {
        const error = (await errorText(response)) ?? `Server returned HTTP ${response.status}`;
        // A rate-limited or busy server leaves the in-browser solver available.
        throw new Error(response.status === 429 && !/browser/i.test(error)
          ? `${error}. The browser solver still works: set Run on to This browser.`
          : error);
      }
      const r = await response.json();
      if (thisRun !== run) return;
      const p = r.proof;
      const proof: Proof = !p ? { kind: 'none' }
        : p.kind === 'optimal' ? { kind: 'optimal', moves: p.upper_bound }
        : p.kind === 'bounded' ? { kind: 'bounded', lower: p.lower_bound, upper: p.upper_bound }
        : { kind: 'unsolvable' };
      applyUpdate(thisRun, {
        type: 'done',
        route: r.route ?? undefined,
        elapsedMs: r.elapsed_ms,
        metrics: {
          expanded: r.expanded,
          generated: r.generated,
          reservedBytes: r.reserved_bytes,
          best: r.moves ?? undefined,
          lowerBound: p?.lower_bound,
          proof,
          status: r.status,
        },
      });
    } catch (error) {
      if (thisRun === run) fail(error instanceof DOMException && error.name === 'AbortError'
        ? `Server search exceeded its ${request.timeMs / 1000} s budget.`
        : error);
    }
  }
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
    const best = readBest();
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
    if (playback) {
      endPlayback();
      message('Playback stopped.');
      return;
    }
    const thisRun = run;
    if (!thisRun?.busy) return;
    if (thisRun.worker) {
      // The grace period covers rebuilding the final best route, which the worker sends last.
      thisRun.worker.postMessage({ type: 'cancel' });
      clearTimeout(thisRun.watchdog);
      thisRun.watchdog = setTimeout(() => {
        setStatus(thisRun.route ? 'Stopped. Verified route retained.' : 'Stopped.');
        endRun();
      }, 1000);
    } else {
      endRun();
      run = undefined;
      setStatus('Stopped waiting for the native search.');
    }
  };
  button('play').onclick = () => {
    if (playback && !playback.paused) {
      clearInterval(playback.timer);
      playback.paused = true;
      updateButtons();
    } else if (playback) animate(playback.route, playback.index);
    else if (run?.route) animate(run.route);
  };
  button('copy').onclick = async () => {
    if (!run?.route) return;
    try {
      await navigator.clipboard.writeText(run.prefix + run.route);
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
      persistence = health.persistence === true;
      setText('connection', persistence ? 'PostgreSQL connected' : 'Native solver connected');
      if (persistence) void pullBest();
    }
  } catch {
    /* Static hosting needs no backend. */
  }
}
start().catch(error => message(`Could not start WebAssembly: ${errorMessage(error)}. Run npm run wasm and reload.`));
