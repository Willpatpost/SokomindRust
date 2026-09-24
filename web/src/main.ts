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
const message = (value: string) => { $('message').textContent = value; };
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
let playback: ReturnType<typeof setInterval> | undefined;
let persistence = false;
const profile = storage.profile();

function updateButtons() {
  button('solve').disabled = busy || !game || snapshot[3] === 1;
  button('cancel').disabled = !busy;
  button('play').disabled = !route || busy;
  button('copy').disabled = !route;
  button('undo').disabled = !game || snapshot[1] === 0;
}
function endRun() {
  busy = false; worker?.terminate(); worker = undefined;
  requestAbort = undefined; clearInterval(timer); clearTimeout(watchdog);
  updateButtons();
}
function stopPlayback() { clearInterval(playback); playback = undefined; button('play').textContent = 'Play route'; }
function invalidate() {
  generation++; requestAbort?.abort(); endRun(); stopPlayback(); route = undefined; updateButtons();
  $('search-status').textContent = 'Ready when you are.';
}
function render() {
  snapshot = game.snapshot();
  board.draw(game.width(), game.height(), tiles, labels, snapshot);
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
async function syncBest(fullRoute: string) {
  if (!persistence || !profile || current.id === 'custom') return;
  try {
    const response = await fetch(`/api/progress/${encodeURIComponent(current.id)}`, { method: 'POST', headers: { 'content-type': 'application/json', 'x-profile-id': profile }, body: JSON.stringify({ route: fullRoute }), signal: AbortSignal.timeout(5000) });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    $('storage').textContent = 'Verified best route saved in PostgreSQL for this browser profile.';
  } catch { $('storage').textContent = 'Server save failed. Local progress is still available if browser storage is enabled.'; }
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
  if (busy || route || playback) invalidate();
  if (game.step(direction)) changed();
}
function applyUpdate(update: SearchUpdate) {
  if (update.route !== undefined) {
    verify(runPrefix + update.route);
    if (game.actions() !== runPrefix) throw new Error('Puzzle changed while solving');
    route = update.route;
  }
  const m = update.metrics;
  $('expanded').textContent = m[0].toLocaleString(); $('generated').textContent = m[1].toLocaleString();
  $('reserved').textContent = `${(m[2] / 1048576).toFixed(1)} MiB`;
  $('elapsed').textContent = `${(update.elapsedMs / 1000).toFixed(1)} s`;
  const hasRoute = route !== undefined;
  const result = hasRoute ? `${route!.length} remaining moves${m[4] ? ' · proven move-optimal from this position' : ' · optimality unproven'}. ` : '';
  const status: Record<string, string> = { running: 'Searching…', solved: 'Search complete.', exhausted: 'No solution from this position.', state_limit: 'State limit reached.', memory_limit: 'Memory limit reached.', time_limit: 'Time budget reached.', cancelled: 'Stopped.' };
  $('search-status').textContent = result + (status[update.status] || update.status);
  if (update.type === 'done') endRun(); else updateButtons();
}
function fail(error: unknown) { $('search-status').textContent = String(error); endRun(); }
async function solve() {
  if (busy) return;
  invalidate(); busy = true; runPrefix = game.actions(); updateButtons();
  const id = generation;
  const request: SolveRequest = { rows: current.rows.join('\n'), actions: runPrefix, mode: select('mode').value,
    timeMs: Number(select('seconds').value) * 1000, memoryMiB: Number(select('memory').value), maxStates: 500_000 };
  $('search-status').textContent = 'Preparing search…';
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
    watchdog = setTimeout(() => { if (id === generation && busy) { $('search-status').textContent = route ? 'Stopped at deadline. Verified route retained.' : 'Worker deadline reached.'; endRun(); } }, request.timeMs + 2000);
  } else {
    requestAbort = new AbortController();
    watchdog = setTimeout(() => requestAbort?.abort(), request.timeMs + 5000);
    try {
      const response = await fetch('/api/solve', { method: 'POST', headers: { 'content-type': 'application/json' }, signal: requestAbort.signal,
        body: JSON.stringify({ rows: current.rows, actions: request.actions, mode: request.mode, time_ms: request.timeMs, max_states: request.maxStates, memory_mib: request.memoryMiB }) });
      if (!response.ok) { const error = await response.json().catch(() => null); throw new Error(error?.error || `Server returned HTTP ${response.status}`); }
      const r = await response.json();
      if (id !== generation) return;
      applyUpdate({ type: 'done', status: r.status, route: r.route ?? undefined, elapsedMs: r.elapsed_ms,
        metrics: new Uint32Array([r.expanded, r.generated, r.reserved_bytes, r.moves ?? 0xffffffff, r.proven ? 1 : 0]) });
    } catch (error) { if (id === generation) fail(error); }
  }
}
function animate(actions: string) {
  stopPlayback(); let index = 0; button('play').textContent = 'Pause';
  playback = setInterval(() => {
    if (index >= actions.length) { stopPlayback(); route = undefined; updateButtons(); return; }
    if (!game.step('UDLR'.indexOf(actions[index++]))) { stopPlayback(); message('Replay was blocked.'); return; }
    changed();
  }, 70);
}

