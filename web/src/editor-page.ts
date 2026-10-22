/** The puzzle editor: local drafts, the paintable grid, the shared-link preview
 * and the private playtest. The engine stays the source of truth — a grid only
 * tests and shares once the Rust parse of its rows succeeds. */
import { WasmGame } from '../wasm/sokomind';
import { BoardView, hue } from './board.ts';
import { DIFFICULTIES } from './catalog.ts';
import { bindInput } from './input.ts';
import { walkRoute } from './walk.ts';
import { formatHash, navigate, route, type Route } from './router.ts';
import { openDialog, closeDialog } from './dialog.ts';
import { ACTIONS, errorMessage, type Snapshot } from './protocol.ts';
import { decodeSnapshot } from './transport.ts';
import * as journal from './journal.ts';
import * as storage from './storage.ts';
import * as editor from './editor.ts';
import * as audio from './audio.ts';
import { $, button, choice, select, setText } from './dom.ts';
import { engineReady, goalsOn, sound } from './play.ts';

/** The editor's own board, for the playtest. */
const editorBoard = new BoardView($('editor-board'));

/** The drafts this browser keeps, loaded on the editor's first mount. */
let drafts: editor.DraftStore | undefined;
/** The grid on show: the active draft's, or a shared preview's read-only view. */
let editorState: editor.EditorState | undefined;
/** The undo history around it; a tool change never enters. */
let editorPast: editor.EditorState[] = [];
let editorFuture: editor.EditorState[] = [];
/** Whether the grid changed since its draft last autosaved. */
let editorDirty = false;
let editorSaveTimer: number | undefined;
/** The puzzle a #/editor?custom= link carries, decoded, when the route has one. */
let shared: editor.SharedPuzzle | undefined;
/** The custom parameter the editor last rendered, to catch the route dropping it. */
let editorCustom: string | undefined;
/** A live playtest, over the grid's rows or a shared puzzle's. */
let playtest: { game: WasmGame; tiles: Uint8Array; labels: Uint8Array; state: Snapshot; title: string } | undefined;
/** The grid's keyboard focus, the one cell the roving tabindex makes tabbable. */
let focusCell = { row: 0, column: 0 };
/** Whether painting floods connected cells instead of replacing one. */
let fillMode = false;
/** The grid's cell buttons, indexed [row][column]. */
let cellButtons: HTMLButtonElement[][] = [];
/** The rows the engine last parsed, and what it said: parsing runs only when
 * the rows change, so typing in the details never re-parses the grid. */
let verdictRows: string | undefined;
let verdictError: string | undefined;

/** The rooms a never-painted draft can start from. */
const STARTERS: editor.LoadablePuzzle[] = [
  {
    title: 'Single push starter',
    difficulty: 'beginner',
    hint: 'Push the box directly onto the goal.',
    rows: ['OOOOOOO', 'O     O', 'O RXS O', 'O     O', 'OOOOOOO'],
  },
  {
    title: 'Turn the corner starter',
    difficulty: 'beginner',
    hint: 'Try shaping a route that approaches the box from two sides.',
    rows: ['OOOOOOOOO', 'O       O', 'O R     O', 'O   X   O', 'O     S O', 'O       O', 'OOOOOOOOO'],
  },
];

/** The keys that select a paint tool, one per tool in toolbar order. */
const TOOL_KEYS: Record<string, string> = Object.fromEntries(editor.TOOLS.map((tool, index) => [String(index + 1), tool.symbol]));

const editorNotice = (value: string) => setText('editor-notice', value);

/** The glyph a cell shows: walls read as W, the floor as a dot. */
function cellGlyph(symbol: string): string {
  if (symbol === 'O') return 'W';
  if (symbol === ' ') return '·';
  if (symbol === 'S') return 'G';
  return symbol;
}

/** The glyph a toolbar tool shows; the goal tool paints a ring, not a letter. */
function toolGlyph(symbol: string): string {
  if (symbol === 'O') return 'W';
  if (symbol === ' ') return '·';
  if (symbol === 'S') return '◎';
  return symbol;
}

/** A cell's name, for the labels that read the grid. */
function cellName(symbol: string): string {
  if (symbol === 'O') return 'wall';
  if (symbol === ' ') return 'floor';
  if (symbol === 'R') return 'robot';
  if (symbol === 'X') return 'generic box';
  if (symbol === 'S') return 'generic goal';
  if (editor.isTypedBox(symbol)) return `typed box ${symbol}`;
  if (editor.isTypedGoal(symbol)) return `typed goal ${symbol}`;
  return 'unknown cell';
}

/** Writes one cell's symbol: its data attribute, glyph, label, and — for a
 * typed pair — the hue its box and goal share with the playing board. */
