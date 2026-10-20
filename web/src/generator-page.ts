/** The generator: the workshop's room-forger. The model grows the candidates;
 * the engine alone certifies them — every room the dialog offers is one the
 * search itself solved, and the typed pairs ride the traced route. */
import { WasmGame } from '../wasm/sokomind';
import { DIFFICULTIES } from './catalog.ts';
import { closeDialog, openDialog } from './dialog.ts';
import { MAX_STATES, WALL } from './protocol.ts';
import { decodeSnapshot } from './transport.ts';
import { SolverClient } from './solver-client.ts';
import * as editor from './editor.ts';
import * as generator from './generator.ts';
import { $, button, choice, select, setText } from './dom.ts';
import { verify } from './play.ts';
import { editorAct, editorNotice, editorState } from './editor-page.ts';

/** How long one candidate may take the engine to solve. */
const GENERATOR_TIME_MS = 10_000;

/** The room the current generation search runs on, as its verify reads it. */
let genRows: string | undefined;
/** The resolver of the solve the client is running, when one is pending. */
let genPending: ((solution: generator.Solution | null) => void) | undefined;
/** The parameters the dialog shows. */
let genParams: generator.GeneratorParams = { ...generator.DEFAULT_PARAMS };
/** Whether a generation is running. */
let genRunning = false;
/** The run's stop flag: Cancel and Close both raise it. */
let genAbort = { aborted: false };
/** The last successful result, held for its Load button. */
let genResult: Extract<generator.GenerationResult, { status: 'success' }> | undefined;

/** The generator's own search client: a fourth engine instance, verifying
 * against the candidate rows instead of the played session. */
const genSolver = new SolverClient({
  worker: () => new Worker(new URL('./solver.worker.ts', import.meta.url), { type: 'module' }),
  changed: () => {
    settleGenerator();
    syncGeneratorButtons();
  },
  elapsed: () => {},
  update: () => {},
  status: value => setText('generator-status', value),
  verify: (prefix, route) => {
    if (genRows === undefined) throw new Error('No room is growing');
    verify(genRows, prefix + route);
  },
});

/** Ends the pending solve from the client's completed state: a retained
 * route still counts, whatever ended the search. */
function settleGenerator() {
  if (genSolver.busy || genPending === undefined) return;
  const settle = genPending;
  genPending = undefined;
  const route = genSolver.route;
  if (route === undefined || genRows === undefined) {
    settle(null);
    return;
  }
  try {
    const game = new WasmGame(genRows);
    game.replay(route);
    const state = decodeSnapshot(game.snapshot(), game.labels().length);
    const solution: generator.Solution = { moves: state.moves, pushes: state.pushes, route };
    game.free();
    settle(solution);
  } catch {
    settle(null);
  }
}

/** Solves a candidate: the promise settles when the client completes. */
function genSolve(rows: string[]): Promise<generator.Solution | null> {
  // The rows travel as a const: the module's genRows cannot narrow into the
  // promise's closure below.
  const text = rows.join('\n');
  genRows = text;
  return new Promise(resolve => {
    void genSolver
      .solve('browser', { rows: text, actions: '', mode: 'fast', timeMs: GENERATOR_TIME_MS, memoryMiB: 128, maxStates: MAX_STATES })
      .then(() => {
        genPending = resolve;
        // A search that never started, or finished inside the call, ends
        // here: the changed callback fired before genPending existed.
        settleGenerator();
      });
  });
}

/** Pairs each box with the goal the traced route lands it on, read off the
 * engine's own snapshots. */
function genTrace(rows: string[], route: string): ReadonlyArray<{ box: generator.Position; goal: generator.Position }> | null {
  try {
    const game = new WasmGame(rows.join('\n'));
    const labels = game.labels();
    const start = decodeSnapshot(game.snapshot(), labels.length);
    game.replay(route);
    const end = decodeSnapshot(game.snapshot(), labels.length);
    const tiles = game.tiles();
    const width = game.width();
    game.free();
    if (!end.solved) return null;
    const pairing: Array<{ box: generator.Position; goal: generator.Position }> = [];
    for (let index = 0; index < labels.length; index++) {
      const goalCell = end.boxes[index]!;
      if (tiles[goalCell] === 0 || tiles[goalCell] === WALL) return null;
      pairing.push({
        box: { row: Math.floor(start.boxes[index]! / width), column: start.boxes[index]! % width },
        goal: { row: Math.floor(goalCell / width), column: goalCell % width },
      });
    }
    return pairing;
  } catch {
    return null;
  }
}

function syncGeneratorButtons() {
  button('generator-run').disabled = genRunning;
  button('generator-cancel').disabled = !genRunning;
}

