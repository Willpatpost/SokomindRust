/** The Solver Lab: a workspace over one room and position, where searches
 * run, their results are watched step by step, and two runs compare. The UI
 * over lab.ts's model. */
import { WasmGame } from '../wasm/sokomind';
import { BoardView } from './board.ts';
import { ENTRIES, type Entry } from './catalog.ts';
import { navigate, type Route } from './router.ts';
import { playHash } from './shell.ts';
import { MODES, MAX_STATES, type SearchUpdate } from './protocol.ts';
import { ENGINES, SolverClient } from './solver-client.ts';
import { statusText } from './verdict.ts';
import * as lab from './lab.ts';
import * as optimal from './optimal.ts';
import * as traces from './traces.ts';
import { $, button, choice, select, setText } from './dom.ts';
import { GLYPHS, seconds, verify } from './play.ts';

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

export { bindLab, renderLab };
