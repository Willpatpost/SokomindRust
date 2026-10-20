import * as storage from './storage.ts';
import raw from '../../data/puzzles.json' with { type: 'json' };

/** One catalog puzzle with the dimensions its rows name. */
export interface Entry {
  id: string;
  title: string;
  difficulty: string;
  collection: string;
  boxes: number;
  rows: string[];
  hint?: string;
  width: number;
  height: number;
}

/** The catalog in curriculum order, as data/puzzles.json lists it. The
 * dimensions come from the rows themselves, so the selector never re-parses
 * a board to know its shape. */
export const ENTRIES: Entry[] = (raw as Omit<Entry, 'width' | 'height'>[]).map(puzzle => ({
  ...puzzle,
  width: Math.max(...puzzle.rows.map(row => row.length)),
  height: puzzle.rows.length,
}));

/** The difficulty tiers in curriculum order. */
export const DIFFICULTIES: string[] = [...new Set(ENTRIES.map(entry => entry.difficulty))];

/** Whether `entry` has a stored best saved for exactly its rows: the page's
 * solved signal, as the next-unsolved walk reads it too. */
export function solved(entry: Entry): boolean {
  return storage.best(entry.id, entry.rows.join('\n')) !== null;
}

/** The favorite puzzle ids. */
export function favorites(): Set<string> {
  const value = storage.read('favorites');
  return new Set(Array.isArray(value) ? value.filter((id): id is string => typeof id === 'string') : []);
}

/** Adds or removes `id` from the favorites, and returns whether it is one
 * now. A full or blocked store leaves the set as it was. */
export function toggleFavorite(id: string): boolean {
  const current = favorites();
  const favorite = !current.has(id);
  const next = favorite ? [...current, id] : [...current].filter(item => item !== id);
  if (!storage.write('favorites', next)) return current.has(id);
  return favorite;
}

/** What the puzzle list keeps; every field optional. */
export interface Filter {
  difficulty?: string;
  collection?: string;
  search?: string;
  boxes?: number;
  status?: 'all' | 'solved' | 'open' | 'favorites';
}

/** The entries a filter keeps, in catalog order. */
export function filter(entries: Entry[], by: Filter): Entry[] {
  const needle = by.search?.trim().toLowerCase();
  return entries.filter(
    entry =>
      (by.difficulty === undefined || entry.difficulty === by.difficulty)
      && (by.collection === undefined || entry.collection === by.collection)
      && (by.boxes === undefined || entry.boxes === by.boxes)
      && (needle === undefined || entry.title.toLowerCase().includes(needle) || entry.id.includes(needle))
      && (by.status === undefined
        || by.status === 'all'
        || (by.status === 'solved' && solved(entry))
        || (by.status === 'open' && !solved(entry))
        || (by.status === 'favorites' && favorites().has(entry.id))),
  );
}

/** How many rows one page of the list shows, as the original's selector does. */
export const PER_PAGE = 50;

/** One page of a list, 1-based: a page past the end is the last page, and an
 * empty list still has one. */
export function paginate(list: Entry[], page: number): { page: number; pages: number; slice: Entry[] } {
  const pages = Math.max(1, Math.ceil(list.length / PER_PAGE));
  const at = Math.min(Math.max(1, page), pages);
  return { page: at, pages, slice: list.slice((at - 1) * PER_PAGE, at * PER_PAGE) };
}

/** A tiny board thumbnail, one cell per div — the real rows, not a stand-in.
 * Ragged rows pad with walls, as the game's own board does. */
export function minimap(entry: Entry): HTMLElement {
  const map = document.createElement('div');
  map.className = 'minimap';
  map.style.setProperty('--columns', String(entry.width));
  for (const row of entry.rows) {
    for (const symbol of row.padEnd(entry.width, 'O')) {
      const cell = document.createElement('div');
      const kind =
        symbol === 'O' ? 'wall' : symbol === ' ' ? 'floor' : symbol === 'R' ? 'robot' : symbol === symbol.toLowerCase() ? 'goal' : 'box';
      cell.className = `mini ${kind}`;
      map.append(cell);
    }
  }
  return map;
}
