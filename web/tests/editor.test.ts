import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DIFFICULTIES } from '../src/catalog.ts';
import * as editor from '../src/editor.ts';

/** A small solvable room: the robot above the box, the goal below it. */
const ROOM = ['OOOOO', 'O R O', 'O X O', 'O S O', 'OOOOO'];

/** A state built from rows, the way loading one always is. */
function stateOf(rows: string[], title = 'Test room'): editor.EditorState {
  return editor.loadPuzzle(editor.initialState(), { title, difficulty: 'beginner', rows });
}

test('a fresh editor is a blank room of walls at the default tier', () => {
  const state = editor.initialState();
  assert.equal(state.width, 7);
  assert.equal(state.height, 7);
  assert.equal(state.cells.length, 7);
  assert.ok(state.cells.every(row => row.length === 7 && row.every(cell => cell === 'O')));
  assert.ok(editor.isBlank(state));
  assert.equal(state.title, 'Untitled');
  // The default tier must be one the catalog carries; the select reads the same list.
  assert.ok(DIFFICULTIES.includes(state.difficulty));
});

test('painting a robot moves it, leaving floor behind', () => {
  let state = { ...stateOf(ROOM), tool: 'R' };
  state = editor.paint(state, 1, 1);
  assert.equal(state.cells[1]![1], 'R');
  state = editor.paint(state, 3, 3);
  assert.equal(state.cells[3]![3], 'R');
  assert.equal(state.cells[1]![1], ' ');
});

test('painting what is already there, or outside the grid, changes nothing', () => {
  const state = { ...stateOf(ROOM), tool: 'X' };
  assert.ok(editor.paint(state, 2, 2) === state); // the box is already there
  assert.ok(editor.paint(state, -1, 0) === state);
  assert.ok(editor.paint(state, 0, 5) === state);
  assert.ok(editor.paint(state, 5, 0) === state);
});

test('the fill tool floods the connected cells holding the target symbol', () => {
  let state = stateOf(['OOOOO', 'O   O', 'O O O', 'O   O', 'OOOOO']);
  state = { ...state, tool: ' ' };
  state = editor.floodFill(state, 1, 1);
  // The floor around the center wall floods; the wall itself holds.
  assert.equal(state.cells[1]![1], ' ');
  assert.equal(state.cells[3]![3], ' ');
  assert.equal(state.cells[2]![2], 'O');
  state = editor.floodFill(state, 2, 2);
  assert.equal(state.cells[2]![2], ' ');
  // The border walls never flood with it.
  assert.equal(state.cells[0]![0], 'O');
});

test('resizing clamps to the bounds and keeps the overlap', () => {
  let state = { ...stateOf(ROOM), tool: 'X' };
  state = editor.paint(state, 1, 1);
  const grown = editor.resize(state, 99, 2);
  assert.equal(grown.width, editor.MAX_SIZE);
  assert.equal(grown.height, editor.MIN_SIZE);
  assert.equal(grown.cells[1]![1], 'X'); // the painted cell survived
  assert.equal(grown.cells[0]![6], 'O'); // the new territory is walls
  const shrunk = editor.resize(grown, 3, 3);
  assert.equal(shrunk.width, 3);
  assert.equal(shrunk.height, 3);
  assert.equal(shrunk.cells[1]![1], 'X');
});

test('clearing erases every cell but keeps the size and details', () => {
  const state = editor.clearBoard(stateOf(ROOM));
  assert.equal(state.width, 5);
  assert.equal(state.height, 5);
  assert.ok(editor.isBlank(state));
  assert.equal(state.title, 'Test room');
});

test('loading pads ragged rows with walls', () => {
  const state = stateOf(['OOOO', 'O RXS']);
  // The longer row names the width; the shorter one pads with walls.
  assert.equal(state.width, 5);
  assert.equal(state.height, 2);
  assert.equal(state.cells[0]![4], 'O');
  assert.equal(state.cells[1]![4], 'S');
  assert.equal(state.title, 'Test room');
  // An empty rows list loads nothing.
  const room = stateOf(ROOM);
  assert.ok(editor.loadPuzzle(room, { title: 'Empty', difficulty: 'beginner', rows: [] }) === room);
});

