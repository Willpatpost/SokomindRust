/** The puzzle editor's model: the grid its tools paint, the local drafts that
 * keep it between visits, and the share links that carry a puzzle to another
 * browser. Pure data and functions — main.ts renders them. The structural
 * checks here only shape the tools' feedback; whether the rows play stays the
 * engine's to say, so testing and sharing wait for the Rust parse of the rows. */
import { DIFFICULTIES } from './catalog.ts';

/** The labels a typed pair may carry: every uppercase letter the board's other
 * symbols leave free. A typed box only fits its matching lowercase goal. */
export const TYPED_LABELS = [
  'A',
  'B',
  'C',
  'D',
  'E',
  'F',
  'G',
  'H',
  'I',
  'J',
  'K',
  'L',
  'M',
  'N',
  'P',
  'Q',
  'T',
  'U',
  'V',
  'W',
  'Y',
  'Z',
] as const;
export type TypedLabel = (typeof TYPED_LABELS)[number];

/** The grid's size bounds; the largest matches the biggest board a share link
 * must be able to carry. */
export const MIN_SIZE = 3;
export const MAX_SIZE = 26;

const WALL = 'O';
const FLOOR = ' ';
const ROBOT = 'R';
const BOX = 'X';
const GOAL = 'S';
const LABELS = TYPED_LABELS as readonly string[];

/** The paint tools, in toolbar order; the keys 1-5 select them. */
export interface Tool {
  symbol: string;
  label: string;
  group: 'terrain' | 'pieces';
}
export const TOOLS: readonly Tool[] = [
  { symbol: WALL, label: 'Wall', group: 'terrain' },
  { symbol: FLOOR, label: 'Floor', group: 'terrain' },
  { symbol: ROBOT, label: 'Robot', group: 'pieces' },
  { symbol: BOX, label: 'Box', group: 'pieces' },
  { symbol: GOAL, label: 'Goal', group: 'pieces' },
];

/** Whether `symbol` names a typed box. */
export function isTypedBox(symbol: string): boolean {
  return LABELS.includes(symbol);
}

/** Whether `symbol` names a typed goal: the lowercase mate of a typed label. */
export function isTypedGoal(symbol: string): boolean {
  return symbol.length === 1 && symbol === symbol.toLowerCase() && LABELS.includes(symbol.toUpperCase());
}

/** Whether `symbol` is one the editor's grid may hold. */
export function isCell(symbol: string): boolean {
  return (
    symbol === FLOOR
    || symbol === WALL
    || symbol === ROBOT
    || symbol === BOX
    || symbol === GOAL
    || isTypedBox(symbol)
    || isTypedGoal(symbol)
  );
}

/** One editable puzzle: a rectangle of cells plus the details a shared link
 * carries. `tool` is the palette's live selection, never persisted with a draft. */
export interface EditorState {
  width: number;
  height: number;
  cells: string[][];
  title: string;
  difficulty: string;
  hint: string;
  tool: string;
}

/** A puzzle as loading one into the grid names it: a catalog room, a template,
 * an import or a shared link. */
export interface LoadablePuzzle {
  title: string;
  difficulty: string;
  hint?: string;
  rows: string[];
}

function emptyGrid(width: number, height: number): string[][] {
  return Array.from({ length: height }, () => Array<string>(width).fill(WALL));
}

function cloneCells(cells: readonly (readonly string[])[]): string[][] {
  return cells.map(row => [...row]);
}

/** A grid holds one robot, so painting one first clears any other. */
function removeRobot(cells: string[][]): void {
  for (const row of cells) for (let column = 0; column < row.length; column++) if (row[column] === ROBOT) row[column] = FLOOR;
}

/** A fresh 7x7 room of walls, titled and tagged as the difficulty every new
 * puzzle starts at. */
export function initialState(): EditorState {
  return {
    width: 7,
    height: 7,
    cells: emptyGrid(7, 7),
    title: 'Untitled',
    difficulty: 'beginner',
    hint: '',
    tool: WALL,
  };
}

/** Paints one cell with the selected tool; the state comes back unchanged when
 * the cell lies outside the grid or already holds the tool. */
export function paint(state: EditorState, row: number, column: number): EditorState {
  if (row < 0 || row >= state.height || column < 0 || column >= state.width) return state;
  if (state.cells[row]![column] === state.tool) return state;
  const cells = cloneCells(state.cells);
  if (state.tool === ROBOT) removeRobot(cells);
  cells[row]![column] = state.tool;
  return { ...state, cells };
}