async function start() {
  await init({ module_or_path: wasmUrl });
  const selector = select('puzzles');
  for (const puzzle of catalog) selector.add(new Option(`${puzzle.title} · ${puzzle.difficulty}`, puzzle.id));
  selector.add(new Option('Custom puzzle', 'custom'));
  const saved = storage.session();
  const savedPuzzle = saved?.id === 'custom' ? { id: 'custom', title: 'Your puzzle', difficulty: 'custom', rows: saved.rows.split('\n') } : catalog.find(p => p.id === saved?.id);
  const canRestore = savedPuzzle && savedPuzzle.rows.join('\n') === saved?.rows;
  try { load(canRestore ? savedPuzzle : catalog[0], canRestore ? saved.actions : ''); }
  catch { load(catalog[0]); message('Saved session was invalid. Started a fresh puzzle.'); }
  selector.onchange = () => {
    if (selector.value === 'custom') { selector.value = current.id; $<HTMLTextAreaElement>('rows').closest('details')!.open = true; return; }
    const puzzle = catalog.find(p => p.id === selector.value); if (puzzle) load(puzzle);
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
    try {
      const actions = $<HTMLTextAreaElement>('route-input').value.toUpperCase().replace(/\s/g, '');
      // Rust validates the whole replay atomically before replacing the current state.
      game.replay(actions); invalidate(); changed();
    } catch (error) { message(String(error)); }
  };
  for (const control of document.querySelectorAll<HTMLButtonElement>('[data-direction]')) control.onclick = () => move(Number(control.dataset.direction));
  document.addEventListener('keydown', event => {
    if (event.ctrlKey || event.metaKey || event.altKey || (event.target as HTMLElement).matches('input,textarea,select')) return;
    const key = event.key.toLowerCase();
    const directions: Record<string, number> = { arrowup: 0, w: 0, arrowdown: 1, s: 1, arrowleft: 2, a: 2, arrowright: 3, d: 3 };
    if (key in directions) { event.preventDefault(); move(directions[key]); }
    else if (key === 'z') { event.preventDefault(); button('undo').click(); }
  });
  button('solve').onclick = () => { void solve(); };
  button('cancel').onclick = () => {
    if (worker) {
      worker.postMessage({ type: 'cancel' }); clearTimeout(watchdog);
      watchdog = setTimeout(() => { $('search-status').textContent = route ? 'Stopped. Verified route retained.' : 'Stopped.'; endRun(); }, 250);
    } else { generation++; requestAbort?.abort(); $('search-status').textContent = 'Stopped waiting for the native search.'; endRun(); }
  };
  button('play').onclick = () => { if (playback) { stopPlayback(); route = undefined; updateButtons(); } else if (route) animate(route); };
  button('copy').onclick = async () => {
    try { await navigator.clipboard.writeText(runPrefix + (route || '')); message('Full route copied as U/D/L/R.'); }
    catch { message('Clipboard is unavailable in this browser.'); }
  };
  select('mode').onchange = () => {
    $('mode-help').textContent = ({ fast: 'Prioritizes a quick first route. It may use extra moves.', quality: 'Keeps the best verified route and searches for shorter routes until the budget ends.', optimal: 'Exact A* minimizes total moves. A limited search makes no optimality claim.' } as Record<string, string>)[select('mode').value];
  };
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