test('the puzzle a state names carries its rows, boxes and hint', () => {
  const puzzle = editor.puzzleOf(stateOf(ROOM, 'Push room'));
  assert.deepEqual(puzzle.rows, ROOM);
  assert.equal(puzzle.boxes, 1);
  assert.equal(puzzle.hint, undefined);
  const hinted = editor.puzzleOf({ ...stateOf(ROOM), hint: 'One push.' });
  assert.equal(hinted.hint, 'One push.');
  // Typed boxes count as boxes too.
  assert.equal(editor.countBoxes(['OAXO', 'OaSO']), 2);
});

test('the painter checks name what stands between the grid and a puzzle', () => {
  const blank = editor.validate(editor.initialState());
  assert.ok(blank.errors.includes('Place a robot (R) on the board.'));
  assert.ok(blank.errors.includes('Place at least one box.'));
  assert.ok(blank.errors.includes('Place at least one goal.'));
  const twoRobots = editor.validate(stateOf(['OOOOO', 'O R O', 'O R O', 'O S O', 'OOOOO']));
  assert.ok(twoRobots.errors.includes('Only one robot allowed.'));
  const generic = editor.validate(stateOf(['OOOOOOO', 'O R   O', 'O XX  O', 'O S   O', 'OOOOOOO']));
  assert.ok(generic.errors.includes('Generic box/goal mismatch: 2 boxes, 1 goals.'));
  const typed = editor.validate(stateOf(['OOOOOOO', 'O R   O', 'O AA  O', 'O a   O', 'OOOOOOO']));
  assert.ok(typed.errors.includes('Label A: 2 boxes but 1 goals.'));
  const stray = editor.validate(stateOf(['OOOOO', 'O R O', 'O a O', 'OOOOO']));
  assert.ok(stray.errors.includes('Label A: 1 goals but no boxes.'));
  const untitled = editor.validate(stateOf(ROOM, '  '));
  assert.ok(untitled.errors.includes('Give your puzzle a title.'));
  const ready = editor.validate(stateOf(ROOM));
  assert.equal(ready.valid, true);
  assert.deepEqual(ready.errors, []);
});

test('the typed guards know their letters', () => {
  assert.ok(editor.isTypedBox('A'));
  assert.ok(editor.isTypedBox('Z'));
  assert.ok(!editor.isTypedBox('O'));
  assert.ok(!editor.isTypedBox('R'));
  assert.ok(!editor.isTypedBox('S'));
  assert.ok(!editor.isTypedBox('X'));
  assert.ok(!editor.isTypedBox('a'));
  assert.ok(editor.isTypedGoal('a'));
  assert.ok(!editor.isTypedGoal('A'));
  assert.ok(!editor.isTypedGoal('x'));
  assert.ok(!editor.isTypedGoal('ab'));
  for (const symbol of [' ', 'O', 'R', 'X', 'S', 'A', 'a']) assert.ok(editor.isCell(symbol));
  for (const symbol of ['@', 'x', 'AB', '']) assert.ok(!editor.isCell(symbol));
});

/** The state as a draft stores it: everything but the live tool. */
function statePayload(state: editor.EditorState): Record<string, unknown> {
  return {
    width: state.width,
    height: state.height,
    cells: state.cells,
    title: state.title,
    difficulty: state.difficulty,
    hint: state.hint,
  };
}

function draftPayload(id: string, state: editor.EditorState, overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, name: 'Room', updatedAt: null, state: statePayload(state), ...overrides };
}

function storePayload(drafts: unknown[], activeId = 'draft-1'): Record<string, unknown> {
  return { activeId, drafts };
}

