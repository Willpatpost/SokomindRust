import './style.css';
import init, { WasmGame } from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import catalog from '../../data/puzzles.json';
import { BoardView } from './board';
import * as storage from './storage';
import type { SearchUpdate, SolveRequest, WorkerReply } from './protocol';

interface Puzzle { id: string; title: string; difficulty: string; rows: string[]; hint?: string }
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const button = (id: string) => $<HTMLButtonElement>(id);
const select = (id: string) => $<HTMLSelectElement>(id);
// role=status re-announces every write, so identical text is left alone.
const message = (value: string) => { const el = $('message'); if (el.textContent !== value) el.textContent = value; };
const board = new BoardView($<HTMLCanvasElement>('board'));
let game: WasmGame;
let current: Puzzle;
let tiles: Uint8Array;
let labels: Uint8Array;
let snapshot: Uint32Array;
let worker: Worker | undefined;
let requestAbort: AbortController | undefined;
let busy = false;
let generation = 0;
let runPrefix = '';
let route: string | undefined;
let timer: ReturnType<typeof setInterval> | undefined;
let watchdog: ReturnType<typeof setTimeout> | undefined;
let saveTimer: ReturnType<typeof setTimeout> | undefined;
let playback: { actions: string; index: number; timer: ReturnType<typeof setInterval> } | undefined;
let pausedRoute: { actions: string; index: number } | undefined;
let persistence = false;
const profile = storage.profile();