/** Flood-fills the connected cells holding the target cell's symbol, as the
 * fill tool does; a robot tool still leaves only one robot. */
export function floodFill(state: EditorState, row: number, column: number): EditorState {
  if (row < 0 || row >= state.height || column < 0 || column >= state.width) return state;
  const target = state.cells[row]![column]!;
  if (target === state.tool) return state;
  const cells = cloneCells(state.cells);
  if (state.tool === ROBOT) removeRobot(cells);
  const stack: Array<[number, number]> = [[row, column]];
  const visited = new Set<string>();
  while (stack.length > 0) {
    const [r, c] = stack.pop()!;
    const key = `${r},${c}`;
    if (visited.has(key)) continue;
    visited.add(key);
    if (r < 0 || r >= state.height || c < 0 || c >= state.width) continue;
    if (cells[r]![c] !== target) continue;
    cells[r]![c] = state.tool;
    stack.push([r - 1, c], [r + 1, c], [r, c - 1], [r, c + 1]);
  }
  return { ...state, cells };
}

/** Resizes the grid, clamped to the size bounds; the overlap keeps its cells
 * and the new territory starts as walls. */
export function resize(state: EditorState, width: number, height: number): EditorState {
  const boundedWidth = Math.max(MIN_SIZE, Math.min(MAX_SIZE, width));
  const boundedHeight = Math.max(MIN_SIZE, Math.min(MAX_SIZE, height));
  if (boundedWidth === state.width && boundedHeight === state.height) return state;
  const cells = emptyGrid(boundedWidth, boundedHeight);
  for (let row = 0; row < Math.min(boundedHeight, state.height); row++)
    for (let column = 0; column < Math.min(boundedWidth, state.width); column++) cells[row]![column] = state.cells[row]![column]!;
  return { ...state, width: boundedWidth, height: boundedHeight, cells };
}

/** Erases every cell to walls, keeping the grid's size and the puzzle's details. */
export function clearBoard(state: EditorState): EditorState {
  return { ...state, cells: emptyGrid(state.width, state.height) };
}

/** Loads a whole puzzle into the grid, padding ragged rows with walls so the
 * missing cells cannot hide a path. */
export function loadPuzzle(state: EditorState, puzzle: LoadablePuzzle): EditorState {
  const height = puzzle.rows.length;
  if (height === 0) return state;
  const width = Math.max(...puzzle.rows.map(row => [...row].length));
  const cells = puzzle.rows.map(row => {
    const symbols = [...row];
    return Array.from({ length: width }, (_, column) => (column < symbols.length ? symbols[column]! : WALL));
  });
  return { ...state, width, height, cells, title: puzzle.title, difficulty: puzzle.difficulty, hint: puzzle.hint ?? '' };
}

/** The state's rows, one string per grid row. */
export function rowsOf(state: EditorState): string[] {
  return state.cells.map(row => row.join(''));
}

/** Whether every cell is wall — a draft never painted, which the starter
 * templates address. */
export function isBlank(state: EditorState): boolean {
  return state.cells.every(row => row.every(cell => cell === WALL));
}

/** The boxes in rows: generic plus typed. */
export function countBoxes(rows: readonly string[]): number {
  let boxes = 0;
  for (const row of rows) for (const symbol of row) if (symbol === BOX || isTypedBox(symbol)) boxes++;
  return boxes;
}

/** The puzzle the state names, for playtesting, sharing and importing. */
export function puzzleOf(state: EditorState): LoadablePuzzle & { boxes: number } {
  const rows = rowsOf(state);
  return { title: state.title, difficulty: state.difficulty, hint: state.hint || undefined, rows, boxes: countBoxes(rows) };
}

/** What the grid holds, goals keyed by the box label they match. */
interface Census {
  robots: number;
  boxes: Map<string, number>;
  goals: Map<string, number>;
}

function census(symbols: readonly string[]): Census {
  const boxes = new Map<string, number>();
  const goals = new Map<string, number>();
  const add = (counts: Map<string, number>, label: string) => counts.set(label, (counts.get(label) ?? 0) + 1);
  let robots = 0;
  for (const symbol of symbols) {
    if (symbol === ROBOT) robots++;
    else if (symbol === BOX || isTypedBox(symbol)) add(boxes, symbol);
    else if (symbol === GOAL) add(goals, BOX);
    else if (isTypedGoal(symbol)) add(goals, symbol.toUpperCase());
  }
  return { robots, boxes, goals };
}