test('the draft store round-trips through storage', () => {
  const saved = new Date().toISOString();
  const store = storePayload([draftPayload('draft-1', stateOf(ROOM), { name: 'Push room', updatedAt: saved })]);
  const parsed = editor.parseDraftStore(JSON.parse(JSON.stringify(store)));
  if (parsed === null) assert.fail('the stored draft store parsed back');
  assert.equal(parsed.activeId, 'draft-1');
  assert.equal(parsed.drafts.length, 1);
  assert.equal(parsed.drafts[0]!.name, 'Push room');
  assert.equal(parsed.drafts[0]!.updatedAt, saved);
  assert.deepEqual(
    parsed.drafts[0]!.state.cells,
    ROOM.map(row => [...row]),
  );
  // The tool never rides with a draft; a loaded one starts on walls.
  assert.equal(parsed.drafts[0]!.state.tool, 'O');
});

test('a malformed payload never loads, so the page can quarantine it', () => {
  assert.equal(editor.parseDraftStore(null), null);
  assert.equal(editor.parseDraftStore('a string'), null);
  assert.equal(editor.parseDraftStore([]), null);
  assert.equal(editor.parseDraftStore({}), null);
  assert.equal(editor.parseDraftStore(storePayload([])), null);
  const room = stateOf(ROOM);
  const bad = (store: Record<string, unknown>) => editor.parseDraftStore(JSON.parse(JSON.stringify(store)));
  assert.equal(bad(storePayload([draftPayload('draft-1', room, { updatedAt: 'yesterday' })])), null);
  assert.equal(bad(storePayload([draftPayload('draft-1', room, { name: '   ' })])), null);
  assert.equal(bad(storePayload([draftPayload('', room)])), null);
  assert.equal(bad(storePayload([draftPayload('draft-1', room, { state: { ...statePayload(room), title: 'a'.repeat(61) } })])), null);
  assert.equal(bad(storePayload([draftPayload('draft-1', room, { state: { ...statePayload(room), hint: 'a'.repeat(201) } })])), null);
  assert.equal(bad(storePayload([draftPayload('draft-1', room, { state: { ...statePayload(room), difficulty: 'custom' } })])), null);
  assert.equal(bad(storePayload([draftPayload('draft-1', room, { state: { ...statePayload(room), cells: [['@']] } })])), null);
  assert.equal(
    bad(storePayload([draftPayload('draft-1', room, { state: { ...statePayload(room), cells: ROOM.map(row => [...row, 'O']) } })])),
    null,
  );
  assert.equal(bad(storePayload([draftPayload('draft-1', room), draftPayload('draft-1', room)])), null);
  assert.equal(bad(storePayload([draftPayload('draft-1', room)], 'draft-9')), null);
  assert.equal(bad(storePayload(Array.from({ length: editor.MAX_DRAFTS + 1 }, (_, index) => draftPayload(`draft-${index}`, room)))), null);
});

test('a first visit keeps one never-saved draft', () => {
  const store = editor.createDraftStore();
  assert.equal(store.drafts.length, 1);
  assert.equal(store.drafts[0]!.updatedAt, null);
  assert.ok(editor.isBlank(store.drafts[0]!.state));
  assert.ok(editor.parseDraftStore(JSON.parse(JSON.stringify(store))) !== null);
});

test('new draft ids never collide with the drafts that exist', () => {
  const existing = new Set(['draft-1']);
  const id = editor.newDraftId(existing);
  assert.ok(id !== 'draft-1');
  assert.match(id, /^draft-/);
  assert.ok(!existing.has(editor.newDraftId(existing)));
});

