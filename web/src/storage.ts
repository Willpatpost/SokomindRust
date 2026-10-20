/** A best route for one puzzle, stored with the rows it solves and its counts. */
export interface Best {
  rows: string;
  route: string;
  moves: number;
  pushes: number;
}
/** The puzzle last played and the moves made on it, replayed on the next visit. */
export interface SavedSession {
  id: string;
  rows: string;
  actions: string;
}
const prefix = 'sokomind-rust.v1.';
/** The full localStorage key `name` is stored under, as a storage event names it. */
export function key(name: string): string {
  return prefix + name;
}
/** The JSON value stored under `name`, or null when storage fails or holds no JSON. */
export function read(name: string): unknown {
  try {
    return JSON.parse(localStorage.getItem(prefix + name) || 'null');
  } catch {
    return null;
  }
}
/** Stores `value` as JSON under the app's key prefix; false when storage is unavailable or full. */
export function write(key: string, value: unknown): boolean {
  try {
    localStorage.setItem(prefix + key, JSON.stringify(value));
    return true;
  } catch {
    return false;
  }
}
/** Every name the app stores something under, for the resets that clear a
 * family of values without knowing its ids. */
export function names(): string[] {
  try {
    return Object.keys(localStorage)
      .filter(key => key.startsWith(prefix))
      .map(key => key.slice(prefix.length));
  } catch {
    return [];
  }
}
/** Removes the value stored under `name`; a blocked store keeps it. */
export function remove(name: string): void {
  try {
    localStorage.removeItem(prefix + name);
  } catch {
    /* A blocked store keeps the value; the reset still clears what it can. */
  }
}
/** The saved session, or null when none is stored or it is malformed. */
export function session(): SavedSession | null {
  const v = read('session') as Partial<SavedSession> | null;
  return v && typeof v.id === 'string' && typeof v.rows === 'string' && typeof v.actions === 'string' ? (v as SavedSession) : null;
}
/** The stored best for puzzle `id`, or null unless it was stored for exactly these `rows`. */
export function best(id: string, rows: string): Best | null {
  const v = read('best.' + id) as Partial<Best> | null;
  return v && v.rows === rows && typeof v.route === 'string' && Number.isInteger(v.moves) && Number.isInteger(v.pushes)
    ? (v as Best)
    : null;
}