/** The tools' feedback on the grid: what still stands between it and a puzzle
 * the engine could parse. These are the painter's checks; the engine's own
 * verdict arrives with the Test button. */
export interface Validation {
  valid: boolean;
  errors: string[];
}

export function validate(state: EditorState): Validation {
  const errors: string[] = [];
  const { robots, boxes, goals } = census(state.cells.flat());
  if (robots === 0) errors.push('Place a robot (R) on the board.');
  if (robots > 1) errors.push('Only one robot allowed.');
  const total = (counts: Map<string, number>) => [...counts.values()].reduce((sum, count) => sum + count, 0);
  if (total(boxes) === 0) errors.push('Place at least one box.');
  if (total(goals) === 0) errors.push('Place at least one goal.');
  for (const [label, count] of boxes) {
    const matching = goals.get(label) ?? 0;
    if (count !== matching)
      errors.push(
        label === BOX
          ? `Generic box/goal mismatch: ${count} boxes, ${matching} goals.`
          : `Label ${label}: ${count} boxes but ${matching} goals.`,
      );
  }
  for (const [label, count] of goals) if (!boxes.has(label)) errors.push(`Label ${label}: ${count} goals but no boxes.`);
  if (!state.title.trim()) errors.push('Give your puzzle a title.');
  return { valid: errors.length === 0, errors };
}

/** Whether rows are a shape the engine could parse: one robot, every box a
 * matching goal, at least one box. The share decoder rejects anything else. */
function rowsPlayable(rows: readonly string[]): boolean {
  const { robots, boxes, goals } = census(rows.flatMap(row => [...row]));
  if (robots !== 1) return false;
  const total = [...boxes.values()].reduce((sum, count) => sum + count, 0);
  if (total === 0) return false;
  for (const label of new Set([...boxes.keys(), ...goals.keys()])) if ((boxes.get(label) ?? 0) !== (goals.get(label) ?? 0)) return false;
  return true;
}

/** One saved editor puzzle, with the name the drafts list shows. */
export interface Draft {
  id: string;
  name: string;
  /** When the draft last autosaved, ISO, or null while never saved. */
  updatedAt: string | null;
  state: EditorState;
}

/** The drafts this browser keeps, and which one is open. */
export interface DraftStore {
  activeId: string;
  drafts: Draft[];
}

/** The most drafts one browser keeps. */
export const MAX_DRAFTS = 25;
/** The localStorage names the store — and a quarantined malformed payload — live under. */
export const DRAFTS_KEY = 'editor-drafts';
export const RECOVERY_KEY = 'editor-drafts-recovery';

/** The state as a draft stores it: everything but the live tool. */
function stateValue(state: EditorState): Record<string, unknown> {
  return {
    width: state.width,
    height: state.height,
    cells: state.cells,
    title: state.title,
    difficulty: state.difficulty,
    hint: state.hint,
  };
}

function parseStateValue(value: unknown): EditorState | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (
    !Number.isSafeInteger(record.width)
    || Number(record.width) < MIN_SIZE
    || Number(record.width) > MAX_SIZE
    || !Number.isSafeInteger(record.height)
    || Number(record.height) < MIN_SIZE
    || Number(record.height) > MAX_SIZE
    || !Array.isArray(record.cells)
    || record.cells.length !== Number(record.height)
    || !record.cells.every(
      row =>
        Array.isArray(row)
        && row.length === Number(record.width)
        && row.every((cell): cell is string => typeof cell === 'string' && isCell(cell)),
    )
    || typeof record.title !== 'string'
    || record.title.length > 60
    || typeof record.difficulty !== 'string'
    || !DIFFICULTIES.includes(record.difficulty)
    || (record.hint !== undefined && (typeof record.hint !== 'string' || record.hint.length > 200))
  )
    return null;
  return {
    width: Number(record.width),
    height: Number(record.height),
    cells: record.cells as string[][],
    title: record.title,
    difficulty: record.difficulty,
    hint: typeof record.hint === 'string' ? record.hint : '',
    tool: WALL,
  };
}

function isIsoTimestamp(value: unknown): value is string {
  if (typeof value !== 'string') return false;
  const milliseconds = Date.parse(value);
  return Number.isFinite(milliseconds) && new Date(milliseconds).toISOString() === value;
}

function parseDraft(value: unknown): Draft | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const state = parseStateValue(record.state);
  if (
    typeof record.id !== 'string'
    || record.id.length === 0
    || record.id.length > 100
    || typeof record.name !== 'string'
    || record.name.trim().length === 0
    || record.name.length > 60
    || (record.updatedAt !== null && !isIsoTimestamp(record.updatedAt))
    || !state
  )
    return null;
  return { id: record.id, name: record.name.trim(), updatedAt: record.updatedAt, state };
}