/** A payload as the app would encode any JSON, for links built to be rejected. */
function payload(value: unknown): string {
  let binary = '';
  for (const byte of new TextEncoder().encode(JSON.stringify(value))) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

const SHAREABLE = { title: 'Push room', difficulty: 'beginner', rows: ROOM, boxes: 1 };

test('a share link carries the puzzle and comes back', () => {
  const puzzle = { ...SHAREABLE, title: 'Café corner 📦', difficulty: 'expert', hint: 'Mind the corner.' };
  const encoded = editor.encodeShare(puzzle);
  assert.ok(encoded.length > 0);
  assert.ok(!/[+/=]/.test(encoded));
  const back = editor.decodeShare(encoded);
  if (back === null) assert.fail('the share link decoded');
  assert.equal(back.title, 'Café corner 📦');
  assert.equal(back.difficulty, 'expert');
  assert.equal(back.hint, 'Mind the corner.');
  assert.deepEqual(back.rows, ROOM);
  assert.equal(back.boxes, 1);
});

test('a link without a hint comes back without one', () => {
  const back = editor.decodeShare(editor.encodeShare(SHAREABLE));
  if (back === null) assert.fail('the share link decoded');
  assert.equal(back.hint, undefined);
  assert.equal(back.title, 'Push room');
});

test('a link an older standard-Base64 encoder made still reads', () => {
  // Reading such a link through its URL turned every + into a space, which the
  // decoder repairs before mapping the URL-safe alphabet back.
  const legacy = editor.encodeShare(SHAREABLE).replace(/-/g, '+').replace(/_/g, '/').replace(/\+/g, ' ');
  const back = editor.decodeShare(legacy);
  if (back === null) assert.fail('the legacy link decoded');
  assert.deepEqual(back.rows, ROOM);
});

test('a payload that is not a puzzle never decodes', () => {
  assert.equal(editor.decodeShare(''), null);
  assert.equal(editor.decodeShare('abc!def'), null);
  assert.equal(editor.decodeShare('a'.repeat(4097)), null);
  const reject = (value: unknown) => editor.decodeShare(payload(value));
  assert.equal(reject({ t: 'No rows', d: 'beginner', r: [] }), null);
  assert.equal(reject({ t: 'Too small', d: 'beginner', r: ['OO', 'OO'] }), null);
  assert.equal(reject({ t: 'Ragged', d: 'beginner', r: ['OOOO', 'OO'] }), null);
  assert.equal(reject({ t: 'Bad tier', d: 'custom', r: ROOM }), null);
  assert.equal(reject({ t: 'Bad symbol', d: 'beginner', r: ['OOOOO', 'O R O', 'O @ O', 'O S O', 'OOOOO'] }), null);
  assert.equal(reject({ t: 'Lowercase x', d: 'beginner', r: ['OOOOO', 'O R O', 'O x O', 'O S O', 'OOOOO'] }), null);
  assert.equal(reject({ t: 'Two robots', d: 'beginner', r: ['OOOOO', 'O R O', 'O R O', 'O S O', 'OOOOO'] }), null);
  assert.equal(reject({ t: 'No goal', d: 'beginner', r: ['OOOOO', 'O R O', 'O X O', 'O   O', 'OOOOO'] }), null);
  assert.equal(reject({ t: 'No box', d: 'beginner', r: ['OOOOO', 'O R O', 'O S O', 'O   O', 'OOOOO'] }), null);
  assert.equal(reject({ t: 'Typed mismatch', d: 'beginner', r: ['OOOOO', 'O R O', 'O A O', 'O   O', 'OOOOO'] }), null);
  assert.equal(reject({ t: '   ', d: 'beginner', r: ROOM }), null);
  assert.equal(reject({ t: 'a'.repeat(61), d: 'beginner', r: ROOM }), null);
  assert.equal(reject({ t: 'Long hint', d: 'beginner', h: 'a'.repeat(201), r: ROOM }), null);
  // A typed pair in balance is a puzzle the link may carry.
  const typed = reject({ t: 'Typed pair', d: 'expert', r: ['OOOOO', 'O R O', 'O A O', 'O a O', 'OOOOO'] });
  if (typed === null) assert.fail('the typed pair decoded');
  assert.equal(typed.boxes, 1);
});
