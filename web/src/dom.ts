/** The DOM hands the pages their elements and their small writers: the
 * element getters, the select reader, and the message setters. The leaf
 * every module shares. */
export const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
export const button = (id: string) => $<HTMLButtonElement>(id);
export const select = (id: string) => $<HTMLSelectElement>(id);
/** The selected value of a select whose options are exactly `values`. */
export function choice<T extends string>(id: string, values: readonly T[]): T {
  const value = select(id).value;
  const known = values.find(item => item === value);
  if (known === undefined) throw new Error(`Unknown #${id} option: ${value}`);
  return known;
}
// role=status re-announces every write, so identical text is left alone.
export function setText(id: string, value: string) {
  const el = $(id);
  if (el.textContent !== value) el.textContent = value;
}
export const message = (value: string) => setText('message', value);
export const setStatus = (value: string) => setText('search-status', value);