function updateCell(cell: HTMLButtonElement, symbol: string, tool: string) {
  cell.dataset.symbol = symbol;
  cell.textContent = cellGlyph(symbol);
  cell.setAttribute(
    'aria-label',
    `Row ${Number(cell.dataset.row) + 1}, column ${Number(cell.dataset.column) + 1}: ${cellName(symbol)}. Paint ${cellName(tool)}.`,
  );
  if (editor.isTypedBox(symbol)) {
    const tone = hue(symbol.charCodeAt(0));
    cell.style.background = `hsl(${tone} 45% 55%)`;
    cell.style.borderColor = `hsl(${tone} 45% 45%)`;
    cell.style.borderStyle = 'solid';
    cell.style.color = 'var(--paper-50)';
  } else if (editor.isTypedGoal(symbol)) {
    const tone = hue(symbol.toUpperCase().charCodeAt(0));
    cell.style.background = '';
    cell.style.borderColor = `hsl(${tone} 45% 45%)`;
    cell.style.borderStyle = 'dashed';
    cell.style.color = `hsl(${tone} 45% 32%)`;
  } else {
    cell.style.background = '';
    cell.style.borderColor = '';
    cell.style.borderStyle = '';
    cell.style.color = '';
  }
}

/** Builds the grid whole; its shape changed. */
function buildGrid(state: editor.EditorState) {
  const grid = $('editor-grid');
  focusCell = { row: Math.min(focusCell.row, state.height - 1), column: Math.min(focusCell.column, state.width - 1) };
  const rows: HTMLElement[] = [];
  cellButtons = state.cells.map((row, rowIndex) => {
    const rowElement = document.createElement('div');
    rowElement.className = 'editor-row';
    rowElement.setAttribute('role', 'row');
    const buttons = row.map((symbol, columnIndex) => {
      const cell = document.createElement('button');
      cell.type = 'button';
      cell.className = 'editor-cell';
      cell.dataset.row = String(rowIndex);
      cell.dataset.column = String(columnIndex);
      cell.setAttribute('role', 'gridcell');
      cell.tabIndex = rowIndex === focusCell.row && columnIndex === focusCell.column ? 0 : -1;
      updateCell(cell, symbol, state.tool);
      rowElement.append(cell);
      return cell;
    });
    rows.push(rowElement);
    return buttons;
  });
  grid.replaceChildren(...rows);
}

/** Applies a state change to the grid: a rebuild when the shape changed, else
 * only the cells that changed. */
function syncGrid(previous: editor.EditorState | undefined, next: editor.EditorState) {
  const grid = $('editor-grid');
  if (previous === undefined || previous.width !== next.width || previous.height !== next.height) buildGrid(next);
  else
    for (let row = 0; row < next.height; row++)
      for (let column = 0; column < next.width; column++)
        if (previous.cells[row]![column] !== next.cells[row]![column]!)
          updateCell(cellButtons[row]![column]!, next.cells[row]![column]!, next.tool);
  grid.style.setProperty('--editor-columns', String(next.width));
  grid.setAttribute('aria-rowcount', String(next.height));
  grid.setAttribute('aria-colcount', String(next.width));
  const badge = $('editor-size-badge');
  const size = `${next.width} × ${next.height}`;
  if (badge.textContent !== size) badge.textContent = size;
  badge.setAttribute('aria-label', `Grid size: ${next.width} by ${next.height}`);
}

/** Applies an editor action: undoable changes dirty the draft, schedule its
 * autosave, and clear the standing notice and share link; a tool or fill-mode
 * change is the painter's live hand, never history. */
function editorAct(next: editor.EditorState) {
  if (editorState === undefined || next === editorState) return;
  const previous = editorState;
  editorPast.push(previous);
  if (editorPast.length > 50) editorPast.shift();
  editorFuture = [];
  editorDirty = true;
  scheduleEditorSave();
  setText('editor-notice', '');
  $('editor-share-field').hidden = true;
  editorState = next;
  syncGrid(previous, next);
  refreshReadiness();
  syncUndoRedo();
  syncStarters();
}

/** Selects a paint tool. */
function setTool(symbol: string) {
  if (editorState === undefined) return;
  editorState = { ...editorState, tool: symbol };
  syncTools();
}

/** Steps the editor back and forward through its history; the tool in hand
 * survives the trip. */
function editorUndo() {
  if (editorState === undefined || editorPast.length === 0) return;
  const from = editorState;
  // The redo stack reads from its front, so the newest undo returns first.
  editorFuture.unshift(from);
  const target = editorPast.pop()!;
  editorState = { ...target, tool: from.tool };
  editorDirty = true;
  scheduleEditorSave();
  $('editor-share-field').hidden = true;
  syncGrid(from, editorState);
  refreshReadiness();
  syncUndoRedo();
  syncStarters();
}

function editorRedo() {
  if (editorState === undefined || editorFuture.length === 0) return;
  const from = editorState;
  editorPast.push(from);
  const target = editorFuture.shift()!;
  editorState = { ...target, tool: from.tool };
  editorDirty = true;
  scheduleEditorSave();
  $('editor-share-field').hidden = true;
  syncGrid(from, editorState);
  refreshReadiness();
  syncUndoRedo();
  syncStarters();
}

function syncUndoRedo() {
  button('editor-undo').disabled = editorPast.length === 0;
  button('editor-redo').disabled = editorFuture.length === 0;
}

/** What the readiness column says, and whether Test and Share may run: the
 * painter's checks first, then the engine's own parse of the rows — the
 * verdict that matters, straight from Rust. */
