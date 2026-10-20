import { ACTIONS, type Snapshot } from './protocol.ts';
import { decodeSnapshot } from './transport.ts';

/** The replay study's model: a route replayed on the engine frame by frame,
 * and the comparison of two such routes — where they share a prefix, where
 * they first differ, and what each costs. The engine is injected (WasmGame
 * satisfies it structurally), so the model stays testable while the Rust
 * engine stays the source of truth: every frame comes from a real replay. */

/** The engine surface a trace needs — WasmGame's own shape. */
export interface Engine {
  width(): number;
  height(): number;
  tiles(): Uint8Array;
  labels(): Uint8Array;
  step(direction: number): boolean;
  snapshot(): Uint32Array;
  on_goal(index: number): boolean;
}

/** One route, rebuilt: the position after every move, each position's
 * boxes-on-goals, and which steps pushed. */
export interface Trace {
  actionLog: string;
  width: number;
  height: number;
  tiles: Uint8Array;
  labels: Uint8Array;
  frames: Snapshot[];
  onGoals: boolean[][];
  pushedSteps: boolean[];
}

/** Replays `actionLog` on the engine, capturing every frame. Null when a
 * step is blocked or the route does not solve — the study shows only routes
 * the engine can rebuild. The tiles and labels are copied, so the engine
 * may be freed once the trace returns. */
export function build(engine: Engine, actionLog: string): Trace | null {
  const tiles = engine.tiles().slice();
  const labels = engine.labels().slice();
  const count = labels.length;
  const frames: Snapshot[] = [];
  const onGoals: boolean[][] = [];
  const pushedSteps: boolean[] = [];
  const capture = () => {
    frames.push(decodeSnapshot(engine.snapshot(), count));
    onGoals.push(Array.from({ length: count }, (_, index) => engine.on_goal(index)));
  };
  capture();
  for (const letter of actionLog) {
    const direction = ACTIONS.indexOf(letter);
    if (direction === -1 || !engine.step(direction)) return null;
    const pushesBefore = frames[frames.length - 1]!.pushes;
    capture();
    pushedSteps.push(frames[frames.length - 1]!.pushes > pushesBefore);
  }
  if (!frames[frames.length - 1]!.solved) return null;
  return { actionLog, width: engine.width(), height: engine.height(), tiles, labels, frames, onGoals, pushedSteps };
}

/** One frame of a trace, with its goal states. */
export interface Position {
  state: Snapshot;
  onGoal: boolean[];
}

/** The trace's position at `step`, bounded to its route. */
export function atStep(trace: Trace, step: number): Position {
  const bounded = Math.max(0, Math.min(Math.floor(step), trace.frames.length - 1));
  return { state: trace.frames[bounded]!, onGoal: trace.onGoals[bounded]! };
}

/** Where two routes diverge: a timeline marker, seekable by its step. */
export interface Marker {
  step: number;
  kind: 'direction' | 'push' | 'finish';
  symbol: 'D' | 'P' | 'F';
  label: string;
}

export interface Comparison {
  commonPrefixMoves: number;
  moveDelta: number;
  pushDelta: number;
  markers: Marker[];
  summary: string;
}

const DIRECTIONS: Record<string, string> = { U: 'up', D: 'down', L: 'left', R: 'right' };

function routeAt(log: string, index: number): string {
  return DIRECTIONS[log[index]!] ?? 'finished';
}

function deltaPhrase(delta: number, noun: string): string {
  if (delta === 0) return `the same number of ${noun}`;
  return `${Math.abs(delta)} ${noun} ${delta < 0 ? 'fewer' : 'more'}`;
}

/** Compares a watched route with a reference: their shared prefix, the first
 * direction and cumulative-push differences, the finish of the shorter, and
 * a plain-language summary of the whole gap. */
export function compare(primary: Trace, reference: Trace): Comparison {
  const shared = Math.min(primary.actionLog.length, reference.actionLog.length);
  let common = 0;
  while (common < shared && primary.actionLog[common] === reference.actionLog[common]) common++;
  const sameLog = primary.actionLog === reference.actionLog;
  const firstActionDifference = sameLog ? undefined : common + 1;
  let firstPushDifference: number | undefined;
  const steps = Math.max(primary.pushedSteps.length, reference.pushedSteps.length);
  let primaryPushes = 0;
  let referencePushes = 0;
  for (let index = 0; index < steps; index++) {
    if (primary.pushedSteps[index]) primaryPushes++;
    if (reference.pushedSteps[index]) referencePushes++;
    if (primaryPushes !== referencePushes) {
      firstPushDifference = index + 1;
      break;
    }
  }
  const markers: Marker[] = [];
  if (firstActionDifference !== undefined) {
    const index = firstActionDifference - 1;
    markers.push({
      step: Math.min(firstActionDifference, primary.actionLog.length),
      kind: 'direction',
      symbol: 'D',
      label: `Direction changes at move ${firstActionDifference}: ${routeAt(primary.actionLog, index)} instead of ${routeAt(reference.actionLog, index)}.`,
    });
  }
  if (firstPushDifference !== undefined && firstPushDifference !== firstActionDifference) {
    markers.push({
      step: Math.min(firstPushDifference, primary.actionLog.length),
      kind: 'push',
      symbol: 'P',
      label: `Cumulative push counts first differ after move ${firstPushDifference}.`,
    });
  }
  if (primary.actionLog.length !== reference.actionLog.length) {
    const shorter = Math.min(primary.actionLog.length, reference.actionLog.length);
    markers.push({
      step: Math.min(shorter, primary.actionLog.length),
      kind: 'finish',
      symbol: 'F',
      label: `The shorter route finishes at move ${shorter}.`,
    });
  }
  const moveDelta = primary.actionLog.length - reference.actionLog.length;
  const pushDelta = primary.frames[primary.frames.length - 1]!.pushes - reference.frames[reference.frames.length - 1]!.pushes;
  const divergence = sameLog
    ? 'The routes use the same directions.'
    : `They match for ${common} ${common === 1 ? 'move' : 'moves'}, then the watched route goes ${routeAt(primary.actionLog, common)} while the comparison goes ${routeAt(reference.actionLog, common)}.`;
  return {
    commonPrefixMoves: common,
    moveDelta,
    pushDelta,
    markers,
    summary: `${divergence} The watched route uses ${deltaPhrase(moveDelta, 'moves')} and ${deltaPhrase(pushDelta, 'pushes')}.`,
  };
}

/** How a trace's step reads, for the study's live description. */
export function describe(trace: Trace, step: number, label: string): string {
  const bounded = Math.max(0, Math.min(Math.floor(step), trace.actionLog.length));
  const state = trace.frames[bounded]!;
  const row = Math.floor(state.player / trace.width) + 1;
  const column = (state.player % trace.width) + 1;
  return `${label}, move ${bounded} of ${trace.actionLog.length}. Keeper at row ${row}, column ${column}. ${state.pushes} ${state.pushes === 1 ? 'push' : 'pushes'}. ${state.solved ? 'Puzzle solved.' : 'Puzzle in progress.'}`;
}
