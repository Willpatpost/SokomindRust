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
function read(key: string): unknown {
  try {
    return JSON.parse(localStorage.getItem(prefix + key) || 'null');
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
// getRandomValues works over plain HTTP; randomUUID needs a secure context.
function randomId(): string {
  return [...crypto.getRandomValues(new Uint8Array(16))].map(b => b.toString(16).padStart(2, '0')).join('');
}
/** This browser's progress profile id, created at random on first use; null when storage is
 * unavailable. Its 32 hex digits are the length crates/server/src/progress.rs requires (checked by
 * scripts/mirrors.test.mjs). */
export function profile(): string | null {
  const value = read('profile');
  if (typeof value === 'string' && /^[a-f0-9]{32}$/.test(value)) return value;
  const id = randomId();
  return write('profile', id) ? id : null;
}