function editorVerdict(): { errors: string[]; playable: boolean } {
  if (editorState === undefined) return { errors: [], playable: false };
  const structural = editor.validate(editorState);
  if (!structural.valid) return { errors: structural.errors, playable: false };
  if (!engineReady) return { errors: ['The engine is still starting…'], playable: false };
  const rows = editor.rowsOf(editorState).join('\n');
  if (rows !== verdictRows) {
    verdictRows = rows;
    verdictError = undefined;
    try {
      const game = new WasmGame(rows);
      game.free();
    } catch (error) {
      verdictError = errorMessage(error);
    }
  }
  return verdictError === undefined ? { errors: [], playable: true } : { errors: [verdictError], playable: false };
}

function refreshReadiness() {
  const panel = $('editor-validation');
  if (editorState === undefined) {
    panel.replaceChildren();
    return;
  }
  const verdict = editorVerdict();
  panel.replaceChildren(
    ...verdict.errors.map(error => {
      const item = document.createElement('p');
      item.textContent = error;
      return item;
    }),
  );
  if (verdict.playable) {
    const ok = document.createElement('p');
    ok.className = 'editor-valid';
    ok.textContent = 'Puzzle is valid';
    panel.append(ok);
  }
  button('editor-test').disabled = !verdict.playable;
  button('editor-share').disabled = !verdict.playable;
}

/** The toolbar's tool buttons, and the typed pair's label. */
const toolButtons: HTMLButtonElement[] = [];
let typedBoxButton: HTMLButtonElement;
let typedGoalButton: HTMLButtonElement;
let typedLabel: editor.TypedLabel = 'A';

function syncTools() {
  const tool = editorState?.tool;
  for (const control of toolButtons) control.setAttribute('aria-pressed', String(control.dataset.tool === tool));
  syncTypedButtons();
  // The cells' labels name what a click paints, and the tool just changed.
  if (editorState !== undefined)
    for (const row of cellButtons) for (const control of row) updateCell(control, control.dataset.symbol ?? 'O', editorState.tool);
}

function syncTypedButtons() {
  const pair = (control: HTMLButtonElement, glyph: string, name: string) => {
    const letter = document.createElement('strong');
    letter.textContent = glyph;
    const kind = document.createElement('span');
    kind.textContent = name;
    control.replaceChildren(letter, kind);
  };
  pair(typedBoxButton, typedLabel, 'Box');
  pair(typedGoalButton, typedLabel.toLowerCase(), 'Goal');
  // No aria-label: the rendered pair is the name, and it contains the
  // visible text by construction — a label beside it would have to repeat
  // both (WCAG's label in name).
  const tool = editorState?.tool;
  typedBoxButton.setAttribute('aria-pressed', String(tool === typedLabel));
  typedGoalButton.setAttribute('aria-pressed', String(tool === typedLabel.toLowerCase()));
}

/** The typed kind the pair paints: selecting a label while a goal tool is
 * active keeps painting goals. */
function typedKind(): 'box' | 'goal' {
  return editorState !== undefined && editor.isTypedGoal(editorState.tool) ? 'goal' : 'box';
}

/** The details fields follow the state; typing in them is the state. */
function syncDetails() {
  if (editorState === undefined) return;
  const title = $<HTMLInputElement>('editor-title');
  if (title.value !== editorState.title) title.value = editorState.title;
  select('editor-difficulty').value = editorState.difficulty;
  const hint = $<HTMLTextAreaElement>('editor-hint');
  if (hint.value !== editorState.hint) hint.value = editorState.hint;
}

function syncSizeInputs() {
  if (editorState === undefined) return;
  $<HTMLInputElement>('editor-width').value = String(editorState.width);
  $<HTMLInputElement>('editor-height').value = String(editorState.height);
}

/** The drafts the operations that replace the store start from: the live grid
 * folded into the active draft. */
function savedDrafts(): editor.Draft[] {
  const store = drafts;
  if (store === undefined || !editorDirty || editorState === undefined) return store === undefined ? [] : store.drafts;
  const updatedAt = new Date().toISOString();
  return store.drafts.map(draft => (draft.id === store.activeId ? { ...draft, state: editorState!, updatedAt } : draft));
}

function activeDraft(): editor.Draft {
  const store = drafts!;
  return store.drafts.find(draft => draft.id === store.activeId) ?? store.drafts[0]!;
}

function syncDraftsUi() {
  if (drafts === undefined) return;
  const selector = select('editor-drafts');
  const active = activeDraft();
  selector.replaceChildren(...drafts.drafts.map(draft => new Option(draft.name, draft.id)));
  selector.value = active.id;
  const name = $<HTMLInputElement>('editor-draft-name');
  if (name.value !== active.name) name.value = active.name;
  button('editor-delete').disabled = drafts.drafts.length <= 1;
  setText('editor-draft-saved', active.updatedAt === null ? 'Not saved yet' : `Last saved ${journal.timeAgo(active.updatedAt)}`);
}

/** Loads the draft store on the editor's first mount: a malformed payload is
 * quarantined for download, never silently dropped. */