/** Shows the parameters in the dialog's fields. */
function syncGeneratorInputs() {
  $<HTMLInputElement>('generator-attempts').value = String(genParams.maxAttempts);
  $<HTMLInputElement>('generator-width').value = String(genParams.width);
  $<HTMLInputElement>('generator-height').value = String(genParams.height);
  $<HTMLInputElement>('generator-boxes').value = String(genParams.boxCount);
  const labels = $<HTMLInputElement>('generator-labels');
  labels.checked = genParams.useLabels;
  labels.disabled = genParams.boxCount < 2;
}

/** Runs a generation, reporting its progress and holding the result for the
 * Load button. */
async function runGenerator() {
  if (genRunning) return;
  genRunning = true;
  genAbort = { aborted: false };
  syncGeneratorButtons();
  $('generator-result').hidden = true;
  const progress = $<HTMLProgressElement>('generator-progress');
  progress.value = 0;
  progress.hidden = false;
  const result = await generator.generate(
    genParams,
    { solve: genSolve, trace: genTrace },
    update => {
      setText('generator-status', update.message);
      progress.value = update.attempt / update.maxAttempts;
    },
    genAbort,
  );
  genRunning = false;
  syncGeneratorButtons();
  progress.hidden = true;
  if (result.status === 'success') {
    genResult = result;
    setText('generator-result-title', 'Puzzle generated');
    setText('generator-result-summary', `A ${result.difficulty}-level room the engine solved itself.`);
    setText('generator-moves', String(result.moves));
    setText('generator-pushes', String(result.pushes));
    setText('generator-attempts-taken', String(result.attempts));
    $('generator-metrics').hidden = false;
    button('generator-accept').hidden = false;
    $('generator-result').hidden = false;
    setText('generator-status', `Read as ${result.difficulty}.`);
  } else {
    genResult = undefined;
    setText('generator-result-title', 'Generation failed');
    setText(
      'generator-result-summary',
      `No room landed within a tier of ${genParams.targetDifficulty} in ${result.attempts} attempts. ${result.lastReason}.`,
    );
    $('generator-metrics').hidden = true;
    button('generator-accept').hidden = true;
    $('generator-result').hidden = false;
  }
}

/** Binds the generator's dialog: the parameters, the run, and the load. */
function bindGenerator() {
  const difficulty = select('generator-difficulty');
  for (const value of DIFFICULTIES) difficulty.add(new Option(value.charAt(0).toUpperCase() + value.slice(1), value));
  difficulty.value = genParams.targetDifficulty;
  difficulty.onchange = () => {
    genParams.targetDifficulty = choice('generator-difficulty', DIFFICULTIES);
    const preset = generator.PRESETS[genParams.targetDifficulty];
    if (preset !== undefined) {
      genParams.width = preset.width;
      genParams.height = preset.height;
      genParams.boxCount = preset.boxCount;
    }
    syncGeneratorInputs();
  };
  const clamp = (value: number, min: number, max: number) => (Number.isFinite(value) ? Math.max(min, Math.min(max, value)) : min);
  const attempts = $<HTMLInputElement>('generator-attempts');
  attempts.addEventListener('change', () => {
    genParams.maxAttempts = clamp(Number(attempts.value), 1, 500);
    syncGeneratorInputs();
  });
  const width = $<HTMLInputElement>('generator-width');
  width.addEventListener('change', () => {
    // The generator's rooms cap at 20, inside the editor's MAX_SIZE (26), so a
    // grown room always fits the editor it opens in.
    genParams.width = clamp(Number(width.value), 5, 20);
    syncGeneratorInputs();
  });
  const height = $<HTMLInputElement>('generator-height');
  height.addEventListener('change', () => {
    genParams.height = clamp(Number(height.value), 5, 20);
    syncGeneratorInputs();
  });
  const boxes = $<HTMLInputElement>('generator-boxes');
  boxes.addEventListener('change', () => {
    genParams.boxCount = clamp(Number(boxes.value), 1, 10);
    syncGeneratorInputs();
  });
  const labels = $<HTMLInputElement>('generator-labels');
  labels.addEventListener('change', () => {
    genParams.useLabels = labels.checked;
  });
  syncGeneratorInputs();

  button('editor-generate').onclick = () => {
    setText('generator-status', 'Configure a room and generate.');
    $('generator-result').hidden = true;
    openDialog('generator');
  };
  button('generator-close').onclick = () => {
    genAbort.aborted = true;
    if (genSolver.busy) genSolver.cancel();
    closeDialog('generator');
  };
  button('generator-cancel').onclick = () => {
    genAbort.aborted = true;
    if (genSolver.busy) genSolver.cancel();
    setText('generator-status', 'Stopping…');
  };
  button('generator-run').onclick = () => void runGenerator();
  button('generator-accept').onclick = () => {
    if (genResult === undefined || editorState === undefined) return;
    editorAct(editor.loadPuzzle(editorState, { title: 'Generated Puzzle', difficulty: genResult.difficulty, rows: genResult.rows }));
    closeDialog('generator');
    editorNotice('Generated room loaded. Test it before sharing.');
  };
}

export { bindGenerator };
