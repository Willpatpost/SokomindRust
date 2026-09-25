export interface Best { rows: string; route: string; moves: number; pushes: number }
export interface SavedSession { id: string; rows: string; actions: string }
const prefix = 'sokomind-rust.v1.';
function read(key: string): unknown {
  try { return JSON.parse(localStorage.getItem(prefix + key) || 'null'); } catch { return null; }
}
export function write(key: string, value: unknown): boolean {
  try { localStorage.setItem(prefix + key, JSON.stringify(value)); return true; } catch { return false; }
}
export function session(): SavedSession | null {
  const v = read('session') as Partial<SavedSession> | null;
  return v && typeof v.id === 'string' && typeof v.rows === 'string' && typeof v.actions === 'string'
    ? v as SavedSession : null;
}
export function best(id: string, rows: string): Best | null {
  const v = read('best.' + id) as Partial<Best> | null;
  return v && v.rows === rows && typeof v.route === 'string' && Number.isInteger(v.moves) && Number.isInteger(v.pushes)
    ? v as Best : null;
}
// getRandomValues works over plain HTTP; randomUUID needs a secure context.
function randomId(): string {
  return [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, '0')).join('');
}
export function profile(): string | null {
  const value = read('profile');
  if (typeof value === 'string' && /^[a-f0-9]{32}$/.test(value)) return value;
  const id = randomId();
  return write('profile', id) ? id : null;
}