function loadDrafts() {
  const raw = storage.read(editor.DRAFTS_KEY);
  const parsed = editor.parseDraftStore(raw);
  if (parsed) {
    drafts = parsed;
    return;
  }
  if (raw !== null) {
    storage.write(editor.RECOVERY_KEY, raw);
    storage.remove(editor.DRAFTS_KEY);
  }
  drafts = editor.createDraftStore();
  storage.write(editor.DRAFTS_KEY, drafts);
}

function syncRecovery() {
  const raw = storage.read(editor.RECOVERY_KEY);
  $('editor-recovery').hidden = raw === null;
}

/** Folds the live grid into its draft and writes the store. */
function saveEditorDrafts() {
  if (drafts === undefined || editorState === undefined || !editorDirty) return;
  const updatedAt = new Date().toISOString();
  drafts = {
    activeId: drafts.activeId,
    drafts: drafts.drafts.map(draft => (draft.id === drafts!.activeId ? { ...draft, state: editorState!, updatedAt } : draft)),
  };
  editorDirty = false;
  storage.write(editor.DRAFTS_KEY, drafts);
  syncDraftsUi();
}

/** The autosave's debounce: a stroke paints many cells, the store writes once. */
function scheduleEditorSave() {
  clearTimeout(editorSaveTimer);
  editorSaveTimer = setTimeout(() => saveEditorDrafts(), 1000);
}

/** Flushes a pending autosave: leaving the editor, hiding the page, or entering
 * a shared preview all save first. */
function flushEditorSave() {
  clearTimeout(editorSaveTimer);
  editorSaveTimer = undefined;
  if (editorDirty) saveEditorDrafts();
}

/** Writes the next store and opens `state` as the grid, if one is named: the
 * operation already persisted everything, so no save stays pending. */
function commitDrafts(next: editor.Draft[], activeId: string, state?: editor.EditorState) {
  drafts = { activeId, drafts: next };
  editorDirty = false;
  clearTimeout(editorSaveTimer);
  editorSaveTimer = undefined;
  storage.write(editor.DRAFTS_KEY, drafts);
  if (state !== undefined) openEditorState(state);
  syncDraftsUi();
}

/** Shows a state as the editing grid, with a fresh history around it. */
function openEditorState(state: editor.EditorState) {
  const previous = editorState;
  editorState = state;
  editorPast = [];
  editorFuture = [];
  syncGrid(previous, state);
  syncDetails();
  syncSizeInputs();
  refreshReadiness();
  syncUndoRedo();
  syncStarters();
}

function syncStarters() {
  $('editor-starters').hidden = !(
    editorState !== undefined
    && editor.isBlank(editorState)
    && shared === undefined
    && playtest === undefined
  );
}

/** The editor page's mount. A route with a custom payload shows that puzzle
 * read-only — the painter's draft waits on storage — and leaving the preview
 * restores it, so looking at a link never costs a stroke. */
function renderEditor(next: Route) {
  if (next.kind !== 'editor') return;
  if (drafts === undefined) loadDrafts();
  exitPlaytest();
  const payload = next.custom;
  if (payload !== undefined && payload !== editorCustom) {
    flushEditorSave();
    shared = editor.decodeShare(payload) ?? undefined;
    $('editor-link-error').hidden = shared !== undefined;
    if (shared !== undefined) {
      openEditorState(editor.loadPuzzle(editorState ?? activeDraft().state, shared));
      setText('editor-shared-note', `Your saved draft "${activeDraft().name}" is unchanged.`);
    } else if (editorState === undefined) openEditorState(activeDraft().state);
  }
  if (payload === undefined && editorCustom !== undefined) {
    const restored = editor.parseDraftStore(storage.read(editor.DRAFTS_KEY));
    drafts = restored ?? editor.createDraftStore();
    shared = undefined;
    $('editor-link-error').hidden = true;
  }
  if (editorState === undefined) openEditorState(activeDraft().state);
  editorCustom = payload;
  const preview = payload !== undefined && shared !== undefined;
  $('editor-shared').hidden = !preview;
  button('editor-shared-play').disabled = shared === undefined || !engineReady;
  $<HTMLFieldSetElement>('editor-fields').disabled = preview;
  syncRecovery();
  syncDraftsUi();
  syncDetails();
  syncSizeInputs();
  syncTools();
  syncStarters();
  refreshReadiness();
}

/** Starts a private playtest of a puzzle's rows: a fresh Rust game over the
 * editor's board, its solves never journaling. */
function startPlaytest(puzzle: editor.LoadablePuzzle) {
  let game: WasmGame;
  try {
    game = new WasmGame(puzzle.rows.join('\n'));
  } catch (error) {
    editorNotice(errorMessage(error));
    return;
  }
  const tiles = game.tiles();
  const labels = game.labels();
  playtest = { game, tiles, labels, state: decodeSnapshot(game.snapshot(), labels.length), title: puzzle.title };
  setText('editor-playtest-title', puzzle.title);
  $('editor-playtest').hidden = false;
  $<HTMLFieldSetElement>('editor-fields').hidden = true;
  $<HTMLElement>('editor-playtest-title').focus();
  drawPlaytest();
}

