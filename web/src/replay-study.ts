/** The replay study: the dialog over a room's verified routes, watched step
 * by step on the board, with the comparison of two attempts. */
import { WasmGame } from '../wasm/sokomind';
import { BoardView } from './board.ts';
import { type Entry } from './catalog.ts';
import { clock } from './timer.ts';
import { openDialog, closeDialog } from './dialog.ts';
import { type Snapshot } from './protocol.ts';
import * as replays from './replays.ts';
import * as traces from './traces.ts';
import { idb } from './idb.ts';
import { $, button, select, setText } from './dom.ts';
import { verifyRoute } from './play.ts';

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

/** Binds the study dialog's controls once: watch and compare, seek and step,
 * play — and the keys the open dialog owns. */
function bindStudy() {
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
}

export { bindStudy, openStudy };