function updateButtons() {
  button('solve').disabled = busy || !game || snapshot[3] === 1;
  button('cancel').disabled = !busy && !playback && !pausedRoute;
  button('play').disabled = busy || (!route && !playback && !pausedRoute);
  button('copy').disabled = !route;
  button('undo').disabled = !game || snapshot[1] === 0;
}
function endRun() {
  busy = false; worker?.terminate(); worker = undefined;
  requestAbort = undefined; clearInterval(timer); clearTimeout(watchdog);
  updateButtons();
}
function stopPlayback() {
  if (playback) clearInterval(playback.timer);
  playback = undefined; button('play').textContent = 'Play route';
}
// Playback leaves the robot away from where the route starts, so a finished,
// blocked, or stopped playback drops the route instead of offering a replay.
function endPlayback() { stopPlayback(); pausedRoute = undefined; route = undefined; updateButtons(); }
function animate(actions: string, start = 0) {
  stopPlayback();
  pausedRoute = undefined;
  button('play').textContent = 'Pause';
  playback = { actions, index: start, timer: setInterval(() => {
    if (!playback) return;
    if (playback.index >= playback.actions.length) { endPlayback(); return; }
    if (!game.step('UDLR'.indexOf(playback.actions[playback.index++]))) { endPlayback(); message('Replay was blocked.'); return; }
    changed();
  }, 70) };
  updateButtons();
}
function invalidate() {
  generation++; requestAbort?.abort(); endRun(); stopPlayback(); pausedRoute = undefined; route = undefined; updateButtons();
  setStatus('Ready when you are.');
}
// role=status re-announces on every text change, so only write real changes.
function setStatus(text: string) { const el = $('search-status'); if (el.textContent !== text) el.textContent = text; }
function render() {
  snapshot = game.snapshot();
  const onGoal = new Uint8Array(labels.length);
  for (let i = 0; i < labels.length; i++) onGoal[i] = game.on_goal(i) ? 1 : 0;
  board.draw(game.width(), game.height(), tiles, labels, snapshot, onGoal);
  $('moves').textContent = String(snapshot[1]); $('pushes').textContent = String(snapshot[2]);
  updateButtons();
}
function saveSession() {
  if (!game) return;
  if (!storage.write('session', { id: current.id, rows: current.rows.join('\n'), actions: game.actions() })) {
    $('storage').textContent = 'Browser storage is unavailable. This session has not been saved.';
  }
}
function updateBest() {
  const best = readBest();
  button('best').disabled = !best;
  $('best-score').textContent = best ? `Best: ${best.moves} moves · ${best.pushes} pushes` : '';
}
function verify(fullRoute: string): Uint32Array {
  const check = new WasmGame(current.rows.join('\n'));
  try { check.replay(fullRoute); const state = check.snapshot(); if (!state[3]) throw new Error('Route does not solve this puzzle'); return state; }
  finally { check.free(); }
}
function readBest(): storage.Best | null {
  const best = storage.best(current.id, current.rows.join('\n'));
  if (!best) return null;
  try {
    const state = verify(best.route);
    return state[1] === best.moves && state[2] === best.pushes ? best : null;
  } catch { return null; }
}
function keepBest(fullRoute: string) {
  const state = verify(fullRoute);
  const old = readBest();
  if (!old || state[1] < old.moves || (state[1] === old.moves && state[2] < old.pushes)) {
    if (!storage.write('best.' + current.id, { rows: current.rows.join('\n'), route: fullRoute, moves: state[1], pushes: state[2] })) {
      $('storage').textContent = 'Could not save the best route in this browser.';
    }
  }
  updateBest();
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
    const response = await fetch(`/api/progress/${encodeURIComponent(current.id)}`, { method: 'POST', headers: { 'content-type': 'application/json', 'x-profile-id': profile }, body: JSON.stringify({ route: fullRoute }), signal: AbortSignal.timeout(5000) });
    if (!response.ok) {
      // A rejected save (bad input, rate limit) is final; 5xx means the server could not save.
      $('storage').textContent = response.status === 429 ? 'Server save rate-limited; try again shortly. Local progress is kept.'
        : response.status < 500 ? `Server save rejected: ${(await errorText(response)) ?? `HTTP ${response.status}`}` : failed;
      return;
    }
    const result = await response.json();
    $('storage').textContent = result.improved
      ? 'Verified best route saved in PostgreSQL for this browser profile.'
      : 'Server already stored an equal or better route for this puzzle.';
  } catch { $('storage').textContent = failed; }
}
async function pullBest() {
  if (!persistence || !profile || current.id === 'custom') return;
  const id = current.id;
  try {
    const response = await fetch(`/api/progress/${encodeURIComponent(id)}`, { headers: { 'x-profile-id': profile }, signal: AbortSignal.timeout(5000) });
    if (!response.ok) return;
    const result = await response.json();
    if (current.id === id && typeof result.route === 'string') keepBest(result.route);
  } catch { /* An optional read failure does not interrupt play. */ }
}
function changed() {
  render(); clearTimeout(saveTimer); saveTimer = setTimeout(saveSession, 350);
  message(snapshot[3] ? `Solved in ${snapshot[1]} moves and ${snapshot[2]} pushes.` : 'Arrow keys or WASD to move. Z to undo.');
  if (snapshot[3]) { keepBest(game.actions()); void syncBest(game.actions()); saveSession(); }
}
function load(puzzle: Puzzle, actions = '') {
  const next = new WasmGame(puzzle.rows.join('\n'));
  try { if (actions) next.replay(actions); }
  catch (error) { next.free(); throw error; }
  if (game) invalidate();
  game?.free(); game = next; current = puzzle;
  tiles = game.tiles(); labels = game.labels();
  select('puzzles').value = puzzle.id;
  $('title').textContent = puzzle.title; $('difficulty').textContent = puzzle.difficulty;
  $('hint').textContent = puzzle.hint || 'Match every box to its goal.';
  $<HTMLTextAreaElement>('rows').value = puzzle.rows.join('\n');
  render(); updateBest();
  message(actions ? 'Session restored by replaying your moves.' : 'Arrow keys or WASD to move. Z to undo.');
  saveSession(); void pullBest();
}
function move(direction: number) {
  if (!game) return;
  if (busy || route || playback || pausedRoute) invalidate();
  if (game.step(direction)) changed();
  else if (game.moves() >= 100000) message('Session move limit reached (100000). Undo or restart to continue.');
}
function applyUpdate(update: SearchUpdate) {
  if (update.route !== undefined) {
    if (runPrefix.length + update.route.length > 100000) {
      throw new Error('Position and route together exceed the 100000-move replay limit');
    }
    verify(runPrefix + update.route);
    if (game.actions() !== runPrefix) throw new Error('Puzzle changed while solving');
    route = update.route;
  }
  const m = update.metrics;
  $('expanded').textContent = m[0].toLocaleString(); $('generated').textContent = m[1].toLocaleString();
  $('reserved').textContent = `${(m[2] / 1048576).toFixed(1)} MiB`;
  const hasRoute = route !== undefined;
  const proofKind = m[4];
  const note = proofKind === 2 ? ' · proven move-optimal from this position'
    : proofKind === 3 ? ' · proven unsolvable'
    // Worker routes arrive throttled, so the gap is measured on the route shown.
    : hasRoute && m[5] !== 0xffffffff ? ` · within ${route!.length - m[5]} of optimal`
    : ' · optimality unproven';
  const result = hasRoute ? `${route!.length} remaining moves${note}. ` : '';
  const status: Record<string, string> = { running: 'Searching…', solved: 'Search complete.', exhausted: proofKind === 3 ? 'No solution exists from this position.' : 'Search ended without finding a route (not a proof — use Optimal to prove unsolvability).', state_limit: 'State limit reached.', memory_limit: 'Memory limit reached.', time_limit: 'Time budget reached.', cancelled: 'Stopped.' };
  setStatus(result + (status[update.status] || update.status));
  // The run timer owns the elapsed display; the final value lands once, here.
  if (update.type === 'done') { endRun(); $('elapsed').textContent = `${(update.elapsedMs / 1000).toFixed(1)} s`; }
  else updateButtons();
}
function fail(error: unknown) { setStatus(error instanceof Error ? error.message : String(error)); endRun(); }
async function solve() {
  if (busy) return;
  invalidate(); busy = true; runPrefix = game.actions(); updateButtons();
  const id = generation;
  const request: SolveRequest = { rows: current.rows.join('\n'), actions: runPrefix, mode: select('mode').value,
    timeMs: Number(select('seconds').value) * 1000, memoryMiB: Number(select('memory').value), maxStates: 500_000 };
  $('expanded').textContent = '—'; $('generated').textContent = '—';
  $('reserved').textContent = '—'; $('elapsed').textContent = '—';
  setStatus('Preparing search…');
  const started = performance.now();
  timer = setInterval(() => { $('elapsed').textContent = `${((performance.now() - started) / 1000).toFixed(1)} s`; }, 150);
  if (select('engine').value === 'browser') {
    worker = new Worker(new URL('./solver.worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = ({ data }: MessageEvent<WorkerReply>) => {
      if (id !== generation) return;
      try { if (data.type === 'error') fail(data.message); else applyUpdate(data); } catch (error) { fail(error); }
    };
    worker.onerror = (event) => { if (id === generation) fail(event.message || 'Worker failed to start'); };
    worker.postMessage({ type: 'solve', request });
    // Includes startup allowance; termination releases the whole WASM arena.
    watchdog = setTimeout(() => { if (id === generation && busy) { setStatus(route ? 'Stopped at deadline. Verified route retained.' : 'Worker deadline reached.'); endRun(); } }, request.timeMs + 2000);
  } else {
    requestAbort = new AbortController();
    watchdog = setTimeout(() => requestAbort?.abort(), request.timeMs + 5000);
    try {
      const response = await fetch('/api/solve', { method: 'POST', headers: { 'content-type': 'application/json' }, signal: requestAbort.signal,
        body: JSON.stringify({ rows: current.rows, actions: request.actions, mode: request.mode, time_ms: request.timeMs, max_states: request.maxStates, memory_mib: request.memoryMiB }) });
      if (!response.ok) {
        const error = (await errorText(response)) ?? `Server returned HTTP ${response.status}`;
        // A rate-limited or busy server leaves the in-browser solver available.
        throw new Error(response.status === 429 && !/browser/i.test(error) ? `${error}. The browser solver still works: set Run on to This browser.` : error);
      }
      const r = await response.json();
      if (id !== generation) return;
      const proofKinds: Record<string, number> = { bounded: 1, optimal: 2, unsolvable: 3 };
      applyUpdate({ type: 'done', status: r.status, route: r.route ?? undefined, elapsedMs: r.elapsed_ms,
        metrics: new Uint32Array([r.expanded, r.generated, r.reserved_bytes, r.moves ?? 0xffffffff,
          r.proof ? proofKinds[r.proof.kind] ?? 0 : 0, r.proof?.lower_bound ?? 0xffffffff]) });
    } catch (error) {
      if (id === generation) fail(error instanceof DOMException && error.name === 'AbortError'
        ? `Server search exceeded its ${request.timeMs / 1000} s budget.`
        : error);
    }
  }
}

async function start() {
  await init({ module_or_path: wasmUrl });
  const selector = select('puzzles');
  for (const puzzle of catalog) selector.add(new Option(`${puzzle.title} · ${puzzle.difficulty}`, puzzle.id));
  selector.add(new Option('Custom puzzle', 'custom'));
  const saved = storage.session();
  const savedPuzzle = saved?.id === 'custom' ? { id: 'custom', title: 'Your puzzle', difficulty: 'custom', rows: saved.rows.split('\n') } : catalog.find(p => p.id === saved?.id);
  const canRestore = savedPuzzle && savedPuzzle.rows.join('\n') === saved?.rows;
  // A stale saved layout falls back to the saved puzzle itself, not puzzle one.
  const target = savedPuzzle ?? catalog[0];
  try { load(target, canRestore ? saved.actions : ''); }
  catch {
    try { load(target); }
    catch { load(catalog[0]); message('Saved session was invalid. Started a fresh puzzle.'); }
  }
  selector.onchange = () => {
    if (selector.value === 'custom') { selector.value = current.id; $<HTMLTextAreaElement>('rows').closest('details')!.open = true; return; }
    const puzzle = catalog.find(p => p.id === selector.value); if (puzzle) load(puzzle);
    // Arrow keys must move the robot, not walk the dropdown after a change.
    selector.blur();
  };
  button('next').onclick = () => { const index = catalog.findIndex(p => p.id === current.id); load(catalog[(index + 1) % catalog.length]); };
  button('undo').onclick = () => { invalidate(); if (game.undo()) changed(); };
  button('reset').onclick = () => { invalidate(); game.reset(); changed(); };
  button('best').onclick = () => {
    const best = readBest(); if (!best) return;
    try { verify(best.route); invalidate(); game.reset(); render(); animate(best.route); } catch (error) { message(String(error)); }
  };
  button('load-custom').onclick = () => {
    try { load({ id: 'custom', title: 'Your puzzle', difficulty: 'custom', rows: $<HTMLTextAreaElement>('rows').value.replace(/\r/g, '').replace(/^\n|\n$/g, '').split('\n') }); }
    catch (error) { message(String(error)); }
  };
  button('load-route').onclick = () => {
    const actions = $<HTMLTextAreaElement>('route-input').value.toUpperCase().replace(/\s/g, '');
    if (!actions) { message('Route is empty.'); return; }
    if (actions.length > 100000) { message('Route exceeds 100000 moves.'); return; }
    try {
      // Rust validates the whole replay atomically before replacing the current state.
      game.replay(actions); invalidate(); changed();
    } catch (error) { message(String(error)); }
  };
  for (const control of document.querySelectorAll<HTMLButtonElement>('[data-direction]')) control.onclick = () => move(Number(control.dataset.direction));
  // Swipe on the board moves the robot; taps stay with the on-screen buttons.
  // Only a clean one-finger swipe counts: a second finger (pinch), a cancelled
  // touch, or any scroll during the gesture discards it, and an oversized
  // board that must pan takes no swipes at all.
  const canvas = $<HTMLCanvasElement>('board');
  const scrollOffsets = () => `${scrollX},${scrollY},${canvas.parentElement!.scrollLeft},${canvas.parentElement!.scrollTop}`;
  let swipe: { id: number; x: number; y: number; scroll: string } | undefined;
  canvas.addEventListener('touchstart', event => {
    const touch = event.changedTouches[0];
    swipe = event.touches.length === 1 && board.fits
      ? { id: touch.identifier, x: touch.clientX, y: touch.clientY, scroll: scrollOffsets() } : undefined;
  }, { passive: true });
  // A second finger that lands off the board still makes this a pinch.
  document.addEventListener('touchstart', event => { if (event.touches.length > 1) swipe = undefined; }, { passive: true });
  canvas.addEventListener('touchcancel', () => { swipe = undefined; }, { passive: true });
  canvas.addEventListener('touchend', event => {
    const gesture = swipe; swipe = undefined;
    const touch = event.changedTouches[0];
    if (!gesture || event.touches.length !== 0 || touch.identifier !== gesture.id || scrollOffsets() !== gesture.scroll) return;
    const dx = touch.clientX - gesture.x, dy = touch.clientY - gesture.y;
    const ax = Math.abs(dx), ay = Math.abs(dy);
    // A swipe travels 24 px and at least twice as far along one axis as the other.
    if (Math.max(ax, ay) < 24 || Math.max(ax, ay) < 2 * Math.min(ax, ay)) return;
    move(ax > ay ? (dx > 0 ? 3 : 2) : (dy > 0 ? 1 : 0));
  }, { passive: true });
  document.addEventListener('keydown', event => {
    if (event.ctrlKey || event.metaKey || event.altKey || (event.target as HTMLElement).matches('input,textarea,select')) return;
    const key = event.key.toLowerCase();
    const directions: Record<string, number> = { arrowup: 0, w: 0, arrowdown: 1, s: 1, arrowleft: 2, a: 2, arrowright: 3, d: 3 };
    if (key in directions) { event.preventDefault(); move(directions[key]); }
    else if (key === 'z') { event.preventDefault(); button('undo').click(); }
  });
  button('solve').onclick = () => { void solve(); };
  button('cancel').onclick = () => {
    if (playback || pausedRoute) { endPlayback(); message('Playback stopped.'); return; }
    if (worker) {
      // The grace period covers rebuilding the final best route, which the worker sends last.
      worker.postMessage({ type: 'cancel' }); clearTimeout(watchdog);
      watchdog = setTimeout(() => { setStatus(route ? 'Stopped. Verified route retained.' : 'Stopped.'); endRun(); }, 1000);
    } else { generation++; requestAbort?.abort(); setStatus('Stopped waiting for the native search.'); endRun(); }
  };
  button('play').onclick = () => {
    if (playback) {
      pausedRoute = { actions: playback.actions, index: playback.index };
      stopPlayback(); button('play').textContent = 'Resume'; updateButtons();
    } else if (pausedRoute) {
      animate(pausedRoute.actions, pausedRoute.index);
    } else if (route) {
      animate(route);
    }
  };
  button('copy').onclick = async () => {
    try { await navigator.clipboard.writeText(runPrefix + (route || '')); message('Full route copied as U/D/L/R.'); }
    catch { message('Clipboard is unavailable in this browser.'); }
  };
  const modeHelp: Record<string, string> = { fast: 'Prioritizes a quick first route. It may use extra moves and never proves optimality.', quality: 'Keeps the best verified route and searches for shorter routes until the budget ends. It never proves optimality.', optimal: 'Exact A* minimizes total moves and can prove a puzzle unsolvable. If a limit stops it early, a route it found is still reported as proven move-optimal or within N moves of optimal.' };
  const syncModeHelp = () => { $('mode-help').textContent = modeHelp[select('mode').value] ?? ''; };
  select('mode').onchange = syncModeHelp;
  // The browser can restore the select's value on reload; match the help text.
  syncModeHelp();
  addEventListener('resize', () => render());
  addEventListener('pagehide', saveSession);
  try {
    const response = await fetch('/api/health', { signal: AbortSignal.timeout(1500) });
    if (response.ok) {
      const health = await response.json(); persistence = health.persistence === true;
      $('connection').textContent = persistence ? 'PostgreSQL connected' : 'Native solver connected';
      if (persistence) void pullBest();
    }
  } catch { /* Static hosting needs no backend. */ }
}
start().catch(error => message(`Could not start WebAssembly: ${String(error)}. Run npm run wasm and reload.`));