function drawPlaytest() {
  const active = playtest;
  if (active === undefined) return;
  const onGoal = Array.from(active.labels, (_, index) => active.game.on_goal(index));
  editorBoard.draw(active.game.width(), active.game.height(), active.tiles, active.labels, active.state, onGoal);
  setText('editor-moves', String(active.state.moves));
  setText('editor-pushes', String(active.state.pushes));
  const status = $('editor-playtest-status');
  status.dataset.solved = String(active.state.solved);
  const text = active.state.solved
    ? `Solved in ${active.state.moves} ${active.state.moves === 1 ? 'move' : 'moves'}!`
    : 'Playtest in progress';
  if (status.textContent !== text) status.textContent = text;
}

function playtestMove(direction: number) {
  const active = playtest;
  if (active === undefined || active.state.solved) return;
  const beforeGoals = goalsOn(active.game, active.labels);
  const beforePushes = active.state.pushes;
  if (active.game.step(direction)) {
    active.state = decodeSnapshot(active.game.snapshot(), active.labels.length);
    const afterGoals = goalsOn(active.game, active.labels);
    const kind: audio.MoveKind =
      afterGoals > beforeGoals
        ? 'goal-enter'
        : afterGoals < beforeGoals
          ? 'goal-leave'
          : active.state.pushes > beforePushes
            ? 'push'
            : 'step';
    const presentation = audio.moveCue(kind, direction, afterGoals, active.labels.length);
    void sound.playCue(presentation.cue, { pitchOffset: presentation.pitchOffset, variant: presentation.variant });
    drawPlaytest();
  } else {
    const blocked = audio.moveCue('blocked', direction, 0, 0);
    void sound.playCue(blocked.cue, { pitchOffset: blocked.pitchOffset });
    editorBoard.blocked(direction);
  }
}

/** A still touch or click on the playtest board walks the robot to the cell,
 * through floor only, as the play page's taps do. */
function playtestTap(x: number, y: number) {
  const active = playtest;
  if (active === undefined || active.state.solved) return;
  const cell = editorBoard.cellAt(x, y);
  if (cell === undefined) return;
  const route = walkRoute(active.game.width(), active.tiles, active.state.boxes, active.state.player, cell);
  if (route === undefined) return;
  for (const letter of route) {
    const direction = ACTIONS.indexOf(letter);
    if (direction === -1 || !active.game.step(direction)) break;
  }
  active.state = decodeSnapshot(active.game.snapshot(), active.labels.length);
  drawPlaytest();
}

function exitPlaytest() {
  if (playtest === undefined) return;
  playtest.game.free();
  playtest = undefined;
  $('editor-playtest').hidden = true;
  $<HTMLFieldSetElement>('editor-fields').hidden = false;
}

/** Leaving the editor: a pending autosave writes, and a live playtest ends. */
function leaveEditor() {
  flushEditorSave();
  exitPlaytest();
}

/** Shares the grid as a #/editor?custom= link: the field always carries it,
 * the clipboard or share sheet copies it. */
async function shareEditor() {
  if (editorState === undefined) return;
  const puzzle = editor.puzzleOf(editorState);
  const url = `${location.origin}${location.pathname}${formatHash({ kind: 'editor', custom: editor.encodeShare(puzzle) })}`;
  $<HTMLInputElement>('editor-share-url').value = url;
  $('editor-share-field').hidden = false;
  try {
    if (navigator.share !== undefined) {
      await navigator.share({ title: puzzle.title, url });
      return;
    }
    await navigator.clipboard.writeText(url);
    editorNotice('Share link copied.');
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') return;
    editorNotice('The share link is ready below. Select it to copy manually.');
  }
}

/** Paints one cell, or floods it in fill mode. */
function paintCell(row: number, column: number) {
  if (editorState === undefined) return;
  editorAct(fillMode ? editor.floodFill(editorState, row, column) : editor.paint(editorState, row, column));
}

/** Paints at a point: the cell button under it, when there is one. */
function paintAt(x: number, y: number) {
  const element = document.elementFromPoint(x, y)?.closest('button.editor-cell');
  if (element instanceof HTMLButtonElement) paintCell(Number(element.dataset.row), Number(element.dataset.column));
}

/** Moves the grid's roving focus, clamped to the grid. */
function focusGridCell(row: number, column: number) {
  if (editorState === undefined) return;
  const next = {
    row: Math.max(0, Math.min(editorState.height - 1, row)),
    column: Math.max(0, Math.min(editorState.width - 1, column)),
  };
  focusCell = next;
  cellButtons[next.row]?.[next.column]?.focus();
}

/** The pointer that is painting, if one is: a mouse or pen drags its paint
 * along; a touch waits for proof it never panned the canvas. */
let activePointer:
  { id: number; pointerType: string; startX: number; startY: number; row: number; column: number; moved: boolean } | undefined;

