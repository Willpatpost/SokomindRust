/** Clock and timers, injected so unit tests can drive time with a fake. */
export interface Scheduler {
  now(): number;
  interval(callback: () => void, ms: number): number;
  timeout(callback: () => void, ms: number): number;
  clearInterval(handle: number): void;
  clearTimeout(handle: number): void;
}
export const browserScheduler: Scheduler = {
  now: () => performance.now(),
  interval: (fn, ms) => window.setInterval(fn, ms),
  timeout: (fn, ms) => window.setTimeout(fn, ms),
  clearInterval: handle => window.clearInterval(handle),
  clearTimeout: handle => window.clearTimeout(handle),
};