/** The drafts stored under DRAFTS_KEY, or null when nothing valid is there. A
 * malformed payload answers null so the page can quarantine it before replacing
 * it — the painter never loses a draft to a bad parse. */
export function parseDraftStore(value: unknown): DraftStore | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (
    typeof record.activeId !== 'string'
    || !Array.isArray(record.drafts)
    || record.drafts.length === 0
    || record.drafts.length > MAX_DRAFTS
  )
    return null;
  const drafts: Draft[] = [];
  for (const entry of record.drafts) {
    const draft = parseDraft(entry);
    if (!draft) return null;
    drafts.push(draft);
  }
  const ids = new Set(drafts.map(draft => draft.id));
  if (ids.size !== drafts.length || !ids.has(record.activeId)) return null;
  return { activeId: record.activeId, drafts };
}

/** A first visit's store: one never-saved draft. */
export function createDraftStore(): DraftStore {
  return { activeId: 'draft-1', drafts: [{ id: 'draft-1', name: 'Untitled', updatedAt: null, state: initialState() }] };
}

/** A fresh draft id that collides with none of `existing`. */
export function newDraftId(existing: ReadonlySet<string>): string {
  for (;;) {
    const id = `draft-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
    if (!existing.has(id)) return id;
  }
}

/** A puzzle as a share link carries it. */
export interface SharedPuzzle extends LoadablePuzzle {
  boxes: number;
}

/** The longest encoded payload a link may carry; longer ones are refused, not truncated. */
const MAX_ENCODED = 4096;

function encodeBase64Url(value: string): string {
  let binary = '';
  for (const byte of new TextEncoder().encode(value)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function decodeBase64Url(value: string): string {
  // Spaces repair links produced by an older standard-Base64 encoder: reading
  // the URL turned their '+' characters into spaces.
  const standard = value.trim().replace(/ /g, '+').replace(/-/g, '+').replace(/_/g, '/');
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(standard) || standard.length % 4 === 1) throw new Error('Invalid Base64 data');
  const padded = standard.padEnd(standard.length + ((4 - (standard.length % 4)) % 4), '=');
  const binary = atob(padded);
  const bytes = Uint8Array.from(binary, character => character.charCodeAt(0));
  return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
}

/** Encodes the puzzle as the payload a `#/editor?custom=` link carries. */
export function encodeShare(puzzle: LoadablePuzzle & { boxes: number }): string {
  const compact: Record<string, unknown> = { t: puzzle.title, d: puzzle.difficulty, r: [...puzzle.rows] };
  if (puzzle.hint !== undefined) compact.h = puzzle.hint;
  return encodeBase64Url(JSON.stringify(compact));
}

/** The puzzle an encoded payload carries, or null when the payload is not one:
 * bad Base64, bad JSON, unknown symbols, sizes outside the bounds, a difficulty
 * the catalog lacks, or rows no engine could parse. */
export function decodeShare(encoded: string): SharedPuzzle | null {
  if (!encoded || encoded.length > MAX_ENCODED) return null;
  let compact: unknown;
  try {
    compact = JSON.parse(decodeBase64Url(encoded));
  } catch {
    return null;
  }
  if (!compact || typeof compact !== 'object' || Array.isArray(compact)) return null;
  const record = compact as Record<string, unknown>;
  if (typeof record.t !== 'string' || !record.t.trim() || record.t.length > 60) return null;
  if (typeof record.d !== 'string' || !DIFFICULTIES.includes(record.d)) return null;
  if (record.h !== undefined && (typeof record.h !== 'string' || record.h.length > 200)) return null;
  if (!Array.isArray(record.r) || record.r.length < MIN_SIZE || record.r.length > MAX_SIZE) return null;
  let width: number | undefined;
  for (const row of record.r) {
    if (typeof row !== 'string') return null;
    const symbols = [...row];
    if (symbols.length < MIN_SIZE || symbols.length > MAX_SIZE || symbols.some(symbol => !isCell(symbol))) return null;
    if (width === undefined) width = symbols.length;
    if (symbols.length !== width) return null;
  }
  const rows = record.r as string[];
  if (!rowsPlayable(rows)) return null;
  return {
    title: record.t,
    difficulty: record.d,
    hint: typeof record.h === 'string' ? record.h : undefined,
    rows,
    boxes: countBoxes(rows),
  };
}