/** Binds the editor page's controls once; renderEditor repaints per route. */
function bindEditor() {
  // The toolbar: grouped tool buttons, then the typed pair.
  const tools = $('editor-tools');
  for (const group of ['terrain', 'pieces'] as const) {
    const groupElement = document.createElement('div');
    groupElement.className = 'editor-tool-group';
    const label = document.createElement('p');
    label.textContent = group === 'terrain' ? 'Terrain' : 'Pieces';
    const row = document.createElement('div');
    row.className = 'editor-tool-row';
    for (const tool of editor.TOOLS.filter(item => item.group === group)) {
      const control = document.createElement('button');
      control.type = 'button';
      control.className = 'editor-tool';
      control.dataset.tool = tool.symbol;
      control.textContent = toolGlyph(tool.symbol);
      const shortcut = editor.TOOLS.indexOf(tool) + 1;
      control.setAttribute('aria-label', `${tool.label} (${shortcut})`);
      control.title = `${tool.label} (${shortcut})`;
      control.onclick = () => setTool(tool.symbol);
      toolButtons.push(control);
      row.append(control);
    }
    groupElement.append(label, row);
    tools.append(groupElement);
  }
  const labels = select('editor-typed-label');
  for (const value of editor.TYPED_LABELS) labels.add(new Option(value, value));
  labels.value = typedLabel;
  labels.onchange = () => {
    typedLabel = choice('editor-typed-label', editor.TYPED_LABELS);
    setTool(typedKind() === 'goal' ? typedLabel.toLowerCase() : typedLabel);
  };
  typedBoxButton = button('editor-typed-box');
  typedGoalButton = button('editor-typed-goal');
  typedBoxButton.onclick = () => setTool(typedLabel);
  typedGoalButton.onclick = () => setTool(typedLabel.toLowerCase());
  syncTypedButtons();

  // The details and size fields: typing in them is the state.
  const difficulty = select('editor-difficulty');
  for (const value of DIFFICULTIES) difficulty.add(new Option(value.charAt(0).toUpperCase() + value.slice(1), value));
  const title = $<HTMLInputElement>('editor-title');
  title.addEventListener('input', () => {
    if (editorState !== undefined) editorAct({ ...editorState, title: title.value });
  });
  difficulty.onchange = () => {
    if (editorState !== undefined) editorAct({ ...editorState, difficulty: choice('editor-difficulty', DIFFICULTIES) });
  };
  const hint = $<HTMLTextAreaElement>('editor-hint');
  hint.addEventListener('input', () => {
    if (editorState !== undefined) editorAct({ ...editorState, hint: hint.value });
  });
  const width = $<HTMLInputElement>('editor-width');
  width.addEventListener('change', () => {
    if (editorState !== undefined) editorAct(editor.resize(editorState, Number(width.value), editorState.height));
    syncSizeInputs();
  });
  const height = $<HTMLInputElement>('editor-height');
  height.addEventListener('change', () => {
    if (editorState !== undefined) editorAct(editor.resize(editorState, editorState.width, Number(height.value)));
    syncSizeInputs();
  });

  // The grid: pointer painting, roving keyboard focus, keyboard activation.
  const grid = $('editor-grid');
  grid.addEventListener('pointerdown', event => {
    if (event.pointerType === 'mouse' && event.button !== 0) return;
    const cell = event.target instanceof Element ? event.target.closest('button.editor-cell') : null;
    if (!(cell instanceof HTMLButtonElement)) return;
    const row = Number(cell.dataset.row);
    const column = Number(cell.dataset.column);
    if (!Number.isInteger(row) || !Number.isInteger(column)) return;
    activePointer = {
      id: event.pointerId,
      pointerType: event.pointerType,
      startX: event.clientX,
      startY: event.clientY,
      row,
      column,
      moved: false,
    };
    // A touch may be a tap or the canvas's scroll; painting waits for proof it never panned.
    if (event.pointerType === 'touch') return;
    grid.setPointerCapture(event.pointerId);
    paintCell(row, column);
  });
  grid.addEventListener('pointermove', event => {
    const active = activePointer;
    if (active === undefined || active.id !== event.pointerId) return;
    if (active.pointerType === 'touch') {
      if (Math.hypot(event.clientX - active.startX, event.clientY - active.startY) > 8) active.moved = true;
      return;
    }
    paintAt(event.clientX, event.clientY);
  });
  const finishPainting = (event: PointerEvent, commitTouch: boolean) => {
    const active = activePointer;
    if (active === undefined || active.id !== event.pointerId) return;
    activePointer = undefined;
    if (commitTouch && active.pointerType === 'touch' && !active.moved) paintCell(active.row, active.column);
    if (grid.hasPointerCapture(event.pointerId)) grid.releasePointerCapture(event.pointerId);
  };
  grid.addEventListener('pointerup', event => finishPainting(event, true));
  grid.addEventListener('pointercancel', event => finishPainting(event, false));
  grid.addEventListener('lostpointercapture', event => {
    if (activePointer?.id === event.pointerId) activePointer = undefined;
  });
  grid.addEventListener('click', event => {
    // Pointer input painted already; a detail of 0 names keyboard activation.
    if (event.detail !== 0) return;
    const cell = event.target instanceof Element ? event.target.closest('button.editor-cell') : null;
    if (cell instanceof HTMLButtonElement) paintCell(Number(cell.dataset.row), Number(cell.dataset.column));
  });
  grid.addEventListener('keydown', event => {
    const cell = event.target instanceof Element ? event.target.closest('button.editor-cell') : null;
    if (!(cell instanceof HTMLButtonElement)) return;
    const row = Number(cell.dataset.row);
    const column = Number(cell.dataset.column);
    let nextRow = row;
    let nextColumn = column;
    switch (event.key) {
      case 'ArrowUp':
        nextRow -= 1;
        break;
      case 'ArrowDown':
        nextRow += 1;
        break;
      case 'ArrowLeft':
        nextColumn -= 1;
        break;
      case 'ArrowRight':
        nextColumn += 1;
        break;
      case 'Home':
        nextRow = event.ctrlKey ? 0 : row;
        nextColumn = 0;
        break;
      case 'End':
        nextRow = event.ctrlKey ? (editorState?.height ?? 1) - 1 : row;
        nextColumn = (editorState?.width ?? 1) - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    focusGridCell(nextRow, nextColumn);
  });
  grid.addEventListener('focusin', event => {
    const cell = event.target instanceof Element ? event.target.closest('button.editor-cell') : null;
    if (!(cell instanceof HTMLButtonElement)) return;
    focusCell = { row: Number(cell.dataset.row), column: Number(cell.dataset.column) };
    for (const row of cellButtons) for (const control of row) control.tabIndex = control === cell ? 0 : -1;
  });

  // The history, fill mode and the actions.
  button('editor-undo').onclick = () => editorUndo();
  button('editor-redo').onclick = () => editorRedo();
  button('editor-fill').onclick = () => {
    fillMode = !fillMode;
    const control = button('editor-fill');
    control.setAttribute('aria-pressed', String(fillMode));
    control.textContent = fillMode ? '◆ Fill on' : '◇ Fill off';
  };
  button('editor-test').onclick = () => {
    if (editorState !== undefined) startPlaytest(editor.puzzleOf(editorState));
  };
  button('editor-share').onclick = () => void shareEditor();
  button('editor-clear').onclick = () => openDialog('editor-clear-dialog');
  button('editor-clear-cancel').onclick = () => closeDialog('editor-clear-dialog');
  button('editor-clear-confirm').onclick = () => {
    closeDialog('editor-clear-dialog');
    if (editorState !== undefined) editorAct(editor.clearBoard(editorState));
  };
  button('editor-import-open').onclick = () => {
    $<HTMLTextAreaElement>('editor-import-text').value = '';
    $('editor-import-error').hidden = true;
    openDialog('editor-import');
  };
  button('editor-import-cancel').onclick = () => closeDialog('editor-import');
  button('editor-import-confirm').onclick = () => {
    const lines = $<HTMLTextAreaElement>('editor-import-text')
      .value.split('\n')
      .filter(line => line.length > 0);
    const error = (message: string) => {
      setText('editor-import-error', message);
      $('editor-import-error').hidden = false;
    };
    if (lines.length < 2) return error('Paste at least 2 rows of puzzle text.');
    const width = Math.max(...lines.map(line => [...line].length));
    if (width < editor.MIN_SIZE || lines.length < editor.MIN_SIZE)
      return error(`Puzzle must be at least ${editor.MIN_SIZE}x${editor.MIN_SIZE}.`);
    if (width > 20 || lines.length > 20) return error('Puzzle must be at most 20x20.');
    if (editorState === undefined) return;
    closeDialog('editor-import');
    editorAct(editor.loadPuzzle(editorState, { title: 'Imported puzzle', difficulty: 'beginner', rows: lines }));
    editorNotice('Puzzle rows imported. Paint the board, then choose Test puzzle.');
  };
  for (const [id, starter] of [
    ['editor-starter-single', STARTERS[0]!],
    ['editor-starter-turn', STARTERS[1]!],
  ] as const) {
    button(id).onclick = () => {
      if (editorState === undefined) return;
      editorAct(editor.loadPuzzle(editorState, starter));
      editorNotice(`${starter.title} loaded. Paint the board, then choose Test puzzle.`);
    };
  }

  // The drafts.
  select('editor-drafts').onchange = () => {
    if (drafts === undefined) return;
    const target = drafts.drafts.find(draft => draft.id === select('editor-drafts').value);
    if (target === undefined || target.id === drafts.activeId) return;
    commitDrafts(savedDrafts(), target.id, target.state);
    editorNotice('Local draft opened.');
  };
  button('editor-rename').onclick = () => {
    if (drafts === undefined) return;
    const name = $<HTMLInputElement>('editor-draft-name').value.trim();
    if (!name) return editorNotice('Give the draft a name.');
    const activeId = drafts.activeId;
    commitDrafts(
      savedDrafts().map(draft => (draft.id === activeId ? { ...draft, name } : draft)),
      activeId,
    );
    editorNotice('Draft renamed.');
  };
  button('editor-new').onclick = () => {
    if (drafts === undefined) return;
    if (drafts.drafts.length >= editor.MAX_DRAFTS) return editorNotice(`You can keep up to ${editor.MAX_DRAFTS} local drafts.`);
    const state = editor.initialState();
    const draft: editor.Draft = {
      id: editor.newDraftId(new Set(drafts.drafts.map(item => item.id))),
      name: 'New draft',
      updatedAt: new Date().toISOString(),
      state,
    };
    commitDrafts([...savedDrafts(), draft], draft.id, state);
    editorNotice('New local draft created.');
  };
  button('editor-duplicate').onclick = () => {
    if (drafts === undefined || editorState === undefined) return;
    if (drafts.drafts.length >= editor.MAX_DRAFTS) return editorNotice(`You can keep up to ${editor.MAX_DRAFTS} local drafts.`);
    const base = savedDrafts();
    const active = base.find(draft => draft.id === drafts!.activeId)!;
    const copy: editor.Draft = {
      ...active,
      id: editor.newDraftId(new Set(base.map(draft => draft.id))),
      name: `${active.name} copy`.slice(0, 60),
      updatedAt: new Date().toISOString(),
      state: editorState,
    };
    commitDrafts([...base, copy], copy.id, copy.state);
    editorNotice('Draft duplicated.');
  };
  button('editor-delete').onclick = () => {
    if (drafts === undefined || drafts.drafts.length <= 1) return;
    setText('editor-delete-message', `Delete "${activeDraft().name}" from this device? This cannot be undone.`);
    openDialog('editor-delete-dialog');
  };
  button('editor-delete-cancel').onclick = () => closeDialog('editor-delete-dialog');
  button('editor-delete-confirm').onclick = () => {
    if (drafts === undefined || drafts.drafts.length <= 1) return;
    const next = drafts.drafts.filter(draft => draft.id !== drafts!.activeId);
    commitDrafts(next, next[0]!.id, next[0]!.state);
    closeDialog('editor-delete-dialog');
    editorNotice('Draft deleted.');
  };

  // The shared preview's banner, and a quarantined draft's recovery.
  button('editor-shared-play').onclick = () => {
    if (shared !== undefined) startPlaytest(shared);
  };
  button('editor-shared-import').onclick = () => {
    if (shared === undefined || editorState === undefined || drafts === undefined) return;
    if (drafts.drafts.length >= editor.MAX_DRAFTS) return editorNotice(`You can keep up to ${editor.MAX_DRAFTS} local drafts.`);
    const state = editorState;
    const draft: editor.Draft = {
      id: editor.newDraftId(new Set(drafts.drafts.map(item => item.id))),
      name: shared.title.slice(0, 60),
      updatedAt: new Date().toISOString(),
      state,
    };
    // The preview never dirtied the draft it interrupted, so the store keeps it as it was.
    commitDrafts([...drafts.drafts, draft], draft.id, state);
    shared = undefined;
    navigate({ kind: 'editor' });
    editorNotice('Shared puzzle imported as a new local draft.');
  };
  button('editor-recovery-download').onclick = () => {
    const raw = storage.read(editor.RECOVERY_KEY);
    if (raw === null) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify(raw, null, 2)], { type: 'application/json' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'sokomind-invalid-editor-draft.json';
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  };
  button('editor-recovery-clear').onclick = () => {
    storage.remove(editor.RECOVERY_KEY);
    $('editor-recovery').hidden = true;
  };

  // The playtest's controls; its pad keeps its own ids, for the play page's
  // data-direction buttons belong to that page alone.
  button('editor-up').onclick = () => playtestMove(0);
  button('editor-down').onclick = () => playtestMove(1);
  button('editor-left').onclick = () => playtestMove(2);
  button('editor-right').onclick = () => playtestMove(3);
  button('editor-playtest-undo').onclick = () => {
    const active = playtest;
    if (active !== undefined && active.game.undo()) {
      active.state = decodeSnapshot(active.game.snapshot(), active.labels.length);
      void sound.playCue('undo');
      drawPlaytest();
    }
  };
  button('editor-playtest-reset').onclick = () => {
    const active = playtest;
    if (active === undefined) return;
    active.game.reset();
    active.state = decodeSnapshot(active.game.snapshot(), active.labels.length);
    void sound.playCue('reset');
    drawPlaytest();
  };
  button('editor-playtest-exit').onclick = () => {
    exitPlaytest();
    button('editor-test').focus();
  };
  bindInput($('editor-board'), {
    move: playtestMove,
    undo: () => button('editor-playtest-undo').click(),
    swipeable: () => editorBoard.fits,
    tap: playtestTap,
  });

  // The editor's own keys: 1-5 pick a tool, the chords work the history, and
  // Ctrl+Enter tests. They belong to the editing grid — a playtest or a
  // preview hands the keyboard back.
  document.addEventListener('keydown', event => {
    if (route().kind !== 'editor' || playtest !== undefined || shared !== undefined || document.querySelector('dialog[open]')) return;
    if (event.altKey) return;
    const editable = event.target instanceof Element && event.target.matches('input,textarea,select');
    if (event.ctrlKey || event.metaKey) {
      if (editable) return;
      const key = event.key.toLowerCase();
      if (key === 'z' && !event.shiftKey) {
        event.preventDefault();
        editorUndo();
      } else if (key === 'z' || key === 'y') {
        event.preventDefault();
        editorRedo();
      } else if (key === 'enter' && editorState !== undefined && !button('editor-test').disabled) {
        event.preventDefault();
        startPlaytest(editor.puzzleOf(editorState));
      }
      return;
    }
    if (editable) return;
    const tool = TOOL_KEYS[event.key];
    if (tool !== undefined) setTool(tool);
  });
}

export { bindEditor, editorAct, editorNotice, editorState, flushEditorSave, leaveEditor, renderEditor };
